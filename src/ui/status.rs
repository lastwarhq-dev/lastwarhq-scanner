//! The window's lines of text, built from the shared state. Kept apart from the window so it
//! can be tested without one.

use std::fmt::Write;
use std::time::Duration;

use crate::app::state::State;
use crate::capture::pipeline::HEARTBEAT_TIMEOUT;
use crate::game::week::{monday_days, vs_day};
use crate::util::time::civil;

/// How the connection line is shown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Health {
    Good,
    Waiting,
    Failed,
}

/// The window's text, one field per line.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusLines {
    pub connection: String,
    pub health: Health,
    pub account: String,
    pub week: String,
    pub roster: String,
    pub signups: String,
    pub results: String,
    pub vs: String,
    pub activity: String,
}

const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The window's text at `now`.
pub fn status_lines(state: &State, now: Duration) -> StatusLines {
    let c = &state.capture;
    let view = &state.view;
    let (connection, health) = if !c.running {
        match &c.error {
            Some(err) => (format!("Not capturing: {err}"), Health::Failed),
            None => (
                "Starting capture · approve the Windows admin prompt if shown".to_string(),
                Health::Waiting,
            ),
        }
    } else if let Some(server) = &c.server {
        match c.last_heartbeat.map(|t| now.saturating_sub(t).as_secs()) {
            Some(age) if Duration::from_secs(age) <= HEARTBEAT_TIMEOUT => (
                format!("Connected · {server} · heartbeat {age}s ago"),
                Health::Good,
            ),
            Some(age) => (
                format!("No heartbeat for {age}s · {server}"),
                Health::Waiting,
            ),
            None => (
                format!("Connected · {server} · waiting for heartbeat"),
                Health::Waiting,
            ),
        }
    } else {
        (
            format!("Searching for the game on {}…", c.adapters.join(", ")),
            Health::Waiting,
        )
    };

    let account = match view.account() {
        None => "waiting for the game to identify it".to_string(),
        Some(a) => {
            let mut s = state
                .account_name()
                .unwrap_or_else(|| format!("uid {}", a.uid));
            if let Some(alliance) = &a.alliance_name {
                let _ = write!(s, " · {alliance}");
                if let Some(abbr) = &a.alliance_abbr {
                    let _ = write!(s, " [{abbr}]");
                }
            }
            s
        }
    };

    let (week, today) = vs_day(now);
    let monday = monday_days(week);
    let week_line = format!(
        "{} – {} · today {} · resets Monday 02:00 UTC",
        day_label(monday),
        day_label(monday + 6),
        WEEKDAYS[(today - 1) as usize]
    );

    let players: Vec<_> = view.players().collect();
    let roster = match view.roster_at {
        Some(t) => format!("{} players · {}", players.len(), when(t, now)),
        None if players.is_empty() => "not loaded · open the alliance member list".to_string(),
        None => format!(
            "{} players (from sign-ups) · open the member list for ranks",
            players.len()
        ),
    };

    let signups = match view.signups_at {
        Some(t) => {
            let picked = players
                .iter()
                .filter(|p| p.choose_time_list.as_ref().is_some_and(|l| !l.is_empty()))
                .count();
            let assigned = players
                .iter()
                .filter(|p| p.ds_group.is_some_and(|g| g > 0))
                .count();
            format!(
                "{picked} picked times · {assigned} assigned to a team · {}",
                when(t, now)
            )
        }
        None => "not loaded · open the Desert Storm participants panel".to_string(),
    };

    let results = match (&state.mail.loaded, &state.mail.error) {
        (None, _) => "not loaded · press Load mail".to_string(),
        (Some(_), Some(err)) => format!("load failed: {err}"),
        (Some(t), None) => {
            let mut battles: Vec<_> = view.ds_battles().collect();
            battles.sort_by_key(|(b, _)| b.time);
            let unattributed = view.unattributed_battles();
            if unattributed > 0 {
                format!(
                    "{unattributed} battles loaded · open the alliance member list to tell which are ours · {}",
                    when(*t, now)
                )
            } else if battles.is_empty() {
                format!("none this week · checked {}", when(*t, now))
            } else {
                let parts: Vec<String> = battles
                    .iter()
                    .map(|(b, team)| {
                        format!(
                            "{} {}",
                            team_name(*team),
                            if b.won { "won" } else { "lost" }
                        )
                    })
                    .collect();
                let fought: usize = battles.iter().map(|(b, _)| b.players.len()).sum();
                format!(
                    "{} · {fought} players · {}",
                    parts.join(" · "),
                    when(*t, now)
                )
            }
        }
    };

    let days = view.vs_days_loaded();
    let marks: Vec<String> = (0..6)
        .map(|d| {
            let mark = if days[d] {
                "✓"
            } else if d as i64 + 1 == today {
                "…"
            } else {
                "–"
            };
            format!("{} {mark}", &WEEKDAYS[d][..3])
        })
        .collect();
    let vs = match view.vs_at {
        Some(t) => format!("{} · {}", marks.join("  "), when(t, now)),
        None => format!("{} · open the VS day tabs", marks.join("  ")),
    };

    let activity = match c.last_message {
        Some(t) => format!("{} messages · last {}", thousands(c.messages), when(t, now)),
        None => "no messages yet".to_string(),
    };

    StatusLines {
        connection,
        health,
        account,
        week: week_line,
        roster,
        signups,
        results,
        vs,
        activity,
    }
}

/// Panel group 1 is Team A, group 2 is Team B.
fn team_name(team: Option<i64>) -> &'static str {
    match team {
        Some(1) => "Team A",
        Some(2) => "Team B",
        _ => "Team ?",
    }
}

/// "Mon 28 Sep" for a day number.
fn day_label(days: u64) -> String {
    let (_, month, day) = civil(days);
    let weekday = ((days + 3) % 7) as usize;
    format!(
        "{} {day} {}",
        &WEEKDAYS[weekday][..3],
        MONTHS[month as usize - 1]
    )
}

/// "22:44 UTC" today, "Fri 2 Oct 22:44 UTC" on another day.
fn when(time: Duration, now: Duration) -> String {
    let secs = time.as_secs();
    let clock = format!("{:02}:{:02} UTC", secs / 3600 % 24, secs / 60 % 60);
    if secs / 86_400 == now.as_secs() / 86_400 {
        clock
    } else {
        format!("{} {clock}", day_label(secs / 86_400))
    }
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Saturday 2026-10-03 10:00 UTC.
    const SATURDAY: u64 = 1_791_021_600;

    #[test]
    fn week_and_time_labels() {
        let now = Duration::from_secs(SATURDAY);
        let lines = status_lines(&State::default(), now);
        assert_eq!(
            lines.week,
            "Mon 28 Sep – Sun 4 Oct · today Saturday · resets Monday 02:00 UTC"
        );
        assert_eq!(when(Duration::from_secs(SATURDAY - 3600), now), "09:00 UTC");
        assert_eq!(
            when(Duration::from_secs(SATURDAY - 86_400), now),
            "Fri 2 Oct 10:00 UTC"
        );
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn connection_line() {
        let now = Duration::from_secs(SATURDAY);
        let mut state = State::default();
        assert_eq!(status_lines(&state, now).health, Health::Waiting);
        state.capture.running = true;
        state.capture.server = Some("203.0.113.10:11234".into());
        state.capture.last_heartbeat = Some(now - Duration::from_secs(3));
        let lines = status_lines(&state, now);
        assert_eq!(
            (lines.connection.as_str(), lines.health),
            (
                "Connected · 203.0.113.10:11234 · heartbeat 3s ago",
                Health::Good
            )
        );
        state.capture.last_heartbeat = Some(now - Duration::from_secs(30));
        assert_eq!(status_lines(&state, now).health, Health::Waiting);
        state.capture.running = false;
        state.capture.error = Some("Npcap is not installed".into());
        assert_eq!(status_lines(&state, now).health, Health::Failed);
    }
}
