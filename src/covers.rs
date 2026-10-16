//! Album cover loading on a worker thread, cached as textures.
//!
//! Thumbnails for every album seen are kept; only one large cover (for the
//! album page or now-playing view) is kept at a time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

const THUMB_PX: u32 = 220;
const LARGE_PX: u32 = 640;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Size {
    Thumb,
    Large,
}

struct Request {
    key: String,
    size: Size,
    track: PathBuf,
}

struct Loaded {
    key: String,
    size: Size,
    image: Option<ColorImage>,
}

enum Slot {
    Loading,
    Ready(TextureHandle),
    Missing,
}

pub struct Covers {
    tx: Sender<Request>,
    rx: Receiver<Loaded>,
    thumbs: HashMap<String, Slot>,
    large: Option<(String, Slot)>,
}

impl Covers {
    pub fn new(ctx: egui::Context) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<Request>();
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("covers".into())
            .spawn(move || worker(req_rx, done_tx, ctx))
            .expect("failed to spawn cover loader");
        Self { tx: req_tx, rx: done_rx, thumbs: HashMap::new(), large: None }
    }

    /// Turns finished loads into textures. Call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(loaded) = self.rx.try_recv() {
            let slot = match loaded.image {
                Some(image) => Slot::Ready(ctx.load_texture(format!("cover-{}", loaded.key), image, TextureOptions::LINEAR)),
                None => Slot::Missing,
            };
            match loaded.size {
                Size::Thumb => {
                    self.thumbs.insert(loaded.key, slot);
                }
                Size::Large => {
                    if let Some((key, s)) = &mut self.large {
                        if *key == loaded.key {
                            *s = slot;
                        }
                    }
                }
            }
        }
    }

    /// Small cover for an album, identified by `key`, loaded from `track`
    /// (embedded art) or its folder. Returns `None` while loading or if absent.
    pub fn thumb(&mut self, key: &str, track: &Path) -> Option<&TextureHandle> {
        if !self.thumbs.contains_key(key) {
            self.thumbs.insert(key.to_string(), Slot::Loading);
            let _ = self.tx.send(Request { key: key.to_string(), size: Size::Thumb, track: track.to_path_buf() });
        }
        match self.thumbs.get(key) {
            Some(Slot::Ready(tex)) => Some(tex),
            _ => None,
        }
    }

    /// Large cover; falls back to the thumbnail while it loads.
    pub fn large(&mut self, key: &str, track: &Path) -> Option<TextureHandle> {
        if self.large.as_ref().is_none_or(|(k, _)| k != key) {
            self.large = Some((key.to_string(), Slot::Loading));
            let _ = self.tx.send(Request { key: key.to_string(), size: Size::Large, track: track.to_path_buf() });
        }
        if let Some((_, Slot::Ready(tex))) = &self.large {
            return Some(tex.clone());
        }
        self.thumb(key, track).cloned()
    }

    /// Forgets everything, e.g. after a rescan.
    pub fn clear(&mut self) {
        self.thumbs.clear();
        self.large = None;
    }
}

fn worker(rx: Receiver<Request>, tx: Sender<Loaded>, ctx: egui::Context) {
    // Most recent requests first: those are the covers currently on screen.
    let mut pending: Vec<Request> = Vec::new();
    loop {
        pending.extend(rx.try_iter());
        let request = match pending.pop() {
            Some(r) => r,
            None => match rx.recv() {
                Ok(r) => r,
                Err(_) => return,
            },
        };
        let px = if request.size == Size::Thumb { THUMB_PX } else { LARGE_PX };
        let image = load(&request.track, px);
        if tx.send(Loaded { key: request.key, size: request.size, image }).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

fn load(track: &Path, px: u32) -> Option<ColorImage> {
    let bytes = crate::music::embedded_cover(track)
        .or_else(|| std::fs::read(crate::music::folder_cover(track.parent()?)?).ok())?;
    let image = image::load_from_memory(&bytes).ok()?.thumbnail(px, px).to_rgba8();
    Some(ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], &image))
}
