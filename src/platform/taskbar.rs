//! Afterglow's taskbar button: the playing video's progress on it, filling
//! up while it plays and yellow while it is paused, and previous, play/pause
//! and next buttons in the preview shown when pointing at it.

use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::UI::Shell::{
    DefSubclassProc, ITaskbarList3, SetWindowSubclass, TBPF_NOPROGRESS, TBPF_NORMAL, TBPF_PAUSED, TBPFLAG, THB_FLAGS,
    THB_ICON, THB_TOOLTIP, THBF_DISABLED, THBF_ENABLED, THBN_CLICKED, THUMBBUTTON, TaskbarList,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateIcon, GetSystemMetrics, HICON, SM_CXSMICON, WM_COMMAND};

/// Steps the bar is shown in: finer than a taskbar button has pixels.
const STEPS: u64 = 1000;
/// Tells our subclass of the window apart from others.
const SUBCLASS_ID: usize = 0xAF7E;

/// A button in the taskbar preview.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Previous,
    PlayPause,
    Next,
}

const BUTTONS: [Button; 3] = [Button::Previous, Button::PlayPause, Button::Next];

/// What the preview's buttons show: whether the video plays, and whether
/// there is a previous and a next one.
#[derive(Clone, Copy, PartialEq)]
pub struct Buttons {
    pub playing: bool,
    pub has_previous: bool,
    pub has_next: bool,
}

/// The icons the buttons are drawn with.
struct Icons {
    play: HICON,
    pause: HICON,
    previous: HICON,
    next: HICON,
}

/// Use it from the UI thread, which owns the window.
pub struct Taskbar {
    list: ITaskbarList3,
    hwnd: HWND,
    /// What was last shown: the state, and how far, in `STEPS`.
    shown: Option<(TBPFLAG, u64)>,
    icons: Option<Icons>,
    /// Whether the buttons were added (that fails until the taskbar button
    /// exists), and what they showed last.
    added: bool,
    buttons: Option<Buttons>,
    clicks: Receiver<Button>,
}

impl Taskbar {
    /// Sets up the taskbar button of the window `hwnd`; clicks on its
    /// buttons repaint `ctx`.
    pub fn new(hwnd: isize, ctx: egui::Context) -> Result<Self, String> {
        let hwnd = HWND(hwnd as *mut _);
        let list = unsafe {
            // The UI thread has COM already (for drag and drop); this is in
            // case it doesn't, and is never undone, as the list lives as long.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let list: ITaskbarList3 = CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message())?;
            list.HrInit().map_err(|e| e.message())?;
            list
        };
        // The buttons report clicks to the window, as WM_COMMAND messages,
        // which a subclass of it passes on. It lives as long as the window.
        let (tx, clicks) = mpsc::channel();
        let state = Box::into_raw(Box::new((tx, ctx)));
        if !unsafe { SetWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID, state as usize) }.as_bool() {
            drop(unsafe { Box::from_raw(state) });
            eprintln!("the taskbar preview buttons won't respond");
        }
        Ok(Self { list, hwnd, shown: None, icons: Icons::new(), added: false, buttons: None, clicks })
    }

    /// Shows how far into the video playback is (0 to 1), or no bar for
    /// `None`. Only talks to the taskbar when what it shows changes.
    pub fn show(&mut self, progress: Option<f64>, paused: bool) {
        let state = match progress {
            None => TBPF_NOPROGRESS,
            Some(_) if paused => TBPF_PAUSED,
            Some(_) => TBPF_NORMAL,
        };
        let value = progress.map_or(0, |p| (p.clamp(0.0, 1.0) * STEPS as f64).round() as u64);
        if self.shown == Some((state, value)) {
            return;
        }
        // Setting a value turns the bar on in the normal state, so the state
        // goes after it.
        let set_value = || if progress.is_some() { unsafe { self.list.SetProgressValue(self.hwnd, value, STEPS) } } else { Ok(()) };
        let result = set_value().and_then(|()| unsafe { self.list.SetProgressState(self.hwnd, state) });
        // Until the taskbar button exists, this fails; it is tried again
        // on the next change.
        if result.is_ok() {
            self.shown = Some((state, value));
        }
    }

    /// Shows the preview's buttons as `buttons` says, adding them the first
    /// time. Only talks to the taskbar when they change.
    pub fn set_buttons(&mut self, buttons: Buttons) {
        let Some(icons) = &self.icons else { return };
        if self.added && self.buttons == Some(buttons) {
            return;
        }
        let list: Vec<THUMBBUTTON> = BUTTONS
            .iter()
            .enumerate()
            .map(|(id, &button)| {
                let (icon, tip, enabled) = match button {
                    Button::Previous => (icons.previous, "Previous video", buttons.has_previous),
                    Button::PlayPause if buttons.playing => (icons.pause, "Pause", true),
                    Button::PlayPause => (icons.play, "Play", true),
                    Button::Next => (icons.next, "Next video", buttons.has_next),
                };
                let mut sz_tip = [0u16; 260];
                for (slot, c) in sz_tip.iter_mut().zip(tip.encode_utf16()) {
                    *slot = c;
                }
                THUMBBUTTON {
                    dwMask: THB_ICON | THB_TOOLTIP | THB_FLAGS,
                    iId: id as u32,
                    iBitmap: 0,
                    hIcon: icon,
                    szTip: sz_tip,
                    dwFlags: if enabled { THBF_ENABLED } else { THBF_DISABLED },
                }
            })
            .collect();
        let result = unsafe {
            if self.added { self.list.ThumbBarUpdateButtons(self.hwnd, &list) } else { self.list.ThumbBarAddButtons(self.hwnd, &list) }
        };
        // Adding fails until the taskbar button exists; it is tried again.
        if result.is_ok() {
            self.added = true;
            self.buttons = Some(buttons);
        }
    }

    /// The preview's buttons clicked since the last call.
    pub fn clicks(&self) -> Vec<Button> {
        self.clicks.try_iter().collect()
    }
}

/// Passes clicks on the preview's buttons on; everything else goes to the
/// window as before.
unsafe extern "system" fn subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, state: usize) -> LRESULT {
    if msg == WM_COMMAND && (wparam.0 >> 16) as u32 & 0xFFFF == THBN_CLICKED {
        let (tx, ctx) = unsafe { &*(state as *const (Sender<Button>, egui::Context)) };
        if let Some(&button) = BUTTONS.get(wparam.0 & 0xFFFF) {
            let _ = tx.send(button);
            ctx.request_repaint();
        }
        return LRESULT(0);
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

impl Icons {
    fn new() -> Option<Self> {
        let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64) as usize;
        let make = |shape: fn(f32, f32) -> bool| icon(size, shape);
        Some(Self { play: make(play)?, pause: make(pause)?, previous: make(previous)?, next: make(next)? })
    }
}

// The shapes, in a square from 0 to 1, with some room around them.
fn play(x: f32, y: f32) -> bool {
    x >= 0.3 && x <= 0.8 && (y - 0.5).abs() <= 0.32 * (0.8 - x) / 0.5
}
fn pause(x: f32, y: f32) -> bool {
    (0.2..=0.8).contains(&y) && ((0.25..=0.42).contains(&x) || (0.58..=0.75).contains(&x))
}
fn next(x: f32, y: f32) -> bool {
    let triangle = x >= 0.22 && x <= 0.66 && (y - 0.5).abs() <= 0.3 * (0.66 - x) / 0.44;
    let bar = (0.68..=0.8).contains(&x) && (0.2..=0.8).contains(&y);
    triangle || bar
}
fn previous(x: f32, y: f32) -> bool {
    next(1.0 - x, y)
}

/// A white icon of `size` pixels in `shape`, its edges smoothed by sampling
/// each pixel 4 × 4 times.
fn icon(size: usize, shape: fn(f32, f32) -> bool) -> Option<HICON> {
    const SAMPLES: usize = 4;
    let mut bgra = Vec::with_capacity(size * size * 4);
    for py in 0..size {
        for px in 0..size {
            let mut hits = 0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = (px as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    hits += usize::from(shape(x, y));
                }
            }
            let alpha = (hits * 255 / (SAMPLES * SAMPLES)) as u8;
            bgra.extend_from_slice(&[255, 255, 255, alpha]);
        }
    }
    // The mask is unused with an alpha channel: rows of whole 16-bit words.
    let mask = vec![0u8; size.div_ceil(16) * 2 * size];
    unsafe { CreateIcon(None, size as i32, size as i32, 1, 32, mask.as_ptr(), bgra.as_ptr()) }.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_the_button_shapes() {
        assert!(play(0.4, 0.5) && !play(0.1, 0.5) && !play(0.75, 0.2));
        assert!(pause(0.3, 0.5) && !pause(0.5, 0.5));
        assert!(next(0.3, 0.5) && next(0.75, 0.3) && !next(0.1, 0.5));
        assert!(previous(0.7, 0.5) && previous(0.25, 0.3));
        assert!(icon(16, play).is_some());
    }
}
