//! Preview frames for the seek bar, from a second, silent mpv instance.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use eframe::egui::{self, TextureHandle, TextureOptions};

use crate::player::mpv::{self, Mpv};

/// Size previews are rendered at, in pixels. Other aspect ratios get bars.
pub const SIZE: [u32; 2] = [224, 126];

pub struct Thumbnails {
    mpv: Mpv,
    /// The file opened for previews.
    path: Option<PathBuf>,
    loaded: bool,
    /// The time last asked for; the next frame shows it.
    requested: Option<f64>,
    texture: Option<TextureHandle>,
}

impl Thumbnails {
    /// Starts the previewer on a background thread, since starting mpv takes
    /// a moment; it arrives on the returned channel.
    pub fn start(ctx: &egui::Context) -> Receiver<Result<Self, String>> {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("preview-start".into()).spawn(move || {
            let _ = tx.send(Self::new(&ctx));
            ctx.request_repaint();
        });
        if let Err(e) = spawned {
            eprintln!("could not start the seek bar previews: {e}");
        }
        rx
    }

    fn new(ctx: &egui::Context) -> Result<Self, String> {
        let mpv = Mpv::thumbnailer(ctx.clone())?;
        mpv.set_render_size(SIZE[0], SIZE[1]);
        Ok(Self { mpv, path: None, loaded: false, requested: None, texture: None })
    }

    /// Asks for a preview of `path` at `time`. Times are rounded to `step`
    /// seconds so moving the pointer slightly doesn't seek again.
    pub fn request(&mut self, path: &Path, time: f64, step: f64) {
        let time = (time / step).round() * step;
        if self.path.as_deref() != Some(path) {
            self.path = Some(path.to_path_buf());
            self.loaded = false;
            self.texture = None;
            self.requested = Some(time);
            self.mpv.load_file(path, time, 0);
            return;
        }
        if self.loaded && self.requested != Some(time) {
            self.requested = Some(time);
            self.mpv.send(&["seek", &format!("{time:.3}"), "absolute+keyframes"]);
        }
    }

    /// Takes in new frames; call once per UI frame.
    pub fn update(&mut self, ctx: &egui::Context) {
        for event in self.mpv.poll_events() {
            match event {
                mpv::Event::FileLoaded => self.loaded = true,
                // Replacing the file also ends the old one; only a real end
                // (seeking past the last keyframe, or an error) needs a reload.
                mpv::Event::EndFile { eof, error, .. } if eof || error => {
                    self.loaded = false;
                    self.path = None;
                }
                mpv::Event::EndFile { .. }
                | mpv::Event::Property { .. }
                | mpv::Event::CommandDone { .. }
                | mpv::Event::PlaybackRestart => {}
            }
        }
        if let Some(image) = self.mpv.take_frame() {
            match &mut self.texture {
                Some(tex) => tex.set(image, TextureOptions::LINEAR),
                None => self.texture = Some(ctx.load_texture("thumbnail", image, TextureOptions::LINEAR)),
            }
        }
    }

    /// The latest preview of `path`, if there is one.
    pub fn texture(&self, path: &Path) -> Option<&TextureHandle> {
        self.texture.as_ref().filter(|_| self.path.as_deref() == Some(path))
    }
}
