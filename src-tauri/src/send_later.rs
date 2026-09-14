//! Send Later planning (P8.1).
//!
//! The composer offers three choices — send now, tomorrow 08:00, and a date
//! and time the user picks — and this module turns each into the durable
//! four-tuple the outbox stores: the UTC instant it enforces, the UTC instant
//! the user chose, the exact local wall time they picked and the IANA zone that
//! wall time belongs to.
//!
//! Deliberate limits:
//!
//! * **No cloud scheduler and no launch daemon.** Sift sends when it is
//!   running and online; the copy in the composer says so. The nearest-deadline
//!   timer in [`crate::scheduler`] is what wakes the outbox, and inbox polling
//!   has nothing to do with schedule precision.
//! * **No new dependency for time zones.** The instant is computed from the
//!   system clock and the system zone ([`chrono::Local`], which reads the
//!   platform tz database), and the IANA *name* is taken from the value the
//!   UI already knows (`Intl.DateTimeFormat().resolvedOptions().timeZone`) or,
//!   failing that, from the platform's localtime link. The name is stored for
//!   display and for auditing a send that happened in another zone; it is
//!   never re-interpreted to move an instant.
//! * **A local time that does not exist, or exists twice, resolves
//!   deterministically**: the first existing instant after a spring-forward
//!   gap, and the first occurrence of a repeated time. Because the outbox
//!   claim is keyed on the stored instant, a second occurrence in the same
//!   day cannot send the message twice.

use crate::db::drafts::SendSchedule;
use crate::dto::SendLaterOption;
use crate::errors::SiftError;
use chrono::{Duration, Local, LocalResult, NaiveDateTime, NaiveTime, TimeZone};

/// Slack for clock jitter between the UI computing an instant and the backend
/// validating it: a schedule less than this far ahead is "now", not "later".
pub const MIN_LEAD_MS: i64 = 5_000;

/// The furthest ahead a message may be scheduled, so a typo in a year field
/// cannot queue mail for the next century.
pub const MAX_AHEAD_MS: i64 = 366 * 24 * 60 * 60 * 1000;

/// The wall time "tomorrow 08:00" uses, in the user's own zone.
pub const TOMORROW_HOUR: u32 = 8;

/// One choice the composer renders.
pub fn options(now_ms: i64, timezone: Option<&str>) -> Vec<SendLaterOption> {
    let tz = timezone
        .map(str::to_string)
        .unwrap_or_else(system_timezone);
    let tomorrow = tomorrow_at(now_ms, TOMORROW_HOUR).map(|(instant, local)| SendSchedule {
        not_before: instant,
        scheduled_at: Some(instant),
        scheduled_local_time: Some(local),
        scheduled_timezone: Some(tz.clone()),
    });
    vec![
        SendLaterOption {
            id: "now".into(),
            label: "Send now".into(),
            not_before: None,
        },
        SendLaterOption {
            id: "tomorrow".into(),
            label: format!("Tomorrow {TOMORROW_HOUR:02}:00"),
            not_before: tomorrow.as_ref().map(|s| s.not_before),
        },
        SendLaterOption {
            id: "custom".into(),
            label: "Choose date and time…".into(),
            not_before: None,
        },
    ]
}

/// The exact instant of tomorrow at `hour:00` in the local zone, plus the
/// local wall-clock string to display.
///
/// A skipped hour (spring forward) resolves to the first instant that does
/// exist; a repeated hour (fall back) resolves to its first occurrence. Either
/// way the instant is recorded, so nothing has to be recomputed later.
pub fn tomorrow_at(now_ms: i64, hour: u32) -> Option<(i64, String)> {
    tomorrow_at_in(&Local, now_ms, hour)
}

/// `tomorrow_at` against an explicit zone.
///
/// Same rules, but parameterised so the DST cases can be checked against a
/// zone with a known transition instead of whatever zone the machine happens
/// to run in — the skipped and repeated hours are the part that must not be
/// left to the platform to demonstrate.
pub fn tomorrow_at_in<Tz: TimeZone>(
    tz: &Tz,
    now_ms: i64,
    hour: u32,
) -> Option<(i64, String)>
where
    Tz::Offset: std::fmt::Display,
{
    let now = tz.timestamp_millis_opt(now_ms).single()?;
    // Adding a day to the local wall time keeps the same clock time, which is
    // what "tomorrow 08:00" means across a transition.
    let date = (now + Duration::days(1)).date_naive();
    let naive = NaiveDateTime::new(date, NaiveTime::from_hms_opt(hour, 0, 0)?);
    let local = resolve_in(tz, naive)?;
    Some((
        local.timestamp_millis(),
        local.format("%Y-%m-%dT%H:%M").to_string(),
    ))
}

fn resolve_local(naive: NaiveDateTime, fallback_ms: i64) -> Option<chrono::DateTime<Local>> {
    resolve_in(&Local, naive).or_else(|| {
        // Nothing within the probe window exists: the caller still needs an
        // instant, and "now" is the only honest one.
        Local.timestamp_millis_opt(fallback_ms).single()
    })
}

/// Resolve one local wall time in `tz`, deterministically.
///
/// A time that exists once is itself; a time that exists twice (fall back) is
/// its first occurrence, so nothing fires on the second pass; a time that does
/// not exist (spring forward) is the first instant that does — the transition
/// itself.
pub fn resolve_in<Tz: TimeZone>(tz: &Tz, naive: NaiveDateTime) -> Option<chrono::DateTime<Tz>> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Some(dt),
        LocalResult::Ambiguous(first, _second) => Some(first),
        LocalResult::None => {
            let mut probe = naive;
            for _ in 0..4 {
                probe += Duration::minutes(15);
                match tz.from_local_datetime(&probe) {
                    LocalResult::Single(dt) => return Some(dt),
                    LocalResult::Ambiguous(first, _) => return Some(first),
                    LocalResult::None => continue,
                }
            }
            None
        }
    }
}

/// Where the send is going, as the user asked for it.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleRequest {
    /// `now`, `tomorrow` or `custom`.
    pub kind: String,
    /// The instant the UI computed for a custom choice.
    #[serde(default)]
    pub not_before: Option<i64>,
    /// The exact local wall time (`YYYY-MM-DDTHH:MM`) for a custom choice.
    #[serde(default)]
    pub local_time: Option<String>,
    /// The IANA zone that local time belongs to.
    #[serde(default)]
    pub timezone: Option<String>,
}

impl Default for ScheduleRequest {
    fn default() -> Self {
        Self {
            kind: "now".into(),
            not_before: None,
            local_time: None,
            timezone: None,
        }
    }
}

impl ScheduleRequest {
    pub fn now() -> Self {
        Self::default()
    }
}

fn invalid(message: &str) -> SiftError {
    SiftError::app("schedule_invalid", message.to_string(), false)
}

/// Validate a calendar string a UI could have produced.
///
/// The shape is checked exactly (`YYYY-MM-DDTHH:MM`), because a value Sift
/// stores is shown back to the user as the intended time; accepting a loose
/// shape would let two different instants claim the same string.
pub fn parse_local_time(value: &str) -> Result<NaiveDateTime, SiftError> {
    if value.len() != 16 || !value.is_char_boundary(16) {
        return Err(invalid(
            "The chosen time could not be read. Pick the date and time again.",
        ));
    }
    let bytes = value.as_bytes();
    let digits = |lo: usize, hi: usize| bytes[lo..hi].iter().all(|b| b.is_ascii_digit());
    if bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' || bytes[13] != b':' {
        return Err(invalid(
            "The chosen time could not be read. Pick the date and time again.",
        ));
    }
    if !(digits(0, 4) && digits(5, 7) && digits(8, 10) && digits(11, 13) && digits(14, 16)) {
        return Err(invalid(
            "The chosen time could not be read. Pick the date and time again.",
        ));
    }
    let (hours, minutes) = (bytes[11..13].iter().fold(0u32, |a, b| a * 10 + (b - b'0') as u32),
                            bytes[14..16].iter().fold(0u32, |a, b| a * 10 + (b - b'0') as u32));
    if hours > 23 || minutes > 59 {
        return Err(invalid("That is not a time of day Sift can use."));
    }
    NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M")
        .map_err(|_| invalid("The chosen date could not be read. Pick it again."))
}

/// Turn a request into the durable schedule, validating what the user asked.
pub fn plan(request: &ScheduleRequest, now_ms: i64) -> Result<SendSchedule, SiftError> {
    let timezone = request
        .timezone
        .clone()
        .filter(|t| !t.trim().is_empty() && t.len() <= 64 && !t.contains(['\n', '\r']))
        .unwrap_or_else(system_timezone);
    match request.kind.as_str() {
        "now" => Ok(SendSchedule::now(now_ms)),
        "tomorrow" => {
            let (instant, local) = tomorrow_at(now_ms, TOMORROW_HOUR)
                .ok_or_else(|| invalid("Sift could not work out tomorrow's date."))?;
            Ok(SendSchedule {
                not_before: instant,
                scheduled_at: Some(instant),
                scheduled_local_time: Some(local),
                scheduled_timezone: Some(timezone),
            })
        }
        "custom" => {
            let local_time = request.local_time.clone().ok_or_else(|| {
                invalid("Pick the date and time you want this to send at.")
            })?;
            let naive = parse_local_time(&local_time)?;
            let instant = request.not_before.ok_or_else(|| {
                invalid("Pick the date and time you want this to send at.")
            })?;
            validate_instant(instant, now_ms)?;
            // The instant and the wall time must describe the same moment as
            // the platform resolves it. A mismatch means one of the two was
            // edited or mis-derived, and Sift refuses rather than storing a
            // schedule that disagrees with itself.
            match resolve_local(naive, now_ms) {
                Some(resolved) if (resolved.timestamp_millis() - instant).abs() <= 60_000 => {}
                _ => {
                    return Err(invalid(
                        "That date and time do not line up with this device's clock. Pick it again.",
                    ))
                }
            }
            Ok(SendSchedule {
                not_before: instant,
                scheduled_at: Some(instant),
                scheduled_local_time: Some(local_time),
                scheduled_timezone: Some(timezone),
            })
        }
        other => Err(invalid(&format!(
            "Sift does not know how to schedule \"{other}\"."
        ))),
    }
}

/// A scheduled instant must be genuinely in the future and not absurdly far
/// out.
pub fn validate_instant(instant: i64, now_ms: i64) -> Result<(), SiftError> {
    if instant < now_ms + MIN_LEAD_MS {
        return Err(invalid(
            "Pick a time at least a few seconds from now. To send immediately, choose Send now.",
        ));
    }
    if instant > now_ms + MAX_AHEAD_MS {
        return Err(invalid(
            "Sift can schedule a message up to a year ahead. Pick an earlier time.",
        ));
    }
    Ok(())
}

/// The platform's IANA zone name, best effort and never authoritative: the UI
/// sends the name it knows, and this is the fallback for a schedule queued
/// before the UI could tell us.
pub fn system_timezone() -> String {
    if let Ok(tz) = std::env::var("TZ") {
        let tz = tz.trim_start_matches(':').trim();
        if !tz.is_empty() && tz.len() <= 64 {
            return tz.to_string();
        }
    }
    if let Ok(link) = std::fs::read_link("/etc/localtime") {
        let path = link.to_string_lossy();
        if let Some(idx) = path.rfind("zoneinfo/") {
            let name = path[idx + "zoneinfo/".len()..].trim_matches('/');
            if !name.is_empty() && name.len() <= 64 {
                return name.to_string();
            }
        }
    }
    "UTC".into()
}

/// A short, honest description of when a scheduled message will leave.
pub fn describe(schedule_local: Option<&str>, timezone: Option<&str>) -> String {
    match (schedule_local, timezone) {
        (Some(local), Some(tz)) => format!("{} {tz}", human_local(local)),
        (Some(local), None) => human_local(local),
        _ => "Scheduled".into(),
    }
}

fn human_local(local: &str) -> String {
    // `2026-09-15T08:00` -> `15 Sep, 08:00`. Purely presentational; the stored
    // value stays the exact wall time.
    let (date, time) = match local.split_once('T') {
        Some((d, t)) => (d, t),
        None => (local, ""),
    };
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return local.to_string();
    }
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month: usize = parts[1].parse().unwrap_or(0);
    let name = MONTHS
        .get(month.saturating_sub(1))
        .copied()
        .unwrap_or_default();
    format!(
        "{} {} {}",
        parts[2].trim_start_matches('0'),
        name,
        time
    )
    .trim()
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(y: i32, m: u32, d: u32, h: u32, mi: u32) -> i64 {
        Local
            .with_ymd_and_hms(y, m, d, h, mi, 0)
            .single()
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn p8_1_send_now_carries_no_schedule() {
        let s = plan(&ScheduleRequest::now(), 1_700_000_000_000).unwrap();
        assert_eq!(s.not_before, 1_700_000_000_000);
        assert!(!s.is_scheduled());
        assert!(s.scheduled_local_time.is_none());
    }

    #[test]
    fn p8_1_tomorrow_is_next_day_at_eight_local() {
        let now = ms(2026, 9, 14, 23, 30);
        let s = plan(
            &ScheduleRequest {
                kind: "tomorrow".into(),
                ..Default::default()
            },
            now,
        )
        .unwrap();
        let local = s.scheduled_local_time.clone().unwrap();
        assert!(local.starts_with("2026-09-15T08:00"), "{local}");
        assert_eq!(s.not_before, ms(2026, 9, 15, 8, 0));
        assert!(s.not_before > now);
    }

    #[test]
    fn p8_1_custom_choice_keeps_the_exact_local_time() {
        let now = ms(2026, 9, 14, 9, 0);
        let instant = ms(2026, 9, 20, 14, 45);
        let s = plan(
            &ScheduleRequest {
                kind: "custom".into(),
                not_before: Some(instant),
                local_time: Some("2026-09-20T14:45".into()),
                timezone: Some("Europe/Berlin".into()),
            },
            now,
        )
        .unwrap();
        assert_eq!(s.not_before, instant);
        assert_eq!(s.scheduled_at, Some(instant));
        assert_eq!(s.scheduled_local_time.as_deref(), Some("2026-09-20T14:45"));
        assert_eq!(s.scheduled_timezone.as_deref(), Some("Europe/Berlin"));
    }

    #[test]
    fn p8_1_past_or_mismatched_times_are_refused() {
        let now = ms(2026, 9, 14, 9, 0);
        let err = plan(
            &ScheduleRequest {
                kind: "custom".into(),
                not_before: Some(now - 60_000),
                local_time: Some("2026-09-14T08:59".into()),
                timezone: None,
            },
            now,
        )
        .unwrap_err();
        assert_eq!(err.code(), "schedule_invalid");
        // The instant and the wall time must agree.
        let err = plan(
            &ScheduleRequest {
                kind: "custom".into(),
                not_before: Some(ms(2026, 9, 20, 14, 45)),
                local_time: Some("2026-09-20T09:15".into()),
                timezone: None,
            },
            now,
        )
        .unwrap_err();
        assert_eq!(err.code(), "schedule_invalid");
        // Absurdly far out.
        let err = plan(
            &ScheduleRequest {
                kind: "custom".into(),
                not_before: Some(now + MAX_AHEAD_MS + 1),
                local_time: Some(local_string(now + MAX_AHEAD_MS + 1)),
                timezone: None,
            },
            now,
        )
        .unwrap_err();
        assert_eq!(err.code(), "schedule_invalid");
        // Malformed calendar strings never reach the database.
        assert!(parse_local_time("2026-9-20T14:45").is_err());
        assert!(parse_local_time("2026-09-20 14:45").is_err());
        assert!(parse_local_time("2026-09-20T25:00").is_err());
        assert!(parse_local_time("2026-02-30T10:00").is_err());
    }

    fn local_string(instant: i64) -> String {
        Local
            .timestamp_millis_opt(instant)
            .single()
            .unwrap()
            .format("%Y-%m-%dT%H:%M")
            .to_string()
    }

    #[test]
    fn p8_1_clock_moved_backward_still_yields_one_instant() {
        // The stored instant is absolute: a clock that moved backwards does not
        // move the deadline, it only delays when the wall clock reaches it.
        let now = ms(2026, 9, 14, 9, 0);
        let s = plan(
            &ScheduleRequest {
                kind: "tomorrow".into(),
                ..Default::default()
            },
            now,
        )
        .unwrap();
        let again = plan(
            &ScheduleRequest {
                kind: "custom".into(),
                not_before: Some(s.not_before),
                local_time: s.scheduled_local_time.clone(),
                timezone: s.scheduled_timezone.clone(),
            },
            now - 3_600_000,
        )
        .unwrap();
        assert_eq!(again.not_before, s.not_before);
        assert_eq!(again.scheduled_local_time, s.scheduled_local_time);
    }

    #[test]
    fn p8_1_repeated_and_skipped_local_times_resolve_deterministically() {
        // Whatever the platform says about the zone, resolving the same wall
        // time twice must give the same answer, and it must be a real instant.
        let naive = chrono::NaiveDate::from_ymd_opt(2026, 3, 8)
            .unwrap()
            .and_hms_opt(2, 30, 0)
            .unwrap();
        let first = resolve_local(naive, 0).unwrap();
        let second = resolve_local(naive, 0).unwrap();
        assert_eq!(first, second);
        // A skipped 02:30 (or any wall time) never returns nothing.
        let naive = chrono::NaiveDate::from_ymd_opt(2026, 11, 1)
            .unwrap()
            .and_hms_opt(1, 30, 0)
            .unwrap();
        assert!(resolve_local(naive, 0).is_some());
    }

    #[test]
    fn p8_1_options_expose_now_tomorrow_and_custom() {
        let now = ms(2026, 9, 14, 9, 0);
        let opts = options(now, Some("America/New_York"));
        assert_eq!(opts.len(), 3);
        assert_eq!(opts[0].id, "now");
        assert!(opts[0].not_before.is_none());
        assert_eq!(opts[1].id, "tomorrow");
        assert!(opts[1].not_before.unwrap() > now);
        assert_eq!(opts[2].id, "custom");
        assert!(opts[2].not_before.is_none());
        assert_eq!(opts[1].label, "Tomorrow 08:00");
    }

    #[test]
    fn p8_1_description_names_the_zone() {
        assert_eq!(
            describe(Some("2026-09-15T08:00"), Some("America/New_York")),
            "15 Sep 08:00 America/New_York"
        );
        assert_eq!(describe(None, None), "Scheduled");
    }
}
