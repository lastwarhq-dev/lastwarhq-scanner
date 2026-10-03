//! What the window shows, built from the shared state. Kept apart from the window so it can be
//! tested without one.

use std::time::Duration;

use crate::app::state::{Install, State, UpdateStatus};
use crate::capture::pipeline::HEARTBEAT_TIMEOUT;
use crate::game::account::Account;
use crate::game::week::{ds_signups_open, vs_day};
use crate::update::Version;

/// How something is doing, drawn as a coloured circle with a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// Green tick.
    Done,
    /// Grey dots: under way, nothing to do.
    Pending,
    /// Amber exclamation mark: something to open in the game.
    Action,
    /// Red cross.
    Failed,
    /// Blue download arrow, for an update.
    Download,
    /// Grey dash: switched off for now, nothing to do.
    Off,
}

/// The whole window's content.
#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    /// The big line at the top, and its mark.
    pub headline: String,
    pub mark: Mark,
    /// The game connection, under the headline, with a dot of the mark's colour.
    pub connection: String,
    pub connection_mark: Mark,
    /// Alliance, roster, DS sign-ups and DS results.
    pub rows: [Row; 4],
    /// What to do about missing VS days, if any are missing.
    pub vs_hint: Option<String>,
    /// Monday to Saturday.
    pub vs_days: [Day; 6],
    pub footer: Footer,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: &'static str,
    pub mark: Mark,
    pub text: String,
}

/// A VS day's tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Day {
    Loaded,
    /// Today: still in progress, so there is nothing to load yet.
    Today,
    /// Over but not loaded.
    Missing,
    /// Later this week.
    Later,
}

/// The line at the bottom: the version, or an update.
#[derive(Debug, Clone, PartialEq)]
pub struct Footer {
    pub mark: Option<Mark>,
    pub text: String,
    /// The update button's label and whether it can be pressed; `None` hides it.
    pub button: Option<(&'static str, bool)>,
}

pub const DAY_NAMES: [&str; 6] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

const IN_SYNC: &str = "In sync";

/// The window's content at `now`.
pub fn screen(state: &State, now: Duration) -> Screen {
    let c = &state.capture;
    let view = &state.view;

    let (connection, connection_mark) = if !c.running {
        match &c.error {
            Some(err) => (err.clone(), Mark::Failed),
            None => (
                "Approve the Windows admin prompt if one is shown".to_string(),
                Mark::Pending,
            ),
        }
    } else if c.server.is_some() {
        match c.last_heartbeat.map(|t| now.saturating_sub(t)) {
            Some(age) if age <= HEARTBEAT_TIMEOUT => ("Game connected".to_string(), Mark::Done),
            Some(age) => (
                format!("No heartbeat from the game for {}s", age.as_secs()),
                Mark::Pending,
            ),
            None => (
                "Game found · waiting for its heartbeat".to_string(),
                Mark::Pending,
            ),
        }
    } else {
        ("Start the game and log in".to_string(), Mark::Pending)
    };

    let alliance = match view.alliance_id() {
        Some(id) => (Mark::Done, alliance_text(view.account(), id)),
        None => (Mark::Action, "Open the member list".to_string()),
    };
    let roster = match view.roster {
        Some(_) => (Mark::Done, IN_SYNC.to_string()),
        None => (Mark::Action, "Open the member list".to_string()),
    };
    let signups = match view.ds_signups {
        _ if !ds_signups_open(now) => (Mark::Off, "Closed until Monday".to_string()),
        Some(_) => (Mark::Done, IN_SYNC.to_string()),
        None => (Mark::Action, "Open the DS participants".to_string()),
    };
    let results = match (&state.mail.loaded, &state.mail.error) {
        // The last read failed: the earlier read's results are kept, but may be out of date.
        (Some(_), Some(err)) => (Mark::Failed, format!("Stale · mail: {err}")),
        (Some(_), None) if view.unattributed_battles() > 0 => {
            (Mark::Action, "Open the member list".to_string())
        }
        (Some(_), None) => (Mark::Done, IN_SYNC.to_string()),
        (None, Some(err)) => (Mark::Failed, format!("Mail: {err}")),
        (None, None) => (Mark::Pending, "Reading mail…".to_string()),
    };
    let row = |label, (mark, text)| Row { label, mark, text };
    let rows = [
        row("Alliance", alliance),
        row("Roster", roster),
        row("DS sign-ups", signups),
        row("DS results", results),
    ];

    let (_, today) = vs_day(now);
    let vs_days: [Day; 6] = std::array::from_fn(|d| {
        let day = d as i64 + 1;
        if view.vs_days[d].is_some() {
            Day::Loaded
        } else if day == today {
            Day::Today
        } else if day < today {
            Day::Missing
        } else {
            Day::Later
        }
    });
    let vs_missing = vs_days.contains(&Day::Missing);
    let vs_hint = vs_missing.then(|| "Open the VS day tabs".to_string());

    let (headline, mark) = if !c.running {
        match c.error {
            Some(_) => ("Not capturing", Mark::Failed),
            None => ("Starting capture", Mark::Pending),
        }
    } else if connection_mark != Mark::Done {
        ("Waiting for the game", Mark::Pending)
    } else if rows
        .iter()
        .all(|r| matches!(r.mark, Mark::Done | Mark::Off))
        && !vs_missing
    {
        ("All in sync", Mark::Done)
    } else {
        ("Open the game panels", Mark::Action)
    };

    Screen {
        headline: headline.to_string(),
        mark,
        connection,
        connection_mark,
        rows,
        vs_hint,
        vs_days,
        footer: footer(&state.update, Version::current()),
    }
}

/// "[ABC] Alliance name", from the account's own messages. The member list carries only the
/// alliance id, so until a name arrives the id stands in.
fn alliance_text(account: Option<&Account>, id: &str) -> String {
    let name = account.and_then(|a| a.alliance_name.as_deref());
    let abbr = account.and_then(|a| a.alliance_abbr.as_deref());
    match (abbr, name) {
        (Some(abbr), Some(name)) => format!("[{abbr}] {name}"),
        (None, Some(name)) => name.to_string(),
        (Some(abbr), None) => format!("[{abbr}]"),
        (None, None) => id.to_string(),
    }
}

/// The footer for the update status, running version `current`.
fn footer(update: &UpdateStatus, current: Version) -> Footer {
    let newer = update.latest.filter(|v| *v > current);
    match (newer, &update.install) {
        (Some(v), Install::Downloading) => Footer {
            mark: Some(Mark::Pending),
            text: format!("Downloading version {v}…"),
            button: Some(("Updating…", false)),
        },
        (Some(_), Install::Failed(err)) => Footer {
            mark: Some(Mark::Failed),
            text: format!("Update failed: {err}"),
            button: Some(("Try again", true)),
        },
        (Some(v), _) => Footer {
            mark: Some(Mark::Download),
            text: format!("Update available · version {v}"),
            button: Some(("Update now", true)),
        },
        (None, Install::Updated) => Footer {
            mark: Some(Mark::Done),
            text: format!("Updated to version {current}"),
            button: None,
        },
        (None, _) => Footer {
            mark: None,
            text: match update.checked {
                Some(_) => format!("Version {current} · up to date"),
                None => format!("Version {current}"),
            },
            button: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::battle::DsBattle;
    use crate::game::panel::Panel;
    use crate::protocol::message::Message;
    use crate::protocol::sfs::Value;

    /// Saturday 2026-10-03 10:00 UTC.
    const SATURDAY: u64 = 1_791_021_600;

    fn connected(now: Duration) -> State {
        let mut state = State::default();
        state.capture.running = true;
        state.capture.server = Some("203.0.113.10:11234".into());
        state.capture.last_heartbeat = Some(now - Duration::from_secs(3));
        state
    }

    /// Records the messages that identify account 7, "Example Player" of "[EXA] Example
    /// Alliance", alliance id "ours".
    fn identify(state: &mut State, secs: u64) {
        let object = |entries: Vec<(&str, Value)>| {
            Value::Object(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
        };
        let text = |s: &str| Value::Str(s.into());
        let mail = object(vec![("toUser", text("7"))]);
        state.record(&Message::command_for_test("push.mail", mail, secs));
        let profile = object(vec![
            ("uid", text("7")),
            ("name", text("Example Player")),
            ("allianceId", text("ours")),
            ("allianceName", text("Example Alliance")),
            ("abbr", text("EXA")),
        ]);
        state.record(&Message::command_for_test(
            "get.new.user.info",
            profile,
            secs,
        ));
    }

    /// Everything loaded at `now`, with no VS day missing on a Monday.
    fn loaded(now: Duration) -> State {
        let mut state = connected(now);
        identify(&mut state, now.as_secs());
        state.view.roster = Some(empty_panel(now));
        state.view.ds_signups = Some(empty_panel(now));
        state.mail.loaded = Some(now);
        state
    }

    fn empty_panel<T>(time: Duration) -> Panel<T> {
        Panel {
            time,
            complete: true,
            entries: Vec::new(),
        }
    }

    #[test]
    fn connection_and_headline() {
        let now = Duration::from_secs(SATURDAY);
        let mut state = State::default();
        let s = screen(&state, now);
        assert_eq!(
            (s.headline.as_str(), s.mark),
            ("Starting capture", Mark::Pending)
        );

        state.capture.running = true;
        let s = screen(&state, now);
        assert_eq!(
            (s.headline.as_str(), s.connection.as_str()),
            ("Waiting for the game", "Start the game and log in")
        );

        let mut state = connected(now);
        let s = screen(&state, now);
        assert_eq!(
            (s.connection.as_str(), s.connection_mark),
            ("Game connected", Mark::Done)
        );
        assert_eq!(
            (s.headline.as_str(), s.mark),
            ("Open the game panels", Mark::Action)
        );

        state.capture.last_heartbeat = Some(now - Duration::from_secs(30));
        let s = screen(&state, now);
        assert_eq!(s.connection, "No heartbeat from the game for 30s");
        assert_eq!(s.headline, "Waiting for the game");

        state.capture.running = false;
        state.capture.error = Some("Npcap is not installed".into());
        let s = screen(&state, now);
        assert_eq!(
            (s.headline.as_str(), s.mark),
            ("Not capturing", Mark::Failed)
        );
        assert_eq!(s.connection, "Npcap is not installed");
    }

    #[test]
    fn all_in_sync_needs_every_row_and_finished_vs_day() {
        // Monday 10:00 UTC: no VS day is over yet.
        let monday = Duration::from_secs(SATURDAY + 2 * 86_400);
        let s = screen(&loaded(monday), monday);
        let texts: Vec<_> = s.rows.iter().map(|r| (r.mark, r.text.as_str())).collect();
        assert_eq!(
            texts,
            [
                (Mark::Done, "[EXA] Example Alliance"),
                (Mark::Done, "In sync"),
                (Mark::Done, "In sync"),
                (Mark::Done, "In sync"),
            ]
        );
        assert_eq!(s.vs_hint, None);
        assert_eq!((s.headline.as_str(), s.mark), ("All in sync", Mark::Done));

        let now = Duration::from_secs(SATURDAY);
        let s = screen(&loaded(now), now);
        // Saturday: Monday to Friday are over, none loaded.
        assert_eq!(
            s.vs_days,
            [
                Day::Missing,
                Day::Missing,
                Day::Missing,
                Day::Missing,
                Day::Missing,
                Day::Today
            ]
        );
        assert_eq!(s.vs_hint.as_deref(), Some("Open the VS day tabs"));
        assert_eq!(s.headline, "Open the game panels");
    }

    #[test]
    fn ds_signups_are_closed_at_the_weekend() {
        let now = Duration::from_secs(SATURDAY);
        let s = screen(&connected(now), now);
        assert_eq!(
            (s.rows[2].mark, s.rows[2].text.as_str()),
            (Mark::Off, "Closed until Monday")
        );
        // Sunday, with everything else loaded: the closed row doesn't hold back the headline.
        let sunday = Duration::from_secs(SATURDAY + 86_400);
        let mut state = loaded(sunday);
        state.view.ds_signups = None;
        for day in &mut state.view.vs_days {
            *day = Some(empty_panel(sunday));
        }
        let s = screen(&state, sunday);
        assert_eq!(s.rows[2].mark, Mark::Off);
        assert_eq!(s.headline, "All in sync");
    }

    #[test]
    fn rows_say_what_to_open() {
        // Friday 10:00 UTC, while sign-ups are open.
        let now = Duration::from_secs(SATURDAY - 86_400);
        let mut state = connected(now);
        let s = screen(&state, now);
        let texts: Vec<_> = s.rows.iter().map(|r| (r.mark, r.text.as_str())).collect();
        assert_eq!(
            texts,
            [
                (Mark::Action, "Open the member list"),
                (Mark::Action, "Open the member list"),
                (Mark::Action, "Open the DS participants"),
                (Mark::Pending, "Reading mail…"),
            ]
        );

        state.mail.error = Some("mail database not found".into());
        let s = screen(&state, now);
        assert_eq!(
            (s.rows[3].mark, s.rows[3].text.as_str()),
            (Mark::Failed, "Mail: mail database not found")
        );

        // Battles read before the alliance is known can't be told apart yet.
        let started = state.start_mail_load(now);
        let battle = DsBattle {
            time: now - Duration::from_secs(3600),
            won: true,
            alliance_id: "ours".into(),
            score: 1,
            enemy_abbr: "X".into(),
            enemy_name: "X".into(),
            enemy_score: 0,
            players: vec![("1".into(), 5)],
            players_complete: true,
        };
        state.finish_mail_load(&started, Ok(vec![battle]), now);
        let s = screen(&state, now);
        assert_eq!(
            (s.rows[3].mark, s.rows[3].text.as_str()),
            (Mark::Action, "Open the member list")
        );
    }

    #[test]
    fn a_failed_mail_refresh_shows_the_results_are_stale() {
        let now = Duration::from_secs(SATURDAY);
        let mut state = loaded(now);
        let s = screen(&state, now);
        assert_eq!(
            (s.rows[3].mark, s.rows[3].text.as_str()),
            (Mark::Done, "In sync")
        );
        // The next read, 5 minutes later, is refused.
        let later = now + Duration::from_secs(300);
        let started = state.start_mail_load(later);
        state.finish_mail_load(
            &started,
            Err("cannot open the mail database: Access is denied. (os error 5)".into()),
            later,
        );
        assert_eq!(state.mail.loaded, Some(now), "the earlier read is kept");
        let s = screen(&state, later);
        assert_eq!(
            (s.rows[3].mark, s.rows[3].text.as_str()),
            (
                Mark::Failed,
                "Stale · mail: cannot open the mail database: Access is denied. (os error 5)"
            )
        );
    }

    #[test]
    fn alliance_from_the_member_list_has_only_its_id() {
        let account = Account {
            uid: "7".into(),
            alliance_abbr: Some("EXA".into()),
            ..Account::default()
        };
        assert_eq!(alliance_text(Some(&account), "ours"), "[EXA]");
        assert_eq!(
            alliance_text(None, "0123456789abcdef0123456789abcdef"),
            "0123456789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn footer_follows_the_update() {
        let v = |s| Version::parse(s).unwrap();
        let current = v("0.2.0");
        let mut update = UpdateStatus::default();
        assert_eq!(footer(&update, current).text, "Version 0.2.0");
        update.checked = Some(Duration::from_secs(SATURDAY));
        update.latest = Some(current);
        let f = footer(&update, current);
        assert_eq!(
            (f.text.as_str(), f.button),
            ("Version 0.2.0 · up to date", None)
        );

        update.latest = Some(v("0.3.0"));
        let f = footer(&update, current);
        assert_eq!(f.text, "Update available · version 0.3.0");
        assert_eq!(f.button, Some(("Update now", true)));

        update.install = Install::Downloading;
        assert_eq!(footer(&update, current).button, Some(("Updating…", false)));
        update.install = Install::Failed("no connection".into());
        let f = footer(&update, current);
        assert_eq!(f.text, "Update failed: no connection");
        assert_eq!(f.button, Some(("Try again", true)));

        // Started by an update, and nothing newer since.
        let update = UpdateStatus {
            latest: Some(current),
            checked: None,
            install: Install::Updated,
        };
        assert_eq!(footer(&update, current).text, "Updated to version 0.2.0");
        // An older latest release is no update.
        let update = UpdateStatus {
            latest: Some(v("0.1.9")),
            ..UpdateStatus::default()
        };
        assert_eq!(footer(&update, current).button, None);
    }
}
