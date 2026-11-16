//! The Afterglow setup: one window, one question, one button.
//!
//! It carries afterglow.exe inside it (built in by build.rs), with its
//! license and notices, asks where they should go, writes them there, then
//! has the new afterglow.exe add its shortcuts and registry entries
//! (`afterglow.exe --register`, see `src/platform/install.rs`). No
//! administrator rights are needed. Over an earlier install it updates that
//! copy where it is.
//!
//! `--quiet` installs without a window, where an earlier install is (or in
//! the usual folder), keeping its shortcuts as they were.

#![windows_subsystem = "windows"]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use eframe::egui;

#[path = "../../src/platform/installed.rs"]
mod installed;

#[path = "../../src/ui/app_icon.rs"]
mod app_icon;

const APP: &[u8] = include_bytes!(env!("AFTERGLOW_PAYLOAD"));
const LICENSE: &[u8] = include_bytes!("../../LICENSE");
const NOTICES: &[u8] = include_bytes!("../../THIRD-PARTY-NOTICES.txt");
const VERSION: &str = env!("AFTERGLOW_VERSION");

const TITLE: &str = "Afterglow Setup";
const NAME: &str = "Afterglow";
/// Phosphor's folder-open icon, for the Browse button.
const FOLDER_OPEN: &str = "\u{E256}";

fn main() {
    if std::env::args().any(|a| a == "--quiet") {
        std::process::exit(match quiet() {
            Ok(()) => 0,
            Err(message) => {
                eprintln!("{message}");
                2
            }
        });
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([520.0, 360.0])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_icon(std::sync::Arc::new(egui::IconData { rgba: app_icon::render(256), width: 256, height: 256 })),
        centered: true,
        ..Default::default()
    };
    let _ = eframe::run_native(TITLE, options, Box::new(|cc| Ok(Box::new(Setup::new(cc)))));
}

/// Installs without asking, as `--quiet` does.
fn quiet() -> Result<(), String> {
    let earlier = installed::find();
    let dir = match &earlier {
        Some(earlier) => earlier.dir.clone(),
        None => installed::default_dir().ok_or("The folder for programs (%LOCALAPPDATA%) wasn't found.")?,
    };
    let (start_menu, desktop) = earlier.as_ref().map_or((true, false), |e| (e.start_menu, e.desktop));
    install(&dir, start_menu, desktop)
}

/// What an install did, or why it couldn't.
type Outcome = Result<(), String>;

/// Closes a running Afterglow in `dir`, writes the files and registers them.
fn install(dir: &Path, start_menu: bool, desktop: bool) -> Outcome {
    let exe = dir.join(installed::EXE);
    if !installed::close_running(&exe, Duration::from_secs(10)) {
        return Err("Afterglow is open and didn't close. Close it, then run the setup again.".into());
    }
    put_files(dir).map_err(|e| format!("Writing to {} failed: {e}", dir.display()))?;
    let mut register = Command::new(&exe);
    // Quietly: what goes wrong is shown here instead.
    register.args(["--register", "--quiet"]).current_dir(dir);
    if desktop {
        register.arg("--desktop");
    }
    if !start_menu {
        register.arg("--no-start-menu");
    }
    let output = register.output().map_err(|e| format!("{} couldn't be started: {e}", exe.display()))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if said.is_empty() { "Afterglow couldn't register itself.".into() } else { said });
    }
    Ok(())
}

/// Writes the files, the exe last and through a temporary name, so a
/// failure doesn't leave half an exe behind.
fn put_files(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("LICENSE.txt"), LICENSE)?;
    std::fs::write(dir.join("THIRD-PARTY-NOTICES.txt"), NOTICES)?;
    let new = dir.join("afterglow.exe.new");
    std::fs::write(&new, APP)?;
    // The old exe was only just closed, so it may take a moment to be free.
    let mut tries = 0;
    loop {
        match std::fs::rename(&new, dir.join(installed::EXE)) {
            Err(_) if tries < 20 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&new);
                return Err(e);
            }
            Ok(()) => return Ok(()),
        }
    }
}

/// Where the setup has got to.
enum Stage {
    /// Asking where Afterglow should go.
    Asking,
    /// Writing, on a thread of its own so the window keeps painting.
    Working(Receiver<Outcome>),
    /// Done, with where it went.
    Done(PathBuf),
    Failed(String),
}

struct Setup {
    /// Afterglow's icon, drawn once and kept.
    icon: Option<egui::TextureHandle>,
    stage: Stage,
    /// The folder being written to, once the work has started.
    target: PathBuf,
    /// The installed version, when there is an install to update.
    earlier: Option<String>,
    dir: String,
    start_menu: bool,
    desktop: bool,
    launch: bool,
}

impl Setup {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert("phosphor".into(), std::sync::Arc::new(egui::FontData::from_static(include_bytes!("../../assets/fonts/Phosphor.ttf"))));
        fonts.families.entry(egui::FontFamily::Proportional).or_default().push("phosphor".into());
        cc.egui_ctx.set_fonts(fonts);
        // Where it already is, when there is somewhere, else the usual place.
        let earlier = installed::find();
        let dir = earlier.as_ref().map(|e| e.dir.clone()).or_else(installed::default_dir).unwrap_or_default();
        let (start_menu, desktop) = earlier.as_ref().map_or((true, false), |e| (e.start_menu, e.desktop));
        Self {
            icon: None,
            stage: Stage::Asking,
            target: PathBuf::new(),
            earlier: earlier.map(|e| e.version),
            dir: dir.display().to_string(),
            start_menu,
            desktop,
            launch: true,
        }
    }

    /// Starts the work and leaves the window free to draw. The thread wakes
    /// the window when it's done, since nothing else would.
    fn act(&mut self, ctx: &egui::Context) {
        let dir = PathBuf::from(self.dir.trim());
        self.target = dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let (start_menu, desktop) = (self.start_menu, self.desktop);
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("install".into()).spawn(move || {
            let _ = tx.send(install(&dir, start_menu, desktop));
            ctx.request_repaint();
        });
        self.stage = match spawned {
            Ok(_) => Stage::Working(rx),
            Err(e) => Stage::Failed(format!("The setup couldn't start writing: {e}")),
        };
    }

    /// Takes the answer when the thread has one.
    fn finished(&mut self) {
        let Stage::Working(rx) = &self.stage else { return };
        let outcome = match rx.try_recv() {
            Ok(outcome) => outcome,
            // Still writing, which is most frames.
            Err(TryRecvError::Empty) => return,
            // The thread went without answering: nothing is coming, and a
            // spinner that never stops is worse than saying so.
            Err(TryRecvError::Disconnected) => Err("Writing stopped before it finished. Close this and try again.".into()),
        };
        self.stage = match outcome {
            Ok(()) => Stage::Done(std::mem::take(&mut self.target)),
            Err(why) => Stage::Failed(why),
        };
    }

    /// Whether this updates an earlier install (or reinstalls it).
    fn updating(&self) -> bool {
        self.earlier.is_some()
    }
}

impl eframe::App for Setup {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Room around everything: the window is small, and its controls
        // shouldn't sit against its edges.
        egui::Frame::new().inner_margin(egui::Margin { left: 22, right: 22, top: 12, bottom: 14 }).show(ui, |ui| self.body(ui));
    }
}

impl Setup {
    fn body(&mut self, ui: &mut egui::Ui) {
        let icon = self
            .icon
            .get_or_insert_with(|| {
                let image = egui::ColorImage::from_rgba_unmultiplied([128, 128], &app_icon::render(128));
                ui.ctx().load_texture("setup-icon", image, egui::TextureOptions::LINEAR)
            })
            .id();
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            ui.add(egui::Image::new((icon, egui::vec2(84.0, 84.0))));
            ui.label(egui::RichText::new(NAME).size(20.0).strong());
            ui.weak(format!("Version {VERSION}"));
        });
        ui.add_space(12.0);

        self.finished();
        match &self.stage {
            Stage::Asking => self.ask(ui),
            Stage::Working(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(if self.updating() { "Updating\u{2026}" } else { "Installing\u{2026}" });
                });
                ui.add_space(4.0);
                ui.weak(self.target.display().to_string());
                // egui draws on demand; without this the spinner would sit still.
                ui.ctx().request_repaint();
            }
            Stage::Done(dir) => {
                let dir = dir.clone();
                ui.label(if self.updating() { "Updated." } else { "Installed." });
                ui.weak(dir.display().to_string());
                ui.add_space(8.0);
                ui.checkbox(&mut self.launch, "Open it now");
                self.footer(ui, "Close", |setup| {
                    if setup.launch {
                        let _ = Command::new(dir.join(installed::EXE)).current_dir(&dir).spawn();
                    }
                    true
                });
            }
            Stage::Failed(why) => {
                let why = why.clone();
                ui.colored_label(ui.visuals().error_fg_color, &why);
                ui.add_space(8.0);
                self.footer(ui, "Close", |_| true);
            }
        }
    }

    /// The question: where should it go.
    fn ask(&mut self, ui: &mut egui::Ui) {
        let updating = self.updating();
        ui.label(if updating { "Installed in:" } else { "Install it in:" });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            // An earlier install is updated where it is.
            ui.add_enabled_ui(!updating, |ui| {
                let width = ui.available_width() - 90.0;
                ui.add_sized([width, 24.0], egui::TextEdit::singleline(&mut self.dir));
                if ui.button(format!("{FOLDER_OPEN} Browse")).clicked()
                    && let Some(picked) = rfd::FileDialog::new().set_title("Choose a folder").pick_folder()
                {
                    self.dir = in_own_folder(picked).display().to_string();
                }
            });
        });
        ui.add_space(10.0);
        ui.checkbox(&mut self.start_menu, "Add a shortcut to the Start menu");
        ui.checkbox(&mut self.desktop, "Add a shortcut to the desktop");

        let label = match &self.earlier {
            Some(version) if version == VERSION => "Reinstall",
            Some(_) => "Update",
            None => "Install",
        };
        let ctx = ui.ctx().clone();
        self.footer(ui, label, move |setup| {
            setup.act(&ctx);
            false
        });
    }

    /// The bottom row: the one button that does the thing. It closes the
    /// window when `act` says so.
    fn footer(&mut self, ui: &mut egui::Ui, label: &str, act: impl FnOnce(&mut Self) -> bool) {
        let enabled = !self.dir.trim().is_empty();
        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);
        let mut done = false;
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let button = egui::Button::new(egui::RichText::new(label).strong()).min_size(egui::vec2(110.0, 28.0));
                if ui.add_enabled(enabled, button).clicked() {
                    done = act(self);
                }
            });
        });
        if done {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// A folder of Afterglow's own inside the one picked, so uninstalling never
/// takes a folder that was full of other things.
fn in_own_folder(picked: PathBuf) -> PathBuf {
    let own = picked.file_name().is_some_and(|name| name.eq_ignore_ascii_case(NAME));
    if own { picked } else { picked.join(NAME) }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Sender, channel};

    use super::*;

    /// A setup part-way through the work, and the channel the thread would
    /// answer on. The window is never built: what is under test is which
    /// stage the answer leaves it in.
    fn working() -> (Setup, Sender<Outcome>) {
        let (tx, rx) = channel();
        let target = PathBuf::from(r"C:\Programs\Afterglow");
        let setup = Setup {
            icon: None,
            stage: Stage::Working(rx),
            target: target.clone(),
            earlier: None,
            dir: target.display().to_string(),
            start_menu: true,
            desktop: false,
            launch: true,
        };
        (setup, tx)
    }

    #[test]
    fn a_thread_still_writing_is_waited_for() {
        let (mut setup, _tx) = working();
        setup.finished();
        assert!(matches!(setup.stage, Stage::Working(_)));
    }

    #[test]
    fn an_install_that_worked_says_where_it_went() {
        let (mut setup, tx) = working();
        let target = setup.target.clone();
        tx.send(Ok(())).unwrap();
        setup.finished();
        assert!(matches!(setup.stage, Stage::Done(ref dir) if *dir == target));
    }

    #[test]
    fn a_failure_is_shown_as_it_came() {
        let (mut setup, tx) = working();
        tx.send(Err("the disk is full".into())).unwrap();
        setup.finished();
        assert!(matches!(setup.stage, Stage::Failed(ref why) if why == "the disk is full"));
    }

    /// The thread is gone and no answer was ever sent, as when the work
    /// panicked: that's a failure, not a spinner for ever.
    #[test]
    fn a_thread_that_dies_without_answering_is_a_failure() {
        let (mut setup, tx) = working();
        drop(tx);
        setup.finished();
        assert!(matches!(setup.stage, Stage::Failed(ref why) if !why.is_empty()));
    }

    #[test]
    fn a_picked_folder_gets_a_folder_of_its_own() {
        assert_eq!(in_own_folder(PathBuf::from(r"D:\Apps")), PathBuf::from(r"D:\Apps\Afterglow"));
        assert_eq!(in_own_folder(PathBuf::from(r"D:\Apps\afterglow")), PathBuf::from(r"D:\Apps\afterglow"));
    }
}
