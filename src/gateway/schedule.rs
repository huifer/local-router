//! Per-provider usage hours: a provider's models may only be used inside
//! daily time windows, e.g. a night-quota account that serves 22:00–08:00.
//!
//! Windows live in the config (`Provider.schedule`) and are evaluated on
//! every request, so a hot config reload moves the boundary immediately.
//! Like the local RPM/TPM throttle this is **not** a vendor 429: a target
//! outside its window is skipped without a cooldown — in a group the next
//! member answers, and a direct request gets a 429 with `Retry-After`
//! pointing at the window's start.
//!
//! A window with `start > end` crosses midnight (`22:00-08:00` means
//! tonight 22:00 until tomorrow 08:00); equal bounds mean all day. Windows
//! without `models` cover every model the provider serves; with `models`,
//! only those named (exact id or a `*` glob). Several windows combine as a
//! union: any window that covers the model and is open admits the request.

use chrono::{DateTime, Local, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

/// One daily usage window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScheduleWindow {
    /// When the window opens, local to `tz`, as `HH:MM`.
    pub start: String,
    /// When it closes, as `HH:MM`. A close before the start wraps past
    /// midnight; an equal close means the window covers the whole day.
    pub end: String,
    /// An IANA zone name (`Asia/Shanghai`); absent means the machine's
    /// local zone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    /// The models this window restricts. Empty means every model the
    /// provider serves; entries match exactly or as `*` globs.
    #[serde(default)]
    pub models: Vec<String>,
}

impl ScheduleWindow {
    /// Check the window's shape: times parse, the zone exists, the model
    /// patterns are sane. Runs from `Config::validate`.
    pub fn validate(&self) -> Result<(), String> {
        parse_hhmm(&self.start)
            .ok_or_else(|| format!("schedule start '{}' must look like HH:MM", self.start))?;
        parse_hhmm(&self.end)
            .ok_or_else(|| format!("schedule end '{}' must look like HH:MM", self.end))?;
        if let Some(tz) = &self.tz {
            tz.parse::<chrono_tz::Tz>()
                .map_err(|_| format!("schedule tz '{tz}' is not an IANA zone name"))?;
        }
        for pattern in &self.models {
            if pattern.trim().is_empty() {
                return Err("a schedule model pattern must not be empty".into());
            }
        }
        Ok(())
    }

    fn covers(&self, model: &str) -> bool {
        self.models.is_empty() || self.models.iter().any(|p| glob_match(p, model))
    }

    fn bounds(&self) -> Option<(NaiveTime, NaiveTime)> {
        Some((parse_hhmm(&self.start)?, parse_hhmm(&self.end)?))
    }

    fn zone(&self) -> Option<chrono_tz::Tz> {
        self.tz.as_deref().and_then(|tz| tz.parse().ok())
    }
}

/// What the schedule says about one model at one moment.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// No window covers this model (or the provider has no schedule).
    Unrestricted,
    /// Some covering window is open right now.
    Allowed,
    /// Every covering window is closed; the soonest reopening is this far
    /// away. Zero is never returned here — an open window is `Allowed`.
    Closed { seconds_until_open: u64 },
}

/// Judge one model against a provider's windows.
pub fn verdict(windows: &[ScheduleWindow], model: &str, now: DateTime<Utc>) -> Verdict {
    let mut soonest: Option<i64> = None;
    for window in windows {
        if !window.covers(model) {
            continue;
        }
        // A window this judge cannot read is treated as absent: validate()
        // refuses such windows at the door, so this only guards a config
        // that was hand-edited around the check.
        let Some((start, end)) = window.bounds() else {
            continue;
        };
        let (now_local, today) = match window.zone() {
            Some(tz) => {
                let local = now.with_timezone(&tz);
                (local.time(), local.date_naive())
            }
            None => {
                let local = now.with_timezone(&Local);
                (local.time(), local.date_naive())
            }
        };
        if inside(now_local, start, end) {
            return Verdict::Allowed;
        }
        let wait = seconds_until_open(now, now_local, today, start, end, window.zone());
        soonest = Some(soonest.map_or(wait, |prev: i64| prev.min(wait)));
    }
    match soonest {
        None => Verdict::Unrestricted,
        Some(secs) => Verdict::Closed {
            seconds_until_open: secs.max(1) as u64,
        },
    }
}

/// The windows covering one model, for the console's live view.
pub fn covering<'a>(windows: &'a [ScheduleWindow], model: &str) -> Vec<&'a ScheduleWindow> {
    windows.iter().filter(|w| w.covers(model)).collect()
}

/// Whether `now` falls inside the daily window `[start, end)`. An end
/// before the start wraps past midnight; equal bounds cover the day.
fn inside(now: NaiveTime, start: NaiveTime, end: NaiveTime) -> bool {
    if start == end {
        return true;
    }
    if start < end {
        now >= start && now < end
    } else {
        now >= start || now < end
    }
}

/// Seconds from `now` until the window next opens. The open instant is
/// built as a wall-clock time in the window's zone (so a DST shift moves
/// the boundary with the wall clock, not the offset); `chrono` resolves a
/// gap forward and a fold to the earlier instant.
fn seconds_until_open(
    now: DateTime<Utc>,
    now_local: NaiveTime,
    today: chrono::NaiveDate,
    start: NaiveTime,
    end: NaiveTime,
    zone: Option<chrono_tz::Tz>,
) -> i64 {
    // The next opening is today's start when it is still ahead; otherwise
    // tomorrow's. Equal bounds never reach here (they are always open).
    let _ = end;
    let day = if start > now_local {
        today
    } else {
        today.succ_opt().unwrap_or(today)
    };
    let naive = day.and_time(start);
    let opens = match zone {
        Some(tz) => match tz.from_local_datetime(&naive) {
            chrono::LocalResult::Single(t) => t.with_timezone(&Utc),
            chrono::LocalResult::Ambiguous(earlier, _) => earlier.with_timezone(&Utc),
            chrono::LocalResult::None => {
                // A DST gap: the wall time does not exist tonight; the
                // next day's opening is the honest answer.
                let tomorrow = day.succ_opt().unwrap_or(day).and_time(start);
                tz.from_local_datetime(&tomorrow)
                    .earliest()
                    .map(|t| t.with_timezone(&Utc))
                    .unwrap_or(now + chrono::Duration::days(1))
            }
        },
        None => match Local.from_local_datetime(&naive) {
            chrono::LocalResult::Single(t) => t.with_timezone(&Utc),
            chrono::LocalResult::Ambiguous(earlier, _) => earlier.with_timezone(&Utc),
            chrono::LocalResult::None => now + chrono::Duration::days(1),
        },
    };
    (opens - now).num_seconds().max(1)
}

/// `HH:MM` (one-digit hours are tolerated). No seconds: the windows are
/// minute-granular by design.
pub fn parse_hhmm(text: &str) -> Option<NaiveTime> {
    let (h, m) = text.trim().split_once(':')?;
    let h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    NaiveTime::from_hms_opt(h, m, 0)
}

/// A `*` glob, the same shape `auto_include` uses.
fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    if let Some(first) = parts.first() {
        if !first.is_empty() {
            let Some(tail) = rest.strip_prefix(first) else {
                return false;
            };
            rest = tail;
        }
    }
    if let Some(last) = parts.last() {
        if !last.is_empty() {
            let Some(head) = rest.strip_suffix(last) else {
                return false;
            };
            rest = head;
        }
    }
    for middle in &parts[1..parts.len() - 1] {
        if middle.is_empty() {
            continue;
        }
        match rest.find(middle) {
            Some(pos) => rest = &rest[pos + middle.len()..],
            None => return false,
        }
    }
    true
}

/// A live row for the console: one provider's windows and whether they are
/// open right now. `active` is judged on the provider's models as a whole —
/// a window restricted to some models shows its own clock, not the
/// provider's aggregate.
#[derive(Debug, Clone, Serialize)]
pub struct ScheduleRow {
    pub provider: String,
    /// Every window of the provider, with its live state.
    pub windows: Vec<WindowRow>,
    /// Whether at least one window is open right now (any model).
    pub active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowRow {
    pub start: String,
    pub end: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    pub models: Vec<String>,
    /// Open right now, judged on the wall clock.
    pub open: bool,
    /// Seconds until the next opening, when closed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seconds_until_open: Option<u64>,
}

/// Build the live rows for every provider that carries a schedule.
pub fn snapshot(config: &crate::config::Config, now: DateTime<Utc>) -> Vec<ScheduleRow> {
    let mut rows = Vec::new();
    for provider in &config.providers {
        if provider.schedule.is_empty() {
            continue;
        }
        let mut windows = Vec::new();
        let mut active = false;
        for window in &provider.schedule {
            let open = match (window.bounds(), parse_hhmm(&window.start)) {
                (Some((start, end)), _) => {
                    let now_local = match window.zone() {
                        Some(tz) => now.with_timezone(&tz).time(),
                        None => now.with_timezone(&Local).time(),
                    };
                    inside(now_local, start, end)
                }
                _ => false,
            };
            let seconds_until_open = if open {
                None
            } else {
                // Judge with a model the window covers, so the countdown
                // is the window's own, not some other window's.
                let probe = window.models.first().map(String::as_str).unwrap_or("");
                match verdict(std::slice::from_ref(window), probe, now) {
                    Verdict::Closed { seconds_until_open } => Some(seconds_until_open),
                    _ => None,
                }
            };
            active |= open;
            windows.push(WindowRow {
                start: window.start.clone(),
                end: window.end.clone(),
                tz: window.tz.clone(),
                models: window.models.clone(),
                open,
                seconds_until_open,
            });
        }
        rows.push(ScheduleRow {
            provider: provider.id.clone(),
            windows,
            active,
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn window(start: &str, end: &str, tz: Option<&str>, models: &[&str]) -> ScheduleWindow {
        ScheduleWindow {
            start: start.into(),
            end: end.into(),
            tz: tz.map(Into::into),
            models: models.iter().map(|m| m.to_string()).collect(),
        }
    }

    /// A fixed instant: 2026-11-13 12:00 UTC == 20:00 in Asia/Shanghai.
    fn at(hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 13, hour, minute, 0).unwrap()
    }

    #[test]
    fn overnight_window_admits_the_night_and_refuses_the_day() {
        let w = window("22:00", "08:00", Some("Asia/Shanghai"), &[]);
        // 20:00 Shanghai: closed, opens in 2h.
        assert_eq!(
            verdict(std::slice::from_ref(&w), "m", at(12, 0)),
            Verdict::Closed {
                seconds_until_open: 2 * 3600
            }
        );
        // 23:30 Shanghai: open.
        assert_eq!(
            verdict(std::slice::from_ref(&w), "m", at(15, 30)),
            Verdict::Allowed
        );
        // 07:59 Shanghai: still open (the end is exclusive).
        assert_eq!(
            verdict(std::slice::from_ref(&w), "m", at(23, 59)),
            Verdict::Allowed
        );
        // 08:00 Shanghai: closed until 22:00.
        assert_eq!(
            verdict(&[w], "m", at(0, 0)),
            Verdict::Closed {
                seconds_until_open: 14 * 3600
            }
        );
    }

    #[test]
    fn a_daytime_window_is_the_plain_case() {
        let w = window("09:00", "18:00", Some("Asia/Shanghai"), &[]);
        // 10:00 Shanghai: open.
        assert_eq!(
            verdict(std::slice::from_ref(&w), "m", at(2, 0)),
            Verdict::Allowed
        );
        // 18:00 Shanghai: closed, opens tomorrow 09:00 (15h away).
        assert_eq!(
            verdict(&[w], "m", at(10, 0)),
            Verdict::Closed {
                seconds_until_open: 15 * 3600
            }
        );
    }

    #[test]
    fn equal_bounds_cover_the_whole_day() {
        let w = window("00:00", "00:00", Some("UTC"), &[]);
        assert_eq!(verdict(&[w], "m", at(3, 33)), Verdict::Allowed);
    }

    #[test]
    fn model_patterns_scope_the_restriction() {
        let w = window("22:00", "08:00", Some("Asia/Shanghai"), &["gpt-*"]);
        // Daytime: the restricted model is refused…
        assert!(matches!(
            verdict(std::slice::from_ref(&w), "gpt-6", at(12, 0)),
            Verdict::Closed { .. }
        ));
        // …while a model no window covers is unrestricted.
        assert_eq!(verdict(&[w], "claude-5", at(12, 0)), Verdict::Unrestricted);
    }

    #[test]
    fn windows_combine_as_a_union() {
        let night = window("22:00", "08:00", Some("Asia/Shanghai"), &[]);
        let lunch = window("12:00", "13:00", Some("Asia/Shanghai"), &[]);
        // 12:30 Shanghai: the lunch window is open even though night is not.
        assert_eq!(verdict(&[night, lunch], "m", at(4, 30)), Verdict::Allowed);
    }

    #[test]
    fn the_soonest_opening_wins_across_windows() {
        let night = window("22:00", "08:00", Some("Asia/Shanghai"), &[]);
        let lunch = window("12:00", "13:00", Some("Asia/Shanghai"), &[]);
        // 10:00 Shanghai: lunch opens in 2h, night in 12h — report 2h.
        assert_eq!(
            verdict(&[night, lunch], "m", at(2, 0)),
            Verdict::Closed {
                seconds_until_open: 2 * 3600
            }
        );
    }

    #[test]
    fn no_schedule_is_unrestricted() {
        assert_eq!(verdict(&[], "m", at(12, 0)), Verdict::Unrestricted);
    }

    #[test]
    fn the_zone_moves_the_wall_clock() {
        // The same instant is 12:00 UTC but 20:00 Shanghai: a UTC window
        // 09:00-18:00 is open, the Shanghai one is not.
        let utc = window("09:00", "18:00", Some("UTC"), &[]);
        let sh = window("09:00", "18:00", Some("Asia/Shanghai"), &[]);
        assert_eq!(verdict(&[utc], "m", at(12, 0)), Verdict::Allowed);
        assert!(matches!(
            verdict(&[sh], "m", at(12, 0)),
            Verdict::Closed { .. }
        ));
    }

    #[test]
    fn validate_refuses_bad_shapes() {
        assert!(window("22:00", "08:00", None, &[]).validate().is_ok());
        assert!(window("22:00", "08:00", Some("Asia/Shanghai"), &[])
            .validate()
            .is_ok());
        assert!(window("25:00", "08:00", None, &[]).validate().is_err());
        assert!(window("22:00", "8:60", None, &[]).validate().is_err());
        assert!(window("22:00", "08:00", Some("Mars/Olympus"), &[])
            .validate()
            .is_err());
        assert!(window("22:00", "08:00", None, &[""]).validate().is_err());
    }

    #[test]
    fn globs_match_the_auto_include_shape() {
        assert!(glob_match("gpt-*", "gpt-6-astra"));
        assert!(glob_match("*-free", "llm-free"));
        assert!(glob_match("a*b*c", "axxbyyc"));
        assert!(!glob_match("gpt-*", "claude"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "exactly"));
        assert!(glob_match("*", "anything"));
    }

    #[test]
    fn the_snapshot_reports_live_state() {
        let mut config = crate::config::Config::default();
        config.providers.push(crate::config::Provider {
            id: "night".into(),
            schedule: vec![window("22:00", "08:00", Some("Asia/Shanghai"), &[])],
            ..Default::default()
        });
        let rows = snapshot(&config, at(12, 0));
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].active);
        assert_eq!(rows[0].windows[0].seconds_until_open, Some(2 * 3600));
        let rows = snapshot(&config, at(15, 0));
        assert!(rows[0].active);
        assert_eq!(rows[0].windows[0].seconds_until_open, None);
    }

    #[test]
    fn a_dst_gap_opens_the_next_day_not_never() {
        // Lord Howe springs forward 02:00 → 02:30 on 2026-10-04: a window
        // opening at 02:15 that night does not exist; the judge must still
        // return a finite countdown, not panic or hang.
        let w = window("02:15", "03:00", Some("Australia/Lord_Howe"), &[]);
        let now = Utc.with_ymd_and_hms(2026, 10, 3, 15, 0, 0).unwrap(); // 01:30 local, before the gap
        let Verdict::Closed { seconds_until_open } = verdict(&[w], "m", now) else {
            panic!("the window must be closed before it opens");
        };
        assert!(seconds_until_open > 0 && seconds_until_open < 3 * 86400);
    }
}
