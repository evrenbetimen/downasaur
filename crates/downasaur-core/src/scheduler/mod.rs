//! Download scheduling and network guard.
//!
//! - [`ScheduleWindow`]: "start at 02:00, pause at 06:00" windows, optionally
//!   restricted to weekdays; windows may cross midnight.
//! - [`network`]: metered-connection detection; when the active connection is
//!   metered (phone hotspot, capped plan) large downloads are paused.

pub mod network;

use chrono::{DateTime, Datelike, Local, NaiveTime, TimeZone, Weekday};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleWindow {
    pub start: NaiveTime,
    pub end: NaiveTime,
    /// Days the window *starts* on. Empty means every day.
    #[serde(default)]
    pub days: Vec<Weekday>,
}

impl ScheduleWindow {
    pub fn new(start: NaiveTime, end: NaiveTime) -> Self {
        Self { start, end, days: Vec::new() }
    }

    fn day_allowed(&self, d: Weekday) -> bool {
        self.days.is_empty() || self.days.contains(&d)
    }

    /// Whether downloads may run at `now`.
    pub fn is_open<Tz: TimeZone>(&self, now: &DateTime<Tz>) -> bool {
        let t = now.time();
        let today = now.weekday();
        if self.start <= self.end {
            self.day_allowed(today) && t >= self.start && t < self.end
        } else {
            // Crosses midnight: the late part belongs to today's window, the early
            // part to yesterday's.
            (t >= self.start && self.day_allowed(today)) || (t < self.end && self.day_allowed(today.pred()))
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerConfig {
    pub enabled: bool,
    pub windows: Vec<ScheduleWindow>,
    /// Global bandwidth cap in bytes/second; `None` is unlimited.
    pub bandwidth_limit: Option<u64>,
    /// Pause running downloads while on a metered connection.
    pub pause_on_metered: bool,
}

impl SchedulerConfig {
    /// With scheduling disabled (or no windows), downloads are always allowed.
    pub fn allows_now(&self) -> bool {
        self.allows_at(&Local::now())
    }

    pub fn allows_at<Tz: TimeZone>(&self, now: &DateTime<Tz>) -> bool {
        !self.enabled || self.windows.is_empty() || self.windows.iter().any(|w| w.is_open(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        // 2027-03-10 is a Wednesday.
        Utc.with_ymd_and_hms(2027, 3, 10, h, m, 0).single().expect("valid")
    }

    fn t(h: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, 0, 0).expect("valid")
    }

    #[test]
    fn night_window() {
        let w = ScheduleWindow::new(t(2), t(6));
        assert!(w.is_open(&at(2, 0)));
        assert!(w.is_open(&at(5, 59)));
        assert!(!w.is_open(&at(6, 0)));
        assert!(!w.is_open(&at(14, 0)));
    }

    #[test]
    fn crosses_midnight_with_day_filter() {
        let w = ScheduleWindow { start: t(23), end: t(3), days: vec![Weekday::Tue] };
        assert!(w.is_open(&at(1, 0)), "Wednesday 01:00 belongs to Tuesday's window");
        assert!(!w.is_open(&at(23, 30)), "Wednesday night is not allowed");
    }

    #[test]
    fn disabled_scheduler_always_allows() {
        let c =
            SchedulerConfig { enabled: false, windows: vec![ScheduleWindow::new(t(2), t(6))], ..Default::default() };
        assert!(c.allows_at(&at(12, 0)));
    }
}
