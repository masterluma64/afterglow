//! Finding a show's opening by listening: the theme song sounds the same in
//! every episode, while the rest of the sound differs. The start of two
//! episodes is decoded (by a silent mpv of its own, on a background thread),
//! each turned into an audio fingerprint, and the longest stretch of sound
//! they share is the opening.
//!
//! The fingerprint is that of Haitsma and Kalker ("A Highly Robust Audio
//! Fingerprinting System", 2002): 32 bits per short step of sound, each
//! telling whether the energy difference between two neighbouring frequency
//! bands grew or shrank since the step before. It doesn't change with the
//! volume, and little with how the sound was encoded.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use eframe::egui;

use crate::player::mpv;

/// Samples per second the sound is listened to at.
pub const RATE: u32 = 11025;
/// Samples in each window the frequencies are measured over (0.37 s).
const FRAME: usize = 4096;
/// Samples from one window to the next (46 ms), the fingerprint's step.
const HOP: usize = 512;
/// The frequency bands compared, from 300 Hz to 2 kHz, where most of what's
/// heard is: 33 bands give 32 differences, one bit each.
const BANDS: usize = 33;
const LOWEST: f32 = 300.0;
const HIGHEST: f32 = 2000.0;
/// Steps quieter than this (as a root mean square, of 1) are left out:
/// silence sounds the same in every episode.
const QUIET: f32 = 0.004;
/// How many of a step's 32 bits may differ for it to count as the same sound.
const MAX_BITS: u32 = 10;
/// How long the shared sound may stop matching (as a step's bits do now and
/// then) before it counts as ended, in steps (1.4 s).
const MAX_GAP: usize = 30;
/// How much of a shared stretch must match, as a fraction of its steps.
const MIN_DENSITY: f32 = 0.5;
/// The shortest and longest an opening can be, in seconds.
const MIN_OPENING: f64 = 15.0;
pub const MAX_OPENING: f64 = 180.0;
/// How much of each episode is listened to, in seconds: openings come
/// early, though some shows have a scene first.
pub const LISTEN: f64 = 420.0;
/// How long decoding one episode may take before it's given up on.
const TIMEOUT: Duration = Duration::from_secs(120);
/// Fingerprints kept for the next episodes to be compared with.
const KEPT: usize = 6;

/// A fingerprint: 32 bits per step, and whether the step is loud enough to
/// compare.
#[derive(Clone, Debug, Default)]
pub struct Print {
    bits: Vec<u32>,
    loud: Vec<bool>,
}

/// The fingerprint of mono sound at `RATE`, with samples from -1 to 1.
pub fn fingerprint(samples: &[f32]) -> Print {
    let window: Vec<f32> = (0..FRAME).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FRAME as f32).cos()).collect();
    // The FFT bins each band adds up, spaced evenly in pitch.
    let bin = |hz: f32| ((hz * FRAME as f32 / RATE as f32).round() as usize).max(1);
    let edges: Vec<usize> = (0..=BANDS).map(|b| bin(LOWEST * (HIGHEST / LOWEST).powf(b as f32 / BANDS as f32))).collect();
    let mut print = Print::default();
    let (mut re, mut im) = (vec![0f32; FRAME], vec![0f32; FRAME]);
    let mut before: Option<[f32; BANDS]> = None;
    let mut start = 0;
    while start + FRAME <= samples.len() {
        let frame = &samples[start..start + FRAME];
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / FRAME as f32).sqrt();
        for i in 0..FRAME {
            re[i] = frame[i] * window[i];
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        let mut energy = [0f32; BANDS];
        for (b, e) in energy.iter_mut().enumerate() {
            *e = (edges[b]..edges[b + 1].max(edges[b] + 1)).map(|k| re[k] * re[k] + im[k] * im[k]).sum();
        }
        if let Some(before) = before {
            let mut bits = 0u32;
            for b in 0..BANDS - 1 {
                if (energy[b] - energy[b + 1]) - (before[b] - before[b + 1]) > 0.0 {
                    bits |= 1 << b;
                }
            }
            print.bits.push(bits);
            print.loud.push(rms >= QUIET);
        }
        before = Some(energy);
        start += HOP;
    }
    print
}

/// An in-place radix-2 FFT; the length must be a power of two.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -std::f32::consts::TAU / len as f32;
        let (wr, wi) = (angle.cos(), angle.sin());
        for chunk in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1f32, 0f32);
            for k in 0..len / 2 {
                let (a, b) = (chunk + k, chunk + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                (cr, ci) = (cr * wr - ci * wi, cr * wi + ci * wr);
            }
        }
        len <<= 1;
    }
}

/// Seconds from the start of the sound to the middle of step `i`.
fn time_of(step: usize) -> f64 {
    // A step's bits compare its window with the one before.
    ((step + 1) * HOP + FRAME / 2) as f64 / RATE as f64
}

/// The longest stretch of sound two fingerprints share, as an opening is
/// long: where it is in each, in seconds.
pub fn shared(a: &Print, b: &Print) -> Option<((f64, f64), (f64, f64))> {
    let steps = |seconds: f64| (seconds * RATE as f64 / HOP as f64) as usize;
    let (min, max) = (steps(MIN_OPENING), steps(MAX_OPENING));
    // The best stretch: its length in steps, and its first and last step in `a`, and the offset to `b`.
    let mut best: Option<(usize, usize, usize, isize)> = None;
    for offset in -(b.bits.len() as isize - 1)..a.bits.len() as isize {
        // Step i of `a` lines up with step i - offset of `b`.
        let from = offset.max(0) as usize;
        let to = a.bits.len().min((b.bits.len() as isize + offset) as usize);
        if to <= from + min {
            continue;
        }
        let mut run: Option<(usize, usize, usize)> = None; // first, last, matching steps
        let mut close = |run: Option<(usize, usize, usize)>| {
            if let Some((first, last, matched)) = run {
                let len = last - first + 1;
                if (min..=max).contains(&len)
                    && matched as f32 >= MIN_DENSITY * len as f32
                    && best.is_none_or(|(best_len, ..)| len > best_len)
                {
                    best = Some((len, first, last, offset));
                }
            }
        };
        for i in from..to {
            let j = (i as isize - offset) as usize;
            let same = a.loud[i] && b.loud[j] && (a.bits[i] ^ b.bits[j]).count_ones() <= MAX_BITS;
            if !same {
                continue;
            }
            run = match run {
                Some((first, last, matched)) if i - last <= MAX_GAP => Some((first, i, matched + 1)),
                ended => {
                    close(ended);
                    Some((i, i, 1))
                }
            };
        }
        close(run);
    }
    let (_, first, last, offset) = best?;
    let in_b = |i: usize| (i as isize - offset) as usize;
    // A stray match just past either end stretches the run: its ends are
    // where most steps match.
    let matches = |i: usize| a.loud[i] && b.loud[in_b(i)] && (a.bits[i] ^ b.bits[in_b(i)]).count_ones() <= MAX_BITS;
    let solid = |from: usize, to: usize| (from..=to).filter(|&i| matches(i)).count() * 2 > to - from;
    let edge = 10;
    let mut first = first;
    while first + edge < last && !solid(first, first + edge) {
        first = (first + 1..last).find(|&i| matches(i)).unwrap_or(last);
    }
    let mut last = last;
    while last > first + edge && !solid(last - edge, last) {
        last = (first..last).rev().find(|&i| matches(i)).unwrap_or(first);
    }
    if ((last - first + 1) as f64) < MIN_OPENING * RATE as f64 / HOP as f64 {
        return None;
    }
    Some(((time_of(first), time_of(last)), (time_of(in_b(first)), time_of(in_b(last)))))
}

/// An opening heard in a video, in seconds from its start: `None` if it
/// was listened to, but none was found.
pub type Opening = Option<(f64, f64)>;

/// Listens to the openings of videos asked for, on a background thread.
pub struct Listener {
    asks: Sender<(PathBuf, Vec<PathBuf>)>,
    /// What was heard: the video asked for, and the others whose opening
    /// was found along the way.
    pub heard: Receiver<Vec<(PathBuf, Opening)>>,
}

impl Listener {
    pub fn new(ctx: egui::Context) -> Self {
        let (asks, ask_rx) = mpsc::channel::<(PathBuf, Vec<PathBuf>)>();
        let (heard_tx, heard) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("listen".into()).spawn(move || {
            let mut prints: Vec<(PathBuf, Option<Print>)> = Vec::new();
            let mut print_of = |path: &Path| -> Option<Print> {
                if let Some((_, print)) = prints.iter().find(|(p, _)| p == path) {
                    return print.clone();
                }
                let print = mpv::decode_audio(path, LISTEN, RATE, TIMEOUT).map(|s| fingerprint(&s));
                if prints.len() >= KEPT {
                    prints.remove(0);
                }
                prints.push((path.to_path_buf(), print.clone()));
                print
            };
            for (video, others) in ask_rx {
                let heard = listen(&video, &others, &mut print_of);
                if heard_tx.send(heard).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        if let Err(e) = spawned {
            eprintln!("could not start listening for openings: {e}");
        }
        Self { asks, heard }
    }

    /// Listens for the opening of `video`, comparing it with `others`
    /// (episodes of the same show, the likeliest first).
    pub fn ask(&self, video: &Path, others: Vec<PathBuf>) {
        let _ = self.asks.send((video.to_path_buf(), others));
    }
}

/// Compares `video` with each of `others` until they share an opening.
fn listen(video: &Path, others: &[PathBuf], print_of: &mut impl FnMut(&Path) -> Option<Print>) -> Vec<(PathBuf, Opening)> {
    let Some(print) = print_of(video) else { return vec![(video.to_path_buf(), None)] };
    for other in others {
        let Some(other_print) = print_of(other) else { continue };
        if let Some((here, there)) = shared(&print, &other_print) {
            return vec![(video.to_path_buf(), Some(here)), (other.clone(), Some(there))];
        }
    }
    vec![(video.to_path_buf(), None)]
}

/// The episodes to compare a video with: others in its folder, nearest in
/// `playlist` first (the next one, the one before, the one after next…).
pub fn neighbours(playlist: &[PathBuf], index: usize, count: usize) -> Vec<PathBuf> {
    let Some(folder) = playlist.get(index).and_then(|p| p.parent()) else { return Vec::new() };
    let same: Vec<&PathBuf> = playlist.iter().filter(|p| p.parent() == Some(folder)).collect();
    let Some(at) = same.iter().position(|p| **p == playlist[index]) else { return Vec::new() };
    let mut found = Vec::new();
    for distance in 1..same.len() {
        for near in [at.checked_add(distance), at.checked_sub(distance)].into_iter().flatten() {
            if let Some(p) = same.get(near) {
                found.push((*p).clone());
            }
        }
        if found.len() >= count {
            break;
        }
    }
    found.truncate(count);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Noise that differs with `seed`, like the talking in an episode.
    fn noise(seconds: f64, seed: u64) -> Vec<f32> {
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..(seconds * RATE as f64) as usize)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state % 2000) as f32 / 1000.0 - 1.0
            })
            .map(|s| s * 0.3)
            .collect()
    }

    /// A tune: a new note every quarter second, the same every time.
    fn theme(seconds: f64, gain: f32) -> Vec<f32> {
        let notes = [262.0, 330.0, 392.0, 523.0, 440.0, 349.0, 294.0, 494.0, 587.0, 370.0, 415.0];
        (0..(seconds * RATE as f64) as usize)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let note = notes[(t * 4.0) as usize * 7 % notes.len()];
                let chord = (t * note * std::f32::consts::TAU).sin() + 0.5 * (t * note * 1.5 * std::f32::consts::TAU).sin();
                gain * 0.4 * chord
            })
            .collect()
    }

    fn episode(parts: &[Vec<f32>]) -> Vec<f32> {
        parts.concat()
    }

    #[test]
    fn fft_finds_a_tone() {
        let (mut re, mut im) = (vec![0f32; 64], vec![0f32; 64]);
        for (i, r) in re.iter_mut().enumerate() {
            *r = (std::f32::consts::TAU * 5.0 * i as f32 / 64.0).cos();
        }
        fft(&mut re, &mut im);
        let power: Vec<f32> = re.iter().zip(&im).map(|(r, i)| r * r + i * i).collect();
        let peak = (0..32).max_by(|&a, &b| power[a].total_cmp(&power[b])).unwrap();
        assert_eq!(peak, 5);
    }

    /// The theme is found in both episodes, wherever it starts, and
    /// however loud it is.
    #[test]
    fn finds_the_shared_theme() {
        let a = episode(&[noise(40.0, 1), theme(60.0, 1.0), noise(60.0, 2)]);
        // Quieter, starting part of a step later, with other sound over it.
        let over: Vec<f32> = theme(60.0, 0.5).iter().zip(noise(60.0, 13)).map(|(t, n)| t + 0.15 * n).collect();
        let b = episode(&[noise(83.017, 3), over, noise(40.0, 4)]);
        let ((a_start, a_end), (b_start, b_end)) = shared(&fingerprint(&a), &fingerprint(&b)).expect("found");
        let near = |x: f64, y: f64| (x - y).abs() < 1.0;
        assert!(near(a_start, 40.0) && near(a_end, 100.0), "in a: {a_start:.1}–{a_end:.1}");
        assert!(near(b_start, 83.0) && near(b_end, 143.0), "in b: {b_start:.1}–{b_end:.1}");
    }

    #[test]
    fn finds_nothing_in_different_sound() {
        let (a, b) = (noise(120.0, 5), noise(120.0, 6));
        assert_eq!(shared(&fingerprint(&a), &fingerprint(&b)), None);
        // Silence sounds the same everywhere, but isn't an opening.
        let quiet = vec![0f32; 60 * RATE as usize];
        let (a, b) = (episode(&[quiet.clone(), noise(30.0, 7)]), episode(&[quiet, noise(30.0, 8)]));
        assert_eq!(shared(&fingerprint(&a), &fingerprint(&b)), None);
        // Too short to be an opening: a jingle.
        let (a, b) = (episode(&[noise(30.0, 9), theme(8.0, 1.0), noise(30.0, 10)]), episode(&[noise(20.0, 11), theme(8.0, 1.0), noise(30.0, 12)]));
        assert_eq!(shared(&fingerprint(&a), &fingerprint(&b)), None);
    }

    #[test]
    fn picks_neighbours_in_the_same_folder() {
        let list: Vec<PathBuf> = ["A/1.mkv", "A/2.mkv", "B/1.mkv", "A/3.mkv", "A/4.mkv"].iter().map(PathBuf::from).collect();
        let names = |found: Vec<PathBuf>| found.iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect::<Vec<_>>();
        assert_eq!(names(neighbours(&list, 3, 3)), ["A/4.mkv", "A/2.mkv", "A/1.mkv"]);
        assert_eq!(names(neighbours(&list, 0, 2)), ["A/2.mkv", "A/3.mkv"]);
        assert!(neighbours(&list, 2, 3).is_empty(), "alone in its folder");
    }
}
