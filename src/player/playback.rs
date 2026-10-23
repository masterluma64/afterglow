//! Loading and playing videos, and following mpv's state while they play.

use crate::*;

/// The chapter playing at `time`.
pub(crate) fn chapter_at(chapters: &[mpv::Chapter], time: f64) -> Option<usize> {
    chapters.iter().rposition(|c| c.time <= time + 0.01)
}

/// The chapter to jump to from `position`: the next one, or going back, the
/// start of the current one (or the previous one if it just started).
pub(crate) fn chapter_target(chapters: &[mpv::Chapter], position: f64, forward: bool) -> Option<usize> {
    let current = chapter_at(chapters, position);
    if forward {
        let next = current.map_or(0, |i| i + 1);
        return (next < chapters.len()).then_some(next);
    }
    match current {
        Some(i) if position - chapters[i].time > CHAPTER_BACK_THRESHOLD => Some(i),
        Some(i) => Some(i.saturating_sub(1)),
        None => (!chapters.is_empty()).then_some(0),
    }
}

/// The next preset speed above (or below) `speed`, staying within the presets.
pub(crate) fn step_speed(speed: f64, up: bool) -> f64 {
    let next = if up {
        SPEEDS.iter().copied().find(|&s| s > speed + 0.001)
    } else {
        SPEEDS.iter().rev().copied().find(|&s| s < speed - 0.001)
    };
    next.unwrap_or(if up { SPEEDS[SPEEDS.len() - 1] } else { SPEEDS[0] })
}

/// Tells whether an end-of-file event is about the video asked for last.
/// mpv can report the end of the previous video after a new one was asked
/// for (when it had just played to its end, say); that must not count as
/// the new one finishing. Each file mpv opens gets a playlist entry id,
/// which the reply to opening it and its end both carry.
#[derive(Default)]
pub(crate) struct Loads {
    /// Videos asked for whose reply hasn't come yet.
    waiting: u32,
    /// The entry id of the video asked for last, once its reply is in.
    current: Option<i64>,
}

impl Loads {
    pub(crate) fn asked(&mut self) {
        self.waiting += 1;
        self.current = None;
    }

    /// The reply to opening a video: its entry id, or `None` if it failed.
    pub(crate) fn answered(&mut self, entry: Option<i64>) {
        self.waiting = self.waiting.saturating_sub(1);
        // Replies come in order, so only the last one is the current video.
        if self.waiting == 0 {
            self.current = entry;
        }
    }

    pub(crate) fn is_current(&self, entry: i64) -> bool {
        self.waiting == 0 && self.current == Some(entry)
    }
}

impl App {
    /// The video to offer when nothing is playing: the last one played if it is
    /// unfinished, otherwise the next unwatched one after it (wrapping around).
    /// `None` when everything is watched.
    pub(crate) fn continue_target(&self) -> Option<usize> {
        let watched = |i: usize| self.store.get(&self.playlist[i]).watched;
        let last = self.store.current.as_ref().and_then(|c| self.playlist.iter().position(|p| p == c));
        let start = match last {
            Some(i) if !watched(i) || self.store.resume_position(&self.playlist[i]).is_some() => return Some(i),
            Some(i) => i + 1,
            None => 0,
        };
        let n = self.playlist.len();
        (0..n).map(|k| (start + k) % n).find(|&i| !watched(i))
    }

    pub(crate) fn save_current_progress(&mut self) {
        if !self.loaded || self.playback.duration <= 0.0 {
            return;
        }
        if let Some(path) = self.current_path().map(Path::to_path_buf) {
            let (position, duration) = (self.playback.position, self.playback.duration);
            if self.held_unwatched {
                self.store.update_position_unwatched(&path, position, duration);
            } else {
                self.store.update_position(&path, position, duration);
            }
        }
    }

    pub(crate) fn play(&mut self, index: usize, ctx: &egui::Context) {
        self.load(index, false, true, ctx);
    }

    /// Plays a playlist video from its start, whatever was saved for it (the
    /// next video after one finishes).
    pub(crate) fn play_from_start(&mut self, index: usize, ctx: &egui::Context) {
        self.load(index, false, false, ctx);
    }

    /// Loads a playlist video at its saved position (with `resume`;
    /// otherwise at its start). With `paused`, it only shows that frame, ready
    /// to continue.
    pub(crate) fn load(&mut self, index: usize, paused: bool, resume: bool, ctx: &egui::Context) {
        self.up_next = None;
        self.save_current_progress();
        let Ok(mpv) = &self.mpv else { return };
        let Some(path) = self.playlist.get(index).cloned() else { return };
        let name = display_name(&path);
        if !path.is_file() {
            return self.warn(format!("{name} was not found (moved or deleted?)"));
        }
        let start = if resume { self.store.resume_position(&path) } else { None };
        mpv.set("pause", if paused { "yes" } else { "no" });
        mpv.load_file(&path, start.unwrap_or(0.0), LOAD_TAG);
        self.loads.asked();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{name} — {APP_NAME}")));
        // Drop the previous frame so it never shows under the new video.
        mpv.hide_frames();
        self.chapters.clear();
        // Until mpv reports the new video's values, don't let the previous
        // one's be saved as this one's progress.
        self.playback.position = start.unwrap_or(0.0);
        self.playback.duration = 0.0;
        self.restoring = None;
        self.tracks_changed_by_user = false;
        self.watch_mark = None;
        self.held_unwatched = false;
        self.current = Some(index);
        self.loaded = false;
        self.store.set_current(Some(&path));
        self.store.save();
        match (paused, start) {
            (true, Some(pos)) => self.notify(format!("{name}  ·  paused at {}", format_time(pos))),
            (true, None) => self.notify(format!("{name}  ·  press Space to play")),
            (false, Some(pos)) => self.notify(format!("{name}  ·  resuming at {}", format_time(pos))),
            (false, None) => self.notify(name),
        }
    }

    pub(crate) fn stop(&mut self, ctx: &egui::Context) {
        self.save_current_progress();
        if self.current.is_some()
            && let Ok(mpv) = &self.mpv
        {
            mpv.send(&["stop"]);
        }
        self.finish(ctx);
    }

    /// Clears the playing state once nothing is playing any more.
    pub(crate) fn finish(&mut self, ctx: &egui::Context) {
        self.current = None;
        self.loaded = false;
        self.chapters.clear();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(APP_NAME.into()));
    }

    pub(crate) fn file_loaded(&mut self) {
        self.open_for_preview();
        let Ok(mpv) = &self.mpv else { return };
        self.loaded = true;
        // Restore this video's track choices. The subtitle delay is global in
        // mpv, so it is always set, back to 0 if nothing was chosen.
        let prefs = self.current_path().and_then(|p| self.store.tracks(p)).cloned().unwrap_or_default();
        mpv.set("sub-delay", &prefs.sub_delay.to_string());
        mpv.set("sub-speed", &prefs.sub_speed.unwrap_or(1.0).to_string());
        // Subtitle files load in the background when sent without waiting, so
        // the saved tracks are selected once they have all been added.
        let files: Vec<PathBuf> = prefs.sub_files.iter().filter(|f| f.is_file()).cloned().collect();
        for file in &files {
            mpv.send_tagged(SUB_RESTORE_TAG, &["sub-add", &file.to_string_lossy(), "auto"]);
        }
        self.restoring = Some((files.len(), prefs));
        self.select_saved_tracks();
    }

    /// Selects the saved audio and subtitle tracks once the saved subtitle
    /// files are loaded.
    pub(crate) fn select_saved_tracks(&mut self) {
        let (Ok(mpv), Some((0, prefs))) = (&self.mpv, &self.restoring) else { return };
        if let Some(aid) = &prefs.audio {
            mpv.set("aid", aid);
        }
        if let Some(sid) = &prefs.subtitle {
            mpv.set("sid", sid);
        }
        self.restoring = None;
    }

    /// Opens the playing video in the previewer too, so the first hover over
    /// the seek bar only has to seek.
    pub(crate) fn open_for_preview(&mut self) {
        let Some(path) = self.current_path().map(Path::to_path_buf) else { return };
        if let Some(Ok(thumbnails)) = &mut self.thumbnails {
            thumbnails.request(&path, self.playback.position, 1.0);
        }
    }

    /// Takes in a changed property that was asked for with `observe`.
    pub(crate) fn property_changed(&mut self, name: &str, value: mpv::Node) {
        let p = &mut self.playback;
        match name {
            // Unavailable between videos: keep the last known position.
            "time-pos" => p.position = value.as_f64().unwrap_or(p.position),
            "duration" => p.duration = value.as_f64().unwrap_or(0.0),
            "pause" => p.paused = value.as_bool().unwrap_or(p.paused),
            "mute" => p.muted = value.as_bool().unwrap_or(p.muted),
            "volume" => p.volume = value.as_f64().unwrap_or(p.volume),
            "speed" => p.speed = value.as_f64().unwrap_or(1.0),
            "sub-delay" => p.sub_delay = value.as_f64().unwrap_or(0.0),
            "sub-speed" => p.sub_speed = value.as_f64().unwrap_or(1.0),
            "chapter-list" => self.chapters = mpv::Chapter::list(&value),
            "track-list" => {
                self.tracks = mpv::Track::list(&value);
                if let Some(kind) = self.announce_track.take() {
                    let what = if kind == "sub" { "Subtitles" } else { "Audio" };
                    let text = match self.tracks.iter().find(|t| t.kind == kind && t.selected) {
                        Some(track) => format!("{what}: {}", track.label),
                        None => format!("{what} off"),
                    };
                    self.notify(text);
                }
            }
            _ => {}
        }
        if self.tracks_changed_by_user && matches!(name, "track-list" | "sub-delay" | "sub-speed") {
            self.remember_tracks();
        }
    }

    /// The tracks of a kind ("audio" or "sub") in the loaded video.
    pub(crate) fn tracks(&self, kind: &str) -> impl Iterator<Item = &mpv::Track> {
        self.tracks.iter().filter(move |t| t.kind == kind)
    }

    /// Saves the playing video's current track choices.
    pub(crate) fn remember_tracks(&mut self) {
        let Some(path) = self.current_path().map(Path::to_path_buf) else { return };
        if !self.loaded {
            return;
        }
        // In mpv's terms: a track id, or "no" for none.
        let selected = |kind| self.tracks(kind).find(|t| t.selected).map_or("no".to_string(), |t| t.id.to_string());
        let (audio, subtitle) = (selected("audio"), selected("sub"));
        let Playback { sub_delay, sub_speed, .. } = self.playback;
        let prefs = self.store.tracks_mut(&path);
        prefs.audio = Some(audio);
        prefs.subtitle = Some(subtitle);
        prefs.sub_delay = sub_delay;
        prefs.sub_speed = ((sub_speed - 1.0).abs() > 1e-6).then_some(sub_speed);
    }

    pub(crate) fn handle_mpv_events(&mut self, ctx: &egui::Context) {
        let Ok(mpv) = &self.mpv else { return };
        for event in mpv.poll_events() {
            match event {
                mpv::Event::FileLoaded => self.file_loaded(),
                mpv::Event::PlaybackRestart => {
                    if let Ok(mpv) = &self.mpv {
                        mpv.show_frames();
                    }
                }
                mpv::Event::Property { name, value } => self.property_changed(&name, value),
                mpv::Event::CommandDone { tag: LOAD_TAG, ok, result } => {
                    let entry = result.get("playlist_entry_id").and_then(mpv::Node::as_i64);
                    self.loads.answered(entry.filter(|_| ok));
                }
                mpv::Event::CommandDone { tag: SUB_STEP_TAG, ok, .. } => {
                    if let Some(which) = self.announce_sub_step.take() {
                        self.notify(if ok { format!("{which} line shown now") } else { "No subtitle line to sync to".into() });
                    }
                }
                mpv::Event::CommandDone { tag: SUB_RESTORE_TAG, .. } => {
                    if let Some((left, _)) = &mut self.restoring {
                        *left = left.saturating_sub(1);
                    }
                    self.select_saved_tracks();
                }
                mpv::Event::CommandDone { .. } => {}
                // The end of a video played before the one asked for last.
                mpv::Event::EndFile { entry, .. } if !self.loads.is_current(entry) => {}
                mpv::Event::EndFile { eof, error, .. } => {
                    self.loaded = false;
                    if error {
                        self.warn("Playback failed: the file could not be decoded.");
                        self.finish(ctx);
                        continue;
                    }
                    if !eof {
                        continue;
                    }
                    let Some(index) = self.current else { continue };
                    if let Some(path) = self.playlist.get(index).cloned() {
                        self.store.finish(&path);
                    }
                    self.finish(ctx);
                    if index + 1 >= self.playlist.len() {
                        self.notify("End of the playlist");
                    } else if self.store.autoplay_next {
                        self.up_next = Some(UpNext { index: index + 1, at: Instant::now() + AUTOPLAY_DELAY });
                    }
                    self.store.save();
                }
            }
        }
    }

    /// Records the playing position (`playback` itself is kept up to date by
    /// mpv's property events) and saves now and then.
    pub(crate) fn sample_playback(&mut self) {
        if self.loaded {
            self.save_current_progress();
            self.count_watching();
        }
        if self.last_save.elapsed() >= SAVE_INTERVAL {
            self.store.save();
            self.last_save = Instant::now();
        }
        // Without a console, a failing save would otherwise go unnoticed.
        if let Some(problem) = self.store.save_problem() {
            self.warn(problem);
        }
    }

    /// Adds the video played since the last sample to the stats. Seeking
    /// doesn't count: the position may only have moved forward as far as
    /// playback could have taken it in the time that passed. That also covers
    /// frames the window didn't draw while minimized.
    fn count_watching(&mut self) {
        let (position, now) = (self.playback.position, Instant::now());
        if let Some((last, at)) = self.watch_mark
            && !self.playback.paused
        {
            let moved = position - last;
            let possible = now.duration_since(at).as_secs_f64() * self.playback.speed.max(1.0) * 1.1 + 0.5;
            if moved > 0.0
                && moved <= possible
                && let Some(path) = self.current_path().map(Path::to_path_buf)
            {
                self.store.record_watching(&path, moved);
            }
        }
        self.watch_mark = Some((position, now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_video_asked_for_last_can_end() {
        let mut loads = Loads::default();
        assert!(!loads.is_current(1), "nothing asked for yet");
        loads.asked();
        loads.answered(Some(1));
        assert!(loads.is_current(1));
        // Next, Next: the first reply is for a video already replaced.
        loads.asked();
        loads.asked();
        assert!(!loads.is_current(1), "the end of the old video, reported late");
        loads.answered(Some(2));
        assert!(!loads.is_current(2), "the second video was skipped");
        loads.answered(Some(3));
        assert!(loads.is_current(3) && !loads.is_current(2));
        loads.asked();
        loads.answered(None);
        assert!(!loads.is_current(3), "after a failed open, nothing is playing");
    }

    #[test]
    fn finds_chapter_targets() {
        let chapters: Vec<_> = [0.0, 60.0, 120.0]
            .into_iter()
            .map(|time| crate::player::mpv::Chapter { title: String::new(), time })
            .collect();
        assert_eq!(chapter_at(&chapters, 59.0), Some(0));
        assert_eq!(chapter_at(&chapters, 60.0), Some(1));
        assert_eq!(chapter_target(&chapters, 30.0, true), Some(1));
        assert_eq!(chapter_target(&chapters, 130.0, true), None, "no chapter after the last");
        assert_eq!(chapter_target(&chapters, 90.0, false), Some(1), "back to the start of this one");
        assert_eq!(chapter_target(&chapters, 61.0, false), Some(0), "just started: the previous one");
        assert_eq!(chapter_target(&chapters, 1.0, false), Some(0));
        assert_eq!(chapter_target(&[], 1.0, false), None);
    }

    #[test]
    fn steps_through_speeds() {
        assert_eq!(format_speed(1.0), "1×");
        assert_eq!(format_speed(1.25), "1.25×");
        assert_eq!(format_speed(0.5), "0.5×");
        assert_eq!(step_speed(1.0, true), 1.25);
        assert_eq!(step_speed(1.0, false), 0.75);
        assert_eq!(step_speed(1.1, false), 1.0, "off-preset speeds snap to the next preset");
        assert_eq!(step_speed(3.0, true), 3.0);
        assert_eq!(step_speed(0.25, false), 0.25);
    }
}
