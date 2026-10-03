//! What the capture thread, the mail loader and the window share.

use std::time::Duration;

use crate::game::battle::DsBattle;
use crate::game::view::View;
use crate::protocol::message::Message;

#[derive(Debug, Default)]
pub struct State {
    pub view: View,
    pub capture: Capture,
    pub mail: MailStatus,
}

/// The last "Load mail" press.
#[derive(Debug, Default)]
pub struct MailStatus {
    pub loaded: Option<Duration>,
    /// Why the load failed, if it did.
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct Capture {
    /// Adapter names being captured on.
    pub adapters: Vec<String>,
    pub running: bool,
    /// Why capture is not running, if it stopped.
    pub error: Option<String>,
    /// The locked game connection's server address, e.g. `203.0.113.10:11234`.
    pub server: Option<String>,
    pub last_heartbeat: Option<Duration>,
    pub messages: u64,
    pub last_message: Option<Duration>,
}

/// What the view's data belongs to: the account, its alliance and the VS week.
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    account: Option<String>,
    alliance: Option<String>,
    week: Option<u64>,
}

impl Identity {
    /// Whether `later` belongs to another account, alliance or week than `self`. Learning a
    /// value that was unknown is not a change.
    fn moved_on(&self, later: &Identity) -> bool {
        fn changed<T: PartialEq>(before: &Option<T>, after: &Option<T>) -> bool {
            before.is_some() && before != after
        }
        changed(&self.account, &later.account)
            || changed(&self.alliance, &later.alliance)
            || changed(&self.week, &later.week)
    }
}

impl State {
    /// Records one decoded message and merges it into the view.
    pub fn record(&mut self, message: &Message) {
        self.capture.messages += 1;
        self.capture.last_message = Some(message.time);
        let before = self.identity();
        self.view.apply(message);
        self.forget_mail_if_changed(&before);
    }

    /// Runs the weekly reset from the clock, so it happens even when no messages arrive.
    pub fn tick(&mut self, now: Duration) {
        let before = self.identity();
        self.view.roll_vs_week(now);
        self.forget_mail_if_changed(&before);
    }

    /// Notes what a mail load about to start belongs to; pass it to [`State::finish_mail_load`].
    pub fn start_mail_load(&mut self, now: Duration) -> Identity {
        self.tick(now);
        self.identity()
    }

    /// Applies a finished mail load, read since `started`, as of `now` (when reading finished).
    /// If the account, alliance or week changed while reading, the result is dropped: the
    /// user presses Load mail again. Returns a short summary for the window.
    pub fn finish_mail_load(
        &mut self,
        started: &Identity,
        result: Result<Vec<DsBattle>, String>,
        now: Duration,
    ) -> String {
        self.tick(now);
        if started.moved_on(&self.identity()) {
            return "The game changed account, alliance or week while reading · press Load mail again"
                .to_string();
        }
        match result {
            Ok(battles) => {
                self.view.set_ds_battles(battles, now);
                self.mail = MailStatus {
                    loaded: Some(now),
                    error: None,
                };
                match (
                    self.view.ds_battles().count(),
                    self.view.unattributed_battles(),
                ) {
                    (_, n) if n > 0 => {
                        "Loaded · open the alliance member list to match battles to the alliance"
                            .to_string()
                    }
                    (0, _) => "No Desert Storm results this week".to_string(),
                    (1, _) => "Loaded 1 Desert Storm battle".to_string(),
                    (n, _) => format!("Loaded {n} Desert Storm battles"),
                }
            }
            Err(err) => {
                self.mail = MailStatus {
                    loaded: Some(now),
                    error: Some(err.clone()),
                };
                format!("Mail load failed: {err}")
            }
        }
    }

    /// The account's name, or its player record's name until its own profile arrives.
    pub fn account_name(&self) -> Option<String> {
        let a = self.view.account()?;
        a.name
            .clone()
            .or_else(|| self.view.get(&a.uid).and_then(|p| p.name.clone()))
    }

    fn identity(&self) -> Identity {
        Identity {
            account: self.view.account().map(|a| a.uid.clone()),
            alliance: self.view.alliance_id().map(str::to_string),
            week: self.view.week(),
        }
    }

    /// A mail load belongs to one account, alliance and week; the view drops its battles when
    /// any of them changes, and the load status goes with them.
    fn forget_mail_if_changed(&mut self, before: &Identity) {
        if before.moved_on(&self.identity()) {
            self.mail = MailStatus::default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::sfs::Value;

    /// Saturday 2026-10-03 10:00 UTC.
    const SATURDAY: u64 = 1_791_021_600;

    /// Monday 2026-10-05 02:00 UTC, the weekly reset after `SATURDAY`.
    const RESET: u64 = SATURDAY + 2 * 86_400 - 8 * 3600;

    fn obj(entries: Vec<(&str, Value)>) -> Value {
        Value::Object(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// An `al.rank` member list of `alliance` with one member, uid 1.
    fn roster(alliance: &str, secs: u64) -> Message {
        let member = obj(vec![("uid", Value::Str("1".into()))]);
        let data = obj(vec![
            ("allianceId", Value::Str(alliance.into())),
            ("list", Value::Array(vec![member])),
        ]);
        Message::command_for_test("al.rank", data, secs)
    }

    fn battle(alliance: &str, secs: u64) -> DsBattle {
        DsBattle {
            time: Duration::from_secs(secs),
            won: true,
            alliance_id: alliance.into(),
            score: 1,
            enemy_abbr: "X".into(),
            enemy_name: "X".into(),
            enemy_score: 0,
            players: vec![("1".into(), 5)],
            players_complete: true,
        }
    }

    #[test]
    fn mail_load_is_applied_as_of_when_it_finished() {
        let mut state = State::default();
        state.record(&roster("ours", SATURDAY));
        let started = state.start_mail_load(Duration::from_secs(SATURDAY));
        let finished = Duration::from_secs(SATURDAY + 3);
        let summary = state.finish_mail_load(
            &started,
            Ok(vec![battle("ours", SATURDAY - 3600)]),
            finished,
        );
        assert_eq!(summary, "Loaded 1 Desert Storm battle");
        assert_eq!(state.mail.loaded, Some(finished));
        assert_eq!(state.view.ds_battles().count(), 1);
    }

    #[test]
    fn mail_load_crossing_the_weekly_reset_is_dropped() {
        let last_week = || Ok(vec![battle("ours", SATURDAY)]);
        // The window's clock runs the reset while the file is being read.
        let mut state = State::default();
        state.record(&roster("ours", SATURDAY));
        let started = state.start_mail_load(Duration::from_secs(RESET - 1));
        state.tick(Duration::from_secs(RESET));
        let summary = state.finish_mail_load(&started, last_week(), Duration::from_secs(RESET + 1));
        assert!(summary.contains("press Load mail again"), "{summary}");
        assert_eq!(state.mail.loaded, None);
        assert_eq!(state.view.ds_battles().count(), 0);
        assert_eq!(state.view.unattributed_battles(), 0);

        // Reading finishes after the reset, before the window's clock has run it.
        let mut state = State::default();
        state.record(&roster("ours", SATURDAY));
        let started = state.start_mail_load(Duration::from_secs(RESET - 1));
        state.finish_mail_load(&started, last_week(), Duration::from_secs(RESET + 1));
        assert_eq!(state.mail.loaded, None);
        assert_eq!(state.view.ds_battles().count(), 0);
    }

    #[test]
    fn mail_load_across_an_alliance_change_is_dropped() {
        let mut state = State::default();
        state.record(&roster("ours", SATURDAY));
        let started = state.start_mail_load(Duration::from_secs(SATURDAY));
        state.record(&roster("theirs", SATURDAY + 1));
        state.finish_mail_load(
            &started,
            Ok(vec![battle("ours", SATURDAY - 3600)]),
            Duration::from_secs(SATURDAY + 2),
        );
        assert_eq!(state.mail.loaded, None);
        assert_eq!(state.view.ds_battles().count(), 0);
    }

    #[test]
    fn alliance_change_forgets_mail_load() {
        let mut state = State::default();
        // Loaded before the alliance was known: learning it keeps the load.
        let started = state.start_mail_load(Duration::from_secs(SATURDAY));
        let summary = state.finish_mail_load(
            &started,
            Ok(vec![battle("ours", SATURDAY - 3600)]),
            Duration::from_secs(SATURDAY),
        );
        assert!(summary.contains("member list"), "{summary}");
        state.record(&roster("ours", SATURDAY + 1));
        assert!(state.mail.loaded.is_some());
        assert_eq!(state.view.ds_battles().count(), 1);
        // Another alliance's roster replaces ours: the load belonged to the old alliance.
        state.record(&roster("theirs", SATURDAY + 2));
        assert_eq!(state.mail.loaded, None);
    }

    #[test]
    fn weekly_reset_forgets_mail_load() {
        let mut state = State::default();
        state.tick(Duration::from_secs(SATURDAY));
        state.mail.loaded = Some(Duration::from_secs(SATURDAY));
        state.tick(Duration::from_secs(SATURDAY + 86_400));
        assert!(state.mail.loaded.is_some(), "Sunday is the same week");
        state.tick(Duration::from_secs(SATURDAY + 2 * 86_400));
        assert!(state.mail.loaded.is_none(), "Monday 10:00 is a new week");
    }
}
