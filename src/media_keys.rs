//! Windows' system media controls: the keyboard's media keys (even while
//! another window has focus) and the media overlay showing what's playing.

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use eframe::egui;
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};

pub use souvlaki::{MediaControlEvent as Event, SeekDirection};

pub struct MediaKeys {
    controls: MediaControls,
    events: Receiver<MediaControlEvent>,
    /// What was last shown: the title, whether its length was known, and
    /// whether it was playing.
    shown: Option<(Option<String>, bool, bool)>,
}

impl MediaKeys {
    /// Registers with the system for the window `hwnd`.
    pub fn new(hwnd: *mut std::ffi::c_void, ctx: egui::Context) -> Result<Self, String> {
        let config = PlatformConfig { display_name: crate::APP_NAME, dbus_name: crate::APP_ID, hwnd: Some(hwnd) };
        let mut controls = MediaControls::new(config).map_err(|e| format!("{e:?}"))?;
        let (tx, events) = mpsc::channel();
        controls
            .attach(move |event| {
                if tx.send(event).is_ok() {
                    ctx.request_repaint();
                }
            })
            .map_err(|e| format!("{e:?}"))?;
        Ok(Self { controls, events, shown: None })
    }

    /// Media key presses since the last call.
    pub fn events(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// Shows what's playing in the system's media overlay. Only talks to the
    /// system when something shown changed.
    pub fn show(&mut self, title: Option<&str>, playing: bool, position: f64, duration: f64) {
        let state = (title.map(str::to_owned), duration > 0.0, playing);
        if self.shown.as_ref() == Some(&state) {
            return;
        }
        if self.shown.as_ref().map(|(t, d, _)| (t, *d)) != Some((&state.0, state.1)) {
            let metadata = MediaMetadata {
                title,
                duration: (duration > 0.0).then(|| Duration::from_secs_f64(duration)),
                ..Default::default()
            };
            let _ = self.controls.set_metadata(metadata);
        }
        let progress = Some(MediaPosition(Duration::from_secs_f64(position.max(0.0))));
        let playback = match (title, playing) {
            (None, _) => MediaPlayback::Stopped,
            (Some(_), true) => MediaPlayback::Playing { progress },
            (Some(_), false) => MediaPlayback::Paused { progress },
        };
        let _ = self.controls.set_playback(playback);
        self.shown = Some(state);
    }
}
