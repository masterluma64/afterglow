//! Watching statistics. They are kept apart from the per-video progress,
//! which is forgotten once a video's file is deleted, so they never shrink:
//! deleting a series after watching it keeps its hours here.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

/// A day, as days since 1970-01-01 (in local time).
pub type Day = i64;

/// Time watched and videos finished.
#[derive(Clone, Copy, Default, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Tally {
    /// Seconds of video played (seeking doesn't count; at 2× speed a minute of
    /// video counts as a minute).
    pub seconds: f64,
    pub finished: u32,
}

impl Tally {
    fn active(&self) -> bool {
        self.seconds >= 1.0 || self.finished > 0
    }
}

/// What was watched of one movie, and when last.
#[derive(Clone, Copy, Default, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Movie {
    pub seconds: f64,
    pub finished: u32,
    pub last_day: Day,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Stats {
    /// Everything, including `before`.
    pub total: Tally,
    /// Estimated once, when stats started being kept, from the progress saved
    /// until then (which says nothing about when).
    pub before: Tally,
    /// Per day, keyed "2026-10-22" so the file reads well and sorts by date.
    pub days: BTreeMap<String, Tally>,
    /// Per series: the full path of the folder the videos were in. Stats
    /// kept before November 2026 have only the folder's name.
    pub series: HashMap<String, Tally>,
    /// Names given to series in the stats, by the same key as `series`.
    pub names: HashMap<String, String>,
    /// Per movie (a video opened by itself): its title, or else its file name.
    pub movies: HashMap<String, Movie>,
    /// Whether `before` was estimated (it is, once).
    pub seeded: bool,
}

impl Stats {
    pub fn add_time(&mut self, series: &str, seconds: f64, today: Day) {
        self.total.seconds += seconds;
        self.days.entry(date_key(today)).or_default().seconds += seconds;
        self.series.entry(series.to_string()).or_default().seconds += seconds;
    }

    pub fn add_finished(&mut self, series: &str, today: Day) {
        self.total.finished += 1;
        self.days.entry(date_key(today)).or_default().finished += 1;
        self.series.entry(series.to_string()).or_default().finished += 1;
    }

    pub fn add_movie_time(&mut self, movie: &str, seconds: f64, today: Day) {
        self.total.seconds += seconds;
        self.days.entry(date_key(today)).or_default().seconds += seconds;
        let m = self.movies.entry(movie.to_string()).or_default();
        m.seconds += seconds;
        m.last_day = today;
    }

    pub fn add_movie_finished(&mut self, movie: &str, today: Day) {
        self.total.finished += 1;
        self.days.entry(date_key(today)).or_default().finished += 1;
        let m = self.movies.entry(movie.to_string()).or_default();
        m.finished += 1;
        m.last_day = today;
    }

    /// Counts what was watched before stats were kept: `(series, tally)` for
    /// each remembered video. Only done once.
    pub fn seed(&mut self, videos: impl IntoIterator<Item = (String, Tally)>) {
        if self.seeded {
            return;
        }
        self.seeded = true;
        for (series, tally) in videos {
            self.before.seconds += tally.seconds;
            self.before.finished += tally.finished;
            let s = self.series.entry(series).or_default();
            s.seconds += tally.seconds;
            s.finished += tally.finished;
        }
        self.total.seconds += self.before.seconds;
        self.total.finished += self.before.finished;
    }

    /// Names a series in the stats; an empty name goes back to the folder's.
    pub fn set_name(&mut self, series: &str, name: &str) {
        match name.trim() {
            "" => self.names.remove(series),
            name => self.names.insert(series.to_string(), name.to_string()),
        };
    }

    /// The name a series is shown by: the one it was given, or else its
    /// folder's (the last part of its path).
    pub fn series_name(&self, series: &str) -> String {
        self.names.get(series).cloned().unwrap_or_else(|| folder_name(series).to_string())
    }

    fn day(&self, day: Day) -> Tally {
        self.days.get(&date_key(day)).copied().unwrap_or_default()
    }

    /// What the stats dialog shows, as of `today`.
    pub fn summary(&self, today: Day) -> Summary {
        let last = |n: i64| (today - n + 1..=today).map(|d| self.day(d).seconds).sum::<f64>();
        let active: Vec<Day> = self.days.iter().filter(|(_, t)| t.active()).filter_map(|(k, _)| day_from_key(k)).collect();
        // A streak still counts while today has nothing yet.
        let mut current = 0;
        let mut day = if self.day(today).active() { today } else { today - 1 };
        while self.day(day).active() {
            current += 1;
            day -= 1;
        }
        let (mut longest, mut run, mut prev) = (0, 0, None);
        for &d in &active {
            run = if prev == Some(d - 1) { run + 1 } else { 1 };
            longest = longest.max(run);
            prev = Some(d);
        }
        let best_day = self
            .days
            .iter()
            .filter_map(|(k, t)| Some((day_from_key(k)?, t.seconds)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .filter(|(_, s)| *s >= 1.0);
        let mut top: Vec<(&String, Tally)> = self.series.iter().filter(|(_, t)| t.active()).map(|(k, t)| (k, *t)).collect();
        top.sort_by(|a, b| b.1.seconds.total_cmp(&a.1.seconds).then_with(|| a.0.cmp(b.0)));
        top.truncate(5);
        // Folders of the same name ("Season 1" of two shows) show the folder
        // above too, unless they were given names.
        let folders: Vec<&str> = top.iter().map(|(k, _)| folder_name(k)).collect();
        let top_series = top
            .iter()
            .zip(&folders)
            .map(|((key, tally), folder)| {
                let shared = folders.iter().filter(|f| *f == folder).count() > 1;
                let name = match parent_name(key) {
                    Some(parent) if shared && !self.names.contains_key(*key) => format!("{parent} / {folder}"),
                    _ => self.series_name(key),
                };
                TopSeries { key: (*key).clone(), name, tally: *tally }
            })
            .collect();
        let movie_finishes: u32 = self.movies.values().map(|m| m.finished).sum();
        let mut recent_movies: Vec<(String, Movie)> =
            self.movies.iter().filter(|(_, m)| m.seconds >= 60.0 || m.finished > 0).map(|(k, m)| (k.clone(), *m)).collect();
        recent_movies.sort_by(|a, b| b.1.last_day.cmp(&a.1.last_day).then_with(|| a.0.cmp(&b.0)));
        recent_movies.truncate(5);
        Summary {
            total: self.total,
            before: self.before,
            last_7_days: last(7),
            last_30_days: last(30),
            current_streak: current,
            longest_streak: longest,
            best_day,
            top_series,
            videos_finished: self.total.finished.saturating_sub(movie_finishes),
            movies_watched: self.movies.values().filter(|m| m.finished > 0).count() as u32,
            movie_seconds: self.movies.values().map(|m| m.seconds).sum(),
            recent_movies,
            daily: (today - 29..=today).map(|d| (d, self.day(d).seconds)).collect(),
        }
    }
}

pub struct Summary {
    pub total: Tally,
    pub before: Tally,
    pub last_7_days: f64,
    pub last_30_days: f64,
    /// Days in a row with something watched, up to today (or yesterday).
    pub current_streak: u32,
    pub longest_streak: u32,
    pub best_day: Option<(Day, f64)>,
    /// The most watched series, most first.
    pub top_series: Vec<TopSeries>,
    /// Videos finished that weren't movies.
    pub videos_finished: u32,
    /// Movies finished at least once.
    pub movies_watched: u32,
    pub movie_seconds: f64,
    /// The movies watched last, most recent first.
    pub recent_movies: Vec<(String, Movie)>,
    /// The last 30 days, oldest first, with the seconds watched on each.
    pub daily: Vec<(Day, f64)>,
}

/// One of the most watched series.
pub struct TopSeries {
    /// Its key in `Stats::series`, to name it by.
    pub key: String,
    /// What it is shown as.
    pub name: String,
    pub tally: Tally,
}

/// The last part of a series' path ("Season 1"); a key from before full
/// paths were kept is a name already.
fn folder_name(series: &str) -> &str {
    series.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().filter(|n| !n.is_empty()).unwrap_or(series)
}

/// The folder above a series' folder ("Kirby" for "D:\\Shows\\Kirby\\Season 1").
fn parent_name(series: &str) -> Option<&str> {
    let trimmed = series.trim_end_matches(['\\', '/']);
    let (parent, _) = trimmed.rsplit_once(['\\', '/'])?;
    let name = folder_name(parent);
    (!name.is_empty() && !name.ends_with(':')).then_some(name)
}

/// Days since 1970-01-01 for a date (Howard Hinnant's `days_from_civil`).
pub fn day_from_date(year: i64, month: u32, day: u32) -> Day {
    let (month, day) = (i64::from(month), i64::from(day));
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day: `(year, month, day)`.
pub fn date_of(day: Day) -> (i64, u32, u32) {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn date_key(day: Day) -> String {
    let (y, m, d) = date_of(day);
    format!("{y:04}-{m:02}-{d:02}")
}

fn day_from_key(key: &str) -> Option<Day> {
    let mut parts = key.splitn(3, '-').map(str::parse::<i64>);
    let (y, m, d) = (parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?);
    Some(day_from_date(y, u32::try_from(m).ok()?, u32::try_from(d).ok()?))
}

/// A date for people: "Oct 22, 2026".
pub fn format_date(day: Day) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (y, m, d) = date_of(day);
    format!("{} {d}, {y}", MONTHS[(m - 1) as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_dates() {
        assert_eq!(day_from_date(1970, 1, 1), 0);
        assert_eq!(day_from_date(2000, 3, 1), 11_017);
        for day in [-1000, 0, 11_016, 11_017, 20_383, 30_000] {
            let (y, m, d) = date_of(day);
            assert_eq!(day_from_date(y, m, d), day);
            assert_eq!(day_from_key(&date_key(day)), Some(day));
        }
        assert_eq!(date_key(day_from_date(2026, 10, 22)), "2026-10-22");
        assert_eq!(format_date(day_from_date(2024, 2, 29)), "Feb 29, 2024");
    }

    #[test]
    fn sums_up_watching() {
        let today = day_from_date(2026, 10, 22);
        let mut stats = Stats::default();
        stats.seed([("Kirby".to_string(), Tally { seconds: 3600.0, finished: 2 })]);
        stats.seed([("Again".to_string(), Tally { seconds: 1.0, finished: 1 })]);
        // Three days in a row up to yesterday, and one earlier.
        for (ago, seconds) in [(1, 600.0), (2, 1200.0), (3, 60.0), (10, 7200.0)] {
            stats.add_time("Kirby", seconds, today - ago);
        }
        stats.add_time("Other", 300.0, today - 40);
        stats.add_finished("Kirby", today - 1);

        let s = stats.summary(today);
        assert_eq!(s.total, Tally { seconds: 3600.0 + 600.0 + 1200.0 + 60.0 + 7200.0 + 300.0, finished: 3 });
        assert_eq!(s.before, Tally { seconds: 3600.0, finished: 2 }, "seeded once");
        assert_eq!(s.last_7_days, 1860.0);
        assert_eq!(s.last_30_days, 1860.0 + 7200.0);
        assert_eq!(s.current_streak, 3, "today has nothing yet, so the streak runs to yesterday");
        assert_eq!(s.longest_streak, 3);
        assert_eq!(s.best_day, Some((today - 10, 7200.0)));
        assert_eq!(s.top_series[0].name, "Kirby");
        assert_eq!(s.top_series.len(), 2);
        assert_eq!(s.daily.len(), 30);
        assert_eq!(s.daily[29], (today, 0.0));

        stats.add_time("Kirby", 5.0, today);
        assert_eq!(stats.summary(today).current_streak, 4);
        assert_eq!(stats.summary(today + 2).current_streak, 0, "a missed day ends it");
    }

    #[test]
    fn names_series_apart() {
        let today = day_from_date(2026, 11, 13);
        let mut stats = Stats::default();
        stats.add_time(r"D:\Shows\Kirby\Season 1", 300.0, today);
        stats.add_time(r"D:\Shows\Frieren\Season 1", 200.0, today);
        stats.add_time(r"E:\Movies\Docs", 100.0, today);
        let names = |stats: &Stats| stats.summary(today).top_series.into_iter().map(|s| s.name).collect::<Vec<_>>();
        assert_eq!(names(&stats), ["Kirby / Season 1", "Frieren / Season 1", "Docs"], "the same folder name tells its show");
        stats.set_name(r"D:\Shows\Kirby\Season 1", " Kirby: Right Back at Ya! ");
        assert_eq!(names(&stats), ["Kirby: Right Back at Ya!", "Frieren / Season 1", "Docs"]);
        stats.set_name(r"D:\Shows\Kirby\Season 1", "");
        assert_eq!(stats.series_name(r"D:\Shows\Kirby\Season 1"), "Season 1", "an empty name goes back to the folder's");
        assert_eq!(stats.series_name("Kirby"), "Kirby", "kept before full paths: a name already");
        assert_eq!(parent_name(r"D:\Season 1"), None, "not the drive");
    }
}
