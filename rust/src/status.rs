use std::time::Duration;

use serde_json::Value;

/// Where a message stands. Mast may learn new states, so an unrecognised one is
/// carried through rather than dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Queued,
    Muted,
    Deduped,
    Suppressed,
    /// A vital's heartbeat. It stores no card, so there is no id to follow.
    Alive,
    Acked,
    Resolved,
    Expired,
    Other(String),
}

impl State {
    /// States a message never leaves, which is when polling it stops.
    pub fn is_terminal(&self) -> bool {
        matches!(self, State::Acked | State::Resolved | State::Expired)
    }

    fn parse(value: &str) -> State {
        match value {
            "queued" => State::Queued,
            "muted" => State::Muted,
            "deduped" => State::Deduped,
            "suppressed" => State::Suppressed,
            "alive" => State::Alive,
            "acked" => State::Acked,
            "resolved" => State::Resolved,
            "expired" => State::Expired,
            other => State::Other(other.to_string()),
        }
    }
}

/// What Mast answered a send with.
#[derive(Debug, Clone)]
pub struct Sent {
    /// None for a heartbeat, which stores no card.
    pub id: Option<String>,
    pub state: State,
    /// True when the send folded into a card an earlier one opened.
    pub duplicate: bool,
    /// How long the incident a resolve just closed had been open.
    pub open_for: Option<Duration>,
}

impl Sent {
    pub(crate) fn from_json(payload: &Value) -> Sent {
        Sent {
            id: text(payload, "id"),
            state: State::parse(&text(payload, "state").unwrap_or_else(|| "queued".into())),
            duplicate: payload
                .get("duplicate")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            open_for: payload
                .get("open_for")
                .and_then(Value::as_u64)
                .map(Duration::from_secs),
        }
    }
}

/// One message as Mast currently sees it.
#[derive(Debug, Clone)]
pub struct Status {
    pub id: Option<String>,
    pub state: State,
    /// Stamps come through as Mast sent them, RFC 3339 in UTC. A stamp Mast
    /// could not work out is left out rather than zeroed, so None here is
    /// unknown and not instant.
    pub received_at: Option<String>,
    pub acked_at: Option<String>,
    pub acked_by: Option<String>,
    pub resolved_at: Option<String>,
    pub expires_at: Option<String>,
    pub dedupe_count: u64,
}

impl Status {
    pub(crate) fn from_json(payload: &Value) -> Status {
        Status {
            id: text(payload, "id"),
            state: State::parse(&text(payload, "state").unwrap_or_default()),
            received_at: text(payload, "received_at"),
            acked_at: text(payload, "acked_at"),
            acked_by: text(payload, "acked_by"),
            resolved_at: text(payload, "resolved_at"),
            expires_at: text(payload, "expires_at"),
            dedupe_count: payload
                .get("dedupe_count")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        }
    }

    /// Whether a human answered the page.
    pub fn acknowledged(&self) -> bool {
        self.state == State::Acked
    }

    pub fn done(&self) -> bool {
        self.state.is_terminal()
    }

    /// How long the page stood open, or None when either stamp is missing or
    /// is not a UTC RFC 3339 stamp this can read.
    pub fn open_for(&self) -> Option<Duration> {
        let start = epoch_seconds(self.received_at.as_deref()?)?;
        let end = self
            .acked_at
            .as_deref()
            .or(self.resolved_at.as_deref())
            .and_then(epoch_seconds)?;
        Some(Duration::from_secs(end.saturating_sub(start).max(0) as u64))
    }
}

fn text(payload: &Value, field: &str) -> Option<String> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Seconds since the epoch for a stamp like 2026-09-17T10:00:00Z, with or
/// without a fraction. Mast stamps in UTC; an offset this does not read is
/// reported as unknown rather than guessed at.
fn epoch_seconds(stamp: &str) -> Option<i64> {
    let bytes = stamp.as_bytes();
    if bytes.len() < 20 || bytes[10] != b'T' || !stamp.ends_with('Z') {
        return None;
    }
    let year: i64 = stamp.get(0..4)?.parse().ok()?;
    let month: i64 = stamp.get(5..7)?.parse().ok()?;
    let day: i64 = stamp.get(8..10)?.parse().ok()?;
    let hour: i64 = stamp.get(11..13)?.parse().ok()?;
    let minute: i64 = stamp.get(14..16)?.parse().ok()?;
    let second: i64 = stamp.get(17..19)?.parse().ok()?;
    if bytes[4] != b'-' || bytes[7] != b'-' || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Days between 1970-01-01 and a civil date, by Howard Hinnant's algorithm.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}
