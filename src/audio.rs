//! Audio analysis for the visualizer.
//!
//! libmpv doesn't expose decoded samples, so mpv renders a tiny "data image"
//! through an FFmpeg filter graph: spectrum bars on top, the waveform below.
//! The frame arrives through the normal render path and is decoded here.
//! Because mpv presents it like a video frame, it stays in sync with playback.

use eframe::egui::ColorImage;

pub const BANDS: usize = 64;
const PANE_H: usize = 48;

/// Size the data image must be rendered at.
pub const FRAME_SIZE: [u32; 2] = [BANDS as u32, (PANE_H * 2) as u32];

/// Value for mpv's `lavfi-complex` while music plays.
pub const ANALYSIS_GRAPH: &str = "[aid1]asplit[ao][a];[a]asplit[s][w];\
    [s]showfreqs=s=64x48:mode=bar:ascale=log:fscale=log:win_size=2048:colors=white,fps=30[sv];\
    [w]showwaves=s=64x48:mode=point:rate=30:colors=white[wv];\
    [sv][wv]vstack[vo]";

/// Raw values decoded from one data frame.
pub struct Frame {
    /// Bar heights, 0..1, low to high frequencies.
    pub spectrum: [f32; BANDS],
    /// Waveform samples, -1..1.
    pub wave: [f32; BANDS],
}

pub fn decode(image: &ColorImage) -> Option<Frame> {
    let [w, h] = image.size;
    if w != BANDS || h != PANE_H * 2 {
        return None;
    }
    let lit = |x: usize, y: usize| {
        let p = image.pixels[y * w + x];
        (p.r() as u32 + p.g() as u32 + p.b() as u32) > 120
    };
    let mut spectrum = [0.0; BANDS];
    let mut wave = [0.0; BANDS];
    for x in 0..BANDS {
        let bar = (0..PANE_H).filter(|&y| lit(x, y)).count();
        spectrum[x] = bar as f32 / PANE_H as f32;
        let rows: Vec<usize> = (PANE_H..PANE_H * 2).filter(|&y| lit(x, y)).collect();
        if !rows.is_empty() {
            let mean = rows.iter().sum::<usize>() as f32 / rows.len() as f32 - PANE_H as f32;
            wave[x] = 1.0 - 2.0 * mean / (PANE_H - 1) as f32;
        }
    }
    Some(Frame { spectrum, wave })
}

/// Smoothed, normalized audio features for one rendered frame.
#[derive(Clone, Copy)]
pub struct Features {
    pub spectrum: [f32; BANDS],
    pub wave: [f32; BANDS],
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub level: f32,
    /// 1 on a detected beat, decaying to 0.
    pub beat: f32,
    /// Animation clock that runs faster when the music is louder.
    pub drive: f32,
}

/// Turns raw data frames into stable, auto-gained features.
pub struct Analyzer {
    target_spectrum: [f32; BANDS],
    target_wave: [f32; BANDS],
    out: Features,
    spectrum_peak: f32,
    wave_peak: f32,
    bass_avg: f32,
    since_beat: f32,
    since_frame: f32,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self {
            target_spectrum: [0.0; BANDS],
            target_wave: [0.0; BANDS],
            out: Features {
                spectrum: [0.0; BANDS],
                wave: [0.0; BANDS],
                bass: 0.0,
                mid: 0.0,
                treble: 0.0,
                level: 0.0,
                beat: 0.0,
                drive: 0.0,
            },
            spectrum_peak: 0.3,
            wave_peak: 0.1,
            bass_avg: 0.0,
            since_beat: 0.0,
            since_frame: 0.0,
        }
    }
}

/// Bars this short are the analyzer's noise floor.
const SPECTRUM_FLOOR: f32 = 0.04;

impl Analyzer {
    pub fn push(&mut self, frame: Frame) {
        let max = frame.spectrum.iter().cloned().fold(0.0, f32::max);
        self.spectrum_peak = self.spectrum_peak.max(max);
        let gain = 1.0 / (self.spectrum_peak - SPECTRUM_FLOOR).max(0.15);
        for (t, raw) in self.target_spectrum.iter_mut().zip(frame.spectrum) {
            *t = ((raw - SPECTRUM_FLOOR) * gain).clamp(0.0, 1.0);
        }
        let wave_max = frame.wave.iter().map(|v| v.abs()).fold(0.0, f32::max);
        self.wave_peak = self.wave_peak.max(wave_max);
        let gain = 1.0 / self.wave_peak.max(0.05);
        for (t, raw) in self.target_wave.iter_mut().zip(frame.wave) {
            *t = (raw * gain).clamp(-1.0, 1.0);
        }
        self.since_frame = 0.0;
    }

    /// Advances smoothing by `dt` seconds and returns the current features.
    pub fn tick(&mut self, dt: f32) -> Features {
        let dt = dt.clamp(0.0, 0.1);
        self.since_frame += dt;
        self.since_beat += dt;
        // Slowly forget loud passages so quiet ones still move.
        self.spectrum_peak = (self.spectrum_peak * (1.0 - 0.08 * dt)).max(0.2);
        self.wave_peak = (self.wave_peak * (1.0 - 0.08 * dt)).max(0.05);
        // Without new frames (paused, buffering), let everything settle.
        if self.since_frame > 0.25 {
            let fade = (-dt * 3.0).exp();
            self.target_spectrum.iter_mut().for_each(|v| *v *= fade);
            self.target_wave.iter_mut().for_each(|v| *v *= fade);
        }

        let approach = |cur: &mut f32, target: f32, attack: f32, release: f32| {
            let rate = if target > *cur { attack } else { release };
            *cur += (target - *cur) * (1.0 - (-dt * rate).exp());
        };
        let o = &mut self.out;
        for i in 0..BANDS {
            approach(&mut o.spectrum[i], self.target_spectrum[i], 30.0, 7.0);
            approach(&mut o.wave[i], self.target_wave[i], 40.0, 40.0);
        }
        let avg = |range: std::ops::Range<usize>| {
            let n = range.len() as f32;
            range.map(|i| self.target_spectrum[i]).sum::<f32>() / n
        };
        let (bass, mid, treble) = (avg(0..8), avg(8..28), avg(28..BANDS));
        approach(&mut o.bass, bass, 25.0, 6.0);
        approach(&mut o.mid, mid, 20.0, 5.0);
        approach(&mut o.treble, treble, 20.0, 5.0);
        approach(&mut o.level, (bass + mid + treble) / 3.0, 10.0, 2.0);

        // Beat: bass jumps clearly above its recent average.
        if bass > self.bass_avg * 1.3 + 0.08 && self.since_beat > 0.22 {
            o.beat = 1.0;
            self.since_beat = 0.0;
        }
        o.beat *= (-dt * 5.0).exp();
        self.bass_avg += (bass - self.bass_avg) * (1.0 - (-dt * 1.5).exp());

        o.drive += dt * (0.35 + o.level * 1.6 + o.beat * 0.8);
        *o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(level: f32) -> Frame {
        let mut spectrum = [SPECTRUM_FLOOR; BANDS];
        spectrum[..8].iter_mut().for_each(|v| *v = level);
        Frame { spectrum, wave: [0.0; BANDS] }
    }

    #[test]
    fn detects_beats_and_settles() {
        let mut a = Analyzer::default();
        for _ in 0..60 {
            a.push(frame(0.2));
            a.tick(1.0 / 60.0);
        }
        let quiet = a.tick(1.0 / 60.0);
        assert!(quiet.beat < 0.2, "steady input is not a beat");
        a.push(frame(0.9));
        let hit = a.tick(1.0 / 60.0);
        assert!(hit.beat > 0.9, "bass jump is a beat");
        assert!(hit.bass > quiet.bass);
        // No frames for a while: features fade out.
        let mut f = hit;
        for _ in 0..180 {
            f = a.tick(1.0 / 60.0);
        }
        assert!(f.bass < 0.05 && f.beat < 0.01);
    }
}
