//! Loading and playing videos, and following mpv's state while they play.

use crate::*;

/// How long listening pauses after a seek.
const LISTEN_HOLD: Duration = Duration::from_secs(3);

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

/// Chapter titles that mark an opening, as lowercase words. A title matches
/// when it has these words in a row, ignoring numbers after a word, so "OP"
/// matches "OP2" and "Opening Credits", but not "Opera".
const OPENING_TITLES: &[&str] = &["opening", "op", "title sequence"];
/// Chapter titles that mark an opening unless another chapter is called
/// one of the `OPENING_TITLES`: some videos call the scene before the
/// opening "Intro".
const INTRO_TITLES: &[&str] = &["intro"];
/// Chapter titles that mark an ending (checked after the opening ones, so
/// "Opening Credits" is an opening).
const ENDING_TITLES: &[&str] = &["credits", "ending", "ed", "outro"];
/// Words in a chapter title that make it a scene around the credits, not
/// the credits: "Post-credits Scene", "Mid-credits".
const SCENE_TITLES: &[&str] = &["post", "mid", "after", "scene", "stinger"];
/// The longest a chapter found as the opening can be, in seconds.
const MAX_OPENING: f64 = 180.0;
/// Chapter titles after the ending that skipping it skips too.
const PREVIEW_TITLES: &[&str] = &["preview", "next time"];

/// Whether a chapter title matches one of `names`.
fn title_matches(title: &str, names: &[&str]) -> bool {
    let title = title.to_lowercase();
    let words: Vec<&str> = title
        .split(|c: char| !c.is_alphanumeric())
        .map(|word| word.trim_end_matches(|c: char| c.is_ascii_digit()))
        .filter(|word| !word.is_empty())
        .collect();
    names.iter().any(|name| {
        let name: Vec<&str> = name.split(' ').collect();
        words.windows(name.len()).any(|w| w == name.as_slice())
    })
}

/// The opening and ending of a video of `duration` seconds found from its
/// chapter titles, in seconds from its start. An opening must start in the
/// first half and an ending in the second half, so a chapter called
/// "Ending" in the middle of a movie isn't one; an opening in the second
/// half is the theme played at the end, so it's the ending.
pub(crate) fn chapter_skips(chapters: &[mpv::Chapter], duration: f64) -> Vec<(progress::Skip, f64, f64)> {
    let end_of = |i: usize| chapters.get(i + 1).map_or(duration, |c| c.time).min(duration);
    let first_half = |c: &mpv::Chapter| c.time < duration / 2.0;
    let opening = |c: &mpv::Chapter| title_matches(&c.title, OPENING_TITLES);
    let ending = |c: &mpv::Chapter| {
        if opening(c) { !first_half(c) } else { title_matches(&c.title, ENDING_TITLES) && !title_matches(&c.title, SCENE_TITLES) }
    };
    let mut skips = Vec::new();
    let found = chapters
        .iter()
        .position(|c| opening(c) && first_half(c))
        .or_else(|| chapters.iter().position(|c| title_matches(&c.title, INTRO_TITLES) && first_half(c)))
        // Longer than that, it's a scene.
        .filter(|&i| end_of(i) - chapters[i].time <= MAX_OPENING);
    if let Some(i) = found {
        skips.push((progress::Skip::Opening, chapters[i].time.max(0.0), end_of(i)));
    }
    if let Some(last) = chapters.iter().rposition(|c| ending(c) && !first_half(c)) {
        // From the first of a run of them ("Credits Start", "Credits End").
        let first = last - chapters[..last].iter().rev().take_while(|c| ending(c) && !first_half(c)).count();
        // A preview after it ends with it, but a scene after the credits
        // doesn't.
        let previews = chapters[last + 1..].iter().take_while(|c| title_matches(&c.title, PREVIEW_TITLES)).count();
        skips.push((progress::Skip::Ending, chapters[first].time, end_of(last + previews)));
    }
    skips.retain(|&(_, start, end)| duration > 0.0 && end > start);
    skips
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
        self.ended = None;
        self.save_current_progress();
        if self.mpv.is_err() {
            return;
        }
        let Some(path) = self.playlist.get(index).cloned() else { return };
        let name = self.video_name(&path);
        if !path.is_file() {
            return self.warn(format!("{name} was not found (moved or deleted?)"));
        }
        if self.store.is_movie(&path) {
            self.store.add_recent_video(&path);
        }
        let start = if resume { self.store.resume_position(&path) } else { None };
        self.apply_folder_settings(&path);
        let Ok(mpv) = &self.mpv else { return };
        mpv.set("pause", if paused { "yes" } else { "no" });
        mpv.load_file(&path, start.unwrap_or(0.0), LOAD_TAG);
        self.loads.asked();
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
        self.update_window_title(ctx);
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
        self.update_window_title(ctx);
    }

    /// Names the playing video in the window's title, once its name is known
    /// or changes (when its title is read from the file).
    pub(crate) fn update_window_title(&mut self, ctx: &egui::Context) {
        let title = match self.current_path() {
            Some(path) => format!("{} — {APP_NAME}", self.video_name(path)),
            None => APP_NAME.to_string(),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    pub(crate) fn file_loaded(&mut self) {
        self.open_for_preview();
        let Ok(mpv) = &self.mpv else { return };
        self.loaded = true;
        // Restore this video's track choices. The subtitle and audio delays
        // are global in mpv, so they are always set: to the folder's if
        // nothing was chosen for the video, else back to 0.
        let prefs = match self.current_path() {
            Some(path) => self.store.tracks(path).cloned().unwrap_or_else(|| self.folder_track_prefs(path)),
            None => Default::default(),
        };
        mpv.set("sub-delay", &prefs.sub_delay.to_string());
        mpv.set("sub-speed", &prefs.sub_speed.unwrap_or(1.0).to_string());
        mpv.set("audio-delay", &prefs.audio_delay.to_string());
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
            "audio-delay" => p.audio_delay = value.as_f64().unwrap_or(0.0),
            "chapter-list" => self.chapters = mpv::Chapter::list(&value),
            // Only once the video is open, so a late report about the one
            // before it can't name this one. A video without a title is left
            // to the duration probe, which knows for sure.
            "metadata/by-key/title" if self.loaded => {
                if let (Some(title), Some(path)) = (value.as_str(), self.current_path().map(Path::to_path_buf)) {
                    self.store.set_title(&path, Some(title.to_string()));
                    self.names.clear();
                }
            }
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
        if self.tracks_changed_by_user && matches!(name, "track-list" | "sub-delay" | "sub-speed" | "audio-delay") {
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
        let Playback { sub_delay, sub_speed, audio_delay, .. } = self.playback;
        let prefs = self.store.tracks_mut(&path);
        prefs.audio_delay = audio_delay;
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
                    self.last_seek = Instant::now();
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
                mpv::Event::CommandDone { tag: tag @ (SCREENSHOT_TAG | SCREENSHOT_COPY_TAG), ok, result } => {
                    self.screenshot_done(ctx, tag == SCREENSHOT_COPY_TAG, ok, &result);
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
                    if eof {
                        self.played_to_end(ctx);
                    }
                }
            }
        }
    }

    /// Marks the playing video watched and goes on to the next one's
    /// countdown, or the end screen.
    pub(crate) fn played_to_end(&mut self, ctx: &egui::Context) {
        let Some(index) = self.current else { return };
        if let Some(path) = self.playlist.get(index).cloned() {
            self.store.finish(&path);
        }
        self.finish(ctx);
        let autoplay = self.playlist.get(index).is_some_and(|p| self.store.autoplay_after(p));
        if index + 1 < self.playlist.len() && autoplay {
            self.up_next = Some(UpNext { index: index + 1, at: Instant::now() + AUTOPLAY_DELAY });
        } else {
            self.ended = Some(index);
        }
        self.store.save();
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

    /// Sets what a video's folder settings change in mpv, before it opens:
    /// the languages, evening out the volume and the speed. A folder's speed
    /// goes back to normal for a video whose folder doesn't set one.
    pub(crate) fn apply_folder_settings(&mut self, video: &Path) {
        let Ok(mpv) = &self.mpv else { return };
        let settings = self.store.settings_for(Some(video));
        mpv.set("slang", &settings.sub_languages);
        mpv.set("alang", &settings.audio_languages);
        mpv.set("af", if settings.level_volume { mpv::VOLUME_LEVELER } else { "" });
        let speed = self.store.folder_settings(video).and_then(|(_, f)| f.speed);
        if let Some(speed) = speed.or(self.folder_speed.map(|_| 1.0)) {
            mpv.set("speed", &speed.to_string());
        }
        self.folder_speed = speed;
    }

    /// The subtitle timing and audio delay a video starts with when none
    /// were chosen for it: its folder's.
    pub(crate) fn folder_track_prefs(&self, video: &Path) -> progress::TrackPrefs {
        let folder = self.store.folder_settings(video).map(|(_, f)| f);
        progress::TrackPrefs {
            sub_delay: folder.and_then(|f| f.sub_delay).unwrap_or(0.0),
            sub_speed: folder.and_then(|f| f.sub_speed),
            audio_delay: folder.and_then(|f| f.audio_delay).unwrap_or(0.0),
            ..Default::default()
        }
    }

    /// The opening and ending of the loaded video, in seconds from its
    /// start: from its chapter titles, or else the opening heard by
    /// listening.
    pub(crate) fn skips(&self) -> Vec<(progress::Skip, f64, f64)> {
        if !self.loaded {
            return Vec::new();
        }
        let found = chapter_skips(&self.chapters, self.playback.duration);
        let heard = self.heard_opening().map(|(start, end)| (progress::Skip::Opening, start, end));
        [progress::Skip::Opening, progress::Skip::Ending]
            .into_iter()
            .filter_map(|kind| found.iter().chain(&heard).find(|s| s.0 == kind).copied())
            .collect()
    }

    /// The opening of the loaded video heard by listening, if listening is on.
    pub(crate) fn heard_opening(&self) -> Option<(f64, f64)> {
        let path = self.current_path().filter(|_| self.store.settings.listen_for_openings)?;
        let (start, end) = self.store.heard_opening(path)??;
        // Within the first half, as for an opening found from chapters.
        (end > start && start < self.playback.duration / 2.0 && end <= self.playback.duration).then_some((start, end))
    }

    /// Asks for the playing episode's opening to be found by listening, if
    /// its chapters don't have one, and it wasn't
    /// listened to yet. It's compared with other episodes in its folder.
    /// It starts as soon as the episode is loaded. Listening reads from the
    /// same disk as playback, so it pauses for a moment after each seek.
    pub(crate) fn listen_for_opening(&mut self, ctx: &egui::Context) {
        let settling = self.last_seek.elapsed();
        self.listener.hold(settling < LISTEN_HOLD);
        if settling < LISTEN_HOLD {
            // To go on once it has settled, even while paused.
            ctx.request_repaint_after(LISTEN_HOLD - settling);
        }
        // Its chapters come in with its length.
        if !self.loaded || self.playback.duration <= 0.0 || !self.store.settings.listen_for_openings {
            return;
        }
        let Some(index) = self.current else { return };
        let path = self.playlist[index].clone();
        if self.listen_asked.as_ref() == Some(&path) {
            return;
        }
        self.listen_asked = Some(path.clone());
        let has_opening = self.skips().iter().any(|s| s.0 == progress::Skip::Opening);
        if has_opening || self.store.heard_opening(&path).is_some() || self.store.is_movie(&path) {
            return;
        }
        let others = player::listen::neighbours(&self.playlist, index, 3);
        if !others.is_empty() {
            self.listener.ask(&path, others);
        }
    }

    /// Takes in openings heard by listening.
    pub(crate) fn receive_heard(&mut self) {
        while let Ok(heard) = self.listener.heard.try_recv() {
            // First the video asked for, then the one it shared its opening
            // with, which keeps one it already had.
            for (i, (path, opening)) in heard.into_iter().enumerate() {
                let known = self.store.heard_opening(&path).flatten().is_some();
                if i == 0 || !known {
                    self.store.set_heard_opening(&path, opening);
                }
            }
        }
    }

    /// The opening or ending playing now, which the Skip button skips.
    pub(crate) fn skip_now(&self) -> Option<(progress::Skip, f64, f64)> {
        let position = self.playback.position;
        self.skips().into_iter().find(|&(_, start, end)| position >= start && position < end - 0.5)
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

    fn chapters(list: &[(f64, &str)]) -> Vec<crate::player::mpv::Chapter> {
        list.iter().map(|&(time, title)| crate::player::mpv::Chapter { title: title.into(), time }).collect()
    }

    #[test]
    fn matches_chapter_titles() {
        for title in ["OP", "OP2", "Opening Credits", "Main Title Sequence"] {
            assert!(title_matches(title, OPENING_TITLES), "{title}");
        }
        for title in ["Introduction", "Opera", "Topology", "Chapter 1"] {
            assert!(!title_matches(title, OPENING_TITLES), "{title}");
        }
        assert!(title_matches("Intro", INTRO_TITLES) && !title_matches("Introduction", INTRO_TITLES));
        for title in ["ED", "Ending", "End Credits", "Outro"] {
            assert!(title_matches(title, ENDING_TITLES), "{title}");
        }
        for title in ["Edward", "The End", "Credit Card"] {
            assert!(!title_matches(title, ENDING_TITLES), "{title}");
        }
    }

    #[test]
    fn finds_skips_from_chapters() {
        use progress::Skip::{Ending, Opening};
        // Chapter layouts from real shows.
        let fmab = chapters(&[(0.0, "Prologue"), (58.8, "Opening"), (148.9, "Part A"), (655.2, "Part B"), (1347.6, "Ending"), (1437.4, "Preview")]);
        assert_eq!(chapter_skips(&fmab, 1467.0), [(Opening, 58.8, 148.9), (Ending, 1347.6, 1467.0)]);
        let frieren = chapters(&[(0.0, "Opening"), (90.0, "Episode"), (1460.0, "Credits Start"), (1550.0, "Credits End"), (1560.0, "Preview End")]);
        assert_eq!(chapter_skips(&frieren, 1560.0), [(Opening, 0.0, 90.0), (Ending, 1460.0, 1560.0)]);
        let tlou = chapters(&[(207.2, "Intro"), (274.3, "Chapter 01"), (4776.0, "Credits")]);
        assert_eq!(chapter_skips(&tlou, 4847.0), [(Opening, 207.2, 274.3), (Ending, 4776.0, 4847.0)]);
        // The scene before the opening is called Intro.
        let eminence = chapters(&[(0.0, "Intro"), (43.0, "OP"), (133.0, "Part A"), (701.0, "Part B"), (1236.5, "Epilogue")]);
        assert_eq!(chapter_skips(&eminence, 1422.0), [(Opening, 43.0, 133.0)]);
        // A long Intro is a scene, and the opening played at the end is the
        // ending.
        let first = chapters(&[(0.0, "Intro"), (212.0, "Part A"), (762.0, "Part B"), (1330.0, "OP")]);
        assert_eq!(chapter_skips(&first, 1421.0), [(Ending, 1330.0, 1421.0)]);
        // A scene after the credits isn't skipped with them.
        let movie = chapters(&[(0.0, "Chapter 1"), (6000.0, "End Credits"), (6500.0, "Post-credits Scene")]);
        assert_eq!(chapter_skips(&movie, 6600.0), [(Ending, 6000.0, 6500.0)]);
        // Too early to be the ending.
        let odd = chapters(&[(0.0, "Start"), (100.0, "Ending"), (300.0, "Rest")]);
        assert_eq!(chapter_skips(&odd, 400.0), []);
        assert_eq!(chapter_skips(&[], 400.0), []);
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
