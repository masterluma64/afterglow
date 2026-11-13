//! A still frame of each video, to show it by in the playlist and on the
//! home screen. They are taken on a background thread with a silent mpv of
//! their own, which starts with the first one asked for, and kept for as long
//! as Afterglow runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage, TextureHandle, TextureId, TextureOptions};

use crate::player::mpv::{self, Mpv};

/// Size stills are taken at, in pixels: sharp up to twice the size they are
/// shown at. Other aspect ratios get bars.
pub const SIZE: [u32; 2] = [192, 108];
/// How far into a video the still is taken: past the opening, before
/// anything is given away.
const AT: &str = "20%";
/// How long a video may take to open before it is given up on.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Stills kept before they are all dropped, to be taken again as needed.
const MAX_KEPT: usize = 400;

/// Videos waiting for a still, the most recently asked for last: they are
/// taken from the end, so the rows in view go first while scrolling.
#[derive(Default)]
struct Queue {
    videos: Mutex<Vec<PathBuf>>,
    ready: Condvar,
}

pub struct Stills {
    queue: Arc<Queue>,
    results: Option<Receiver<(PathBuf, Option<ColorImage>)>>,
    /// Each video's still once taken: `None` if it couldn't be.
    kept: HashMap<PathBuf, Option<TextureHandle>>,
    ctx: egui::Context,
}

impl Stills {
    pub fn new(ctx: egui::Context) -> Self {
        Self { queue: Arc::default(), results: None, kept: HashMap::new(), ctx }
    }

    /// The still of `video`, once it has been taken; asks for it otherwise.
    pub fn get(&mut self, video: &Path) -> Option<TextureId> {
        if let Some(still) = self.kept.get(video) {
            return still.as_ref().map(TextureHandle::id);
        }
        self.start();
        let mut videos = self.queue.videos.lock().unwrap();
        match videos.iter().position(|v| v == video) {
            // Asked for again: move it to the front of the line.
            Some(i) if i + 1 != videos.len() => {
                let video = videos.remove(i);
                videos.push(video);
            }
            Some(_) => {}
            None => {
                videos.push(video.to_path_buf());
                self.queue.ready.notify_one();
            }
        }
        None
    }

    /// Takes in the stills taken since the last call; call once per frame.
    pub fn update(&mut self) {
        let Some(results) = &self.results else { return };
        let taken: Vec<_> = results.try_iter().collect();
        if self.kept.len() + taken.len() > MAX_KEPT {
            self.kept.clear();
        }
        for (video, image) in taken {
            let texture = image.map(|image| self.ctx.load_texture("still", image, TextureOptions::LINEAR));
            self.kept.insert(video, texture);
        }
    }

    /// Starts the thread that takes them, the first time one is asked for.
    fn start(&mut self) {
        if self.results.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.results = Some(rx);
        let (queue, ctx) = (self.queue.clone(), self.ctx.clone());
        let spawned = std::thread::Builder::new().name("stills".into()).spawn(move || {
            let mpv = match Mpv::thumbnailer(ctx.clone()) {
                Ok(mpv) => mpv,
                Err(e) => return eprintln!("could not start taking stills: {e}"),
            };
            mpv.set_render_size(SIZE[0], SIZE[1]);
            loop {
                let video = {
                    let mut videos = queue.videos.lock().unwrap();
                    loop {
                        match videos.pop() {
                            Some(video) => break video,
                            None => videos = queue.ready.wait(videos).unwrap(),
                        }
                    }
                };
                let still = take(&mpv, &video);
                if tx.send((video, still)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        if let Err(e) = spawned {
            eprintln!("could not start taking stills: {e}");
        }
    }
}

/// Opens `video` at `AT` and waits for its frame.
fn take(mpv: &Mpv, video: &Path) -> Option<ColorImage> {
    if !video.is_file() {
        return None;
    }
    // No frame of the video before shows for this one.
    mpv.hide_frames();
    mpv.set("start", AT);
    mpv.send(&["loadfile", &video.to_string_lossy(), "replace"]);
    let deadline = Instant::now() + TIMEOUT;
    let mut started = false;
    while Instant::now() < deadline {
        for event in mpv.poll_events() {
            match event {
                mpv::Event::PlaybackRestart => {
                    mpv.show_frames();
                    started = true;
                }
                mpv::Event::EndFile { error: true, .. } => return None,
                _ => {}
            }
        }
        if started && let Some(frame) = mpv.take_frame() {
            mpv.send(&["stop"]);
            return Some(frame);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    mpv.send(&["stop"]);
    None
}
