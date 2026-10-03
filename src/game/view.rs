//! The current view: the newest list from each panel, kept as the panel sent it, and this
//! week's Desert Storm battles from the mail. Panels are not merged; each entry carries its
//! player's uid, and whoever receives the data joins panels by it.
//!
//! The view belongs to one account, one alliance and one VS week. When any of them changes,
//! [`View::clear`] drops what belonged to the old one:
//!
//! | Change | Cleared |
//! |---|---|
//! | VS week | VS rankings, Desert Storm sign-ups and battles |
//! | Alliance | the above, and the member list |
//! | Account | everything, including the account itself |
//!
//! Desert Storm sign-ups are also cleared, and new ones ignored, from Saturday until the
//! weekly reset (see [`ds_signups_open`]).

use std::time::Duration;

use crate::game::account::{Account, own_uid};
use crate::game::battle::DsBattle;
use crate::game::panel::{Member, Panel, Participant, VsScore};
use crate::game::week::{ds_signups_open, vs_day};
use crate::protocol::message::Message;
use crate::protocol::sfs::Value;

#[derive(Debug, Default)]
pub struct View {
    account: Option<Account>,
    /// The alliance whose member list the roster came from. Used as our alliance until the
    /// account's own messages name it.
    roster_alliance: Option<String>,
    /// VS week the view's data belongs to.
    vs_week: Option<u64>,
    /// The alliance member list.
    pub roster: Option<Panel<Member>>,
    /// The Desert Storm participants panel; `None` from Saturday to the weekly reset.
    pub ds_signups: Option<Panel<Participant>>,
    /// Each completed VS day's ranking, Monday (0) to Saturday (5).
    pub vs_days: [Option<Panel<VsScore>>; 6],
    /// This week's Desert Storm battles, from the last mail load.
    ds_battles: Vec<DsBattle>,
}

/// What [`View::clear`] drops; each scope includes the ones before it.
enum Scope {
    /// A new VS week.
    Week,
    /// Another alliance, on the same account.
    Alliance,
    /// Another account, with this uid.
    Account(String),
}

impl View {
    /// Takes in a panel message. Returns false for messages that change nothing.
    pub fn apply(&mut self, message: &Message) -> bool {
        let rolled = self.roll_vs_week(message.time);
        // A message captured before the weekly reset but handled after it (the window's clock
        // can run the reset first) belongs to the week that has ended.
        if self.vs_week.is_some_and(|w| vs_day(message.time).0 < w) {
            return rolled;
        }
        let Some(data) = message.data() else {
            return rolled;
        };
        let command = message.command().unwrap_or_default();
        let changed = self.note_account(command, data) || rolled;
        match command {
            "al.rank" => {
                let list = read_list(data, "list", member, message.time);
                // An empty list means the message was not understood; it says nothing about
                // which alliance this is.
                let Some(list) = list else {
                    return changed;
                };
                if self.note_roster_alliance(data).is_none() {
                    return changed;
                }
                self.roster = Some(list);
                true
            }
            "dragon.assign.player.info" => {
                if !ds_signups_open(message.time) {
                    return changed;
                }
                match read_list(data, "users", participant, message.time) {
                    Some(list) => {
                        self.ds_signups = Some(list);
                        true
                    }
                    None => changed,
                }
            }
            "al.battle.rank.info" => self.apply_vs(data, message.time) || changed,
            _ => changed,
        }
    }

    /// The VS week the view holds data for.
    pub fn week(&self) -> Option<u64> {
        self.vs_week
    }

    pub fn account(&self) -> Option<&Account> {
        self.account.as_ref()
    }

    /// Our alliance: as the account's own messages name it, or else the alliance whose member
    /// list the roster came from. `None` until either has arrived.
    pub fn alliance_id(&self) -> Option<&str> {
        self.account
            .as_ref()
            .and_then(|a| a.alliance_id.as_deref())
            .or(self.roster_alliance.as_deref())
    }

    /// Our alliance's warzone: the `serverId` the member list gives its members, when every
    /// member that has one has the same. `None` until the member list has arrived, or if they
    /// differ.
    pub fn warzone(&self) -> Option<i64> {
        let mut ids = self
            .roster
            .as_ref()?
            .entries
            .iter()
            .filter_map(|m| m.server_id);
        let first = ids.next()?;
        ids.all(|id| id == first).then_some(first)
    }

    /// Drops what belonged to the old week, alliance or account. The only place the view's
    /// data is cleared, apart from sign-ups closing at the weekend.
    fn clear(&mut self, scope: Scope) {
        if let Scope::Account(uid) = scope {
            *self = View {
                account: Some(Account {
                    uid,
                    ..Account::default()
                }),
                vs_week: self.vs_week,
                ..View::default()
            };
            return;
        }
        self.ds_battles.clear();
        self.ds_signups = None;
        self.vs_days = Default::default();
        if let Scope::Alliance = scope {
            self.roster_alliance = None;
            self.roster = None;
        }
    }

    /// Tracks the logged-in account. A different uid means the user switched accounts: every
    /// list and result belongs to the old account, so the view starts again. A different
    /// alliance on the same account clears the old alliance's data.
    fn note_account(&mut self, command: &str, data: &Value) -> bool {
        let mut changed = false;
        if let Some(uid) = own_uid(command, data) {
            match &self.account {
                Some(account) if account.uid == uid => {}
                Some(_) => {
                    self.clear(Scope::Account(uid));
                    changed = true;
                }
                None => {
                    self.account = Some(Account {
                        uid,
                        ..Account::default()
                    })
                }
            }
        }
        let before = self.alliance_id().map(str::to_string);
        let Some(account) = self.account.as_mut() else {
            return changed;
        };
        changed |= account.absorb(command, data);
        let after = account.alliance_id.clone();
        if before.is_some() && after.is_some() && before != after {
            self.clear(Scope::Alliance);
        }
        changed
    }

    /// Checks an `al.rank` member list's alliance. Returns `None` when the list is another
    /// alliance's (the account's own alliance is known and differs), so it must be ignored.
    /// Otherwise returns whether the view moved to a new alliance: while only rosters say
    /// which alliance this is, the newest roster wins.
    fn note_roster_alliance(&mut self, data: &Value) -> Option<bool> {
        let Some(id) = string(data, "allianceId") else {
            return Some(false);
        };
        let account_alliance = self.account.as_ref().and_then(|a| a.alliance_id.as_deref());
        if account_alliance.is_some_and(|ours| ours != id) {
            return None;
        }
        let moved =
            account_alliance.is_none() && self.roster_alliance.as_deref().is_some_and(|r| r != id);
        if moved {
            self.clear(Scope::Alliance);
        }
        self.roster_alliance = Some(id);
        Some(moved)
    }

    /// Clears everything that belongs to one VS week once `time` falls in a later week: VS
    /// rankings and Desert Storm sign-ups and battles. The member list stays. Also clears the
    /// sign-ups from Saturday on. Called for every message and, from the window's clock, every
    /// second, so both happen even while the game is closed. Returns whether anything changed.
    pub fn roll_vs_week(&mut self, time: Duration) -> bool {
        let (week, _) = vs_day(time);
        // A time from an earlier week (a message captured before the reset but handled after
        // it) says nothing about this week, so it changes nothing.
        if self.vs_week.is_some_and(|w| week < w) {
            return false;
        }
        let new_week = self.vs_week.is_some_and(|w| week > w);
        if new_week {
            self.clear(Scope::Week);
        }
        self.vs_week = Some(week);
        let closed = !ds_signups_open(time) && self.ds_signups.take().is_some();
        new_week || closed
    }

    /// Replaces the Desert Storm battles with those from the view's VS week, after running the
    /// weekly reset for `now`. The view's week can already be later than `now` (the window's
    /// clock ran the reset first), and then that later week is the one kept. Returns how many
    /// were kept.
    pub fn set_ds_battles(&mut self, battles: Vec<DsBattle>, now: Duration) -> usize {
        self.roll_vs_week(now);
        let week = self.vs_week;
        self.ds_battles = battles
            .into_iter()
            .filter(|b| Some(vs_day(b.time).0) == week)
            .collect();
        self.ds_battles.len()
    }

    /// This week's battles fought by our alliance. The mail database holds every account's
    /// mail, so it can hold other alliances' battles too; a battle counts only when it belongs
    /// to our alliance. Until our alliance is known, none do: a player's past battle for
    /// another alliance must not pass as ours.
    pub fn ds_battles(&self) -> impl Iterator<Item = &DsBattle> {
        let alliance = self.alliance_id();
        self.ds_battles
            .iter()
            .filter(move |b| alliance == Some(b.alliance_id.as_str()))
    }

    /// Battles from the last mail load that can't be attributed yet because our alliance is
    /// not known.
    pub fn unattributed_battles(&self) -> usize {
        if self.alliance_id().is_some() {
            0
        } else {
            self.ds_battles.len()
        }
    }

    /// `al.battle.rank.info`: one day's VS ranking for both alliances, kept whole. The current
    /// day is skipped because its scores keep changing until the reset.
    fn apply_vs(&mut self, data: &Value, time: Duration) -> bool {
        let Some(day) = int(data, "day") else {
            return false;
        };
        let (_, today) = vs_day(time);
        if !(1..=6).contains(&day) || day >= today {
            return false;
        }
        match read_list(data, "rankInfo", vs_score, time) {
            Some(list) => {
                self.vs_days[(day - 1) as usize] = Some(list);
                true
            }
            None => false,
        }
    }
}

/// Entries of the array at `key`, skipping anything without a string `uid`.
fn entries<'a>(data: &'a Value, key: &str) -> impl Iterator<Item = (&'a Value, String)> {
    data.get(key)
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| Some((entry, entry.get("uid")?.as_str()?.to_string())))
}

fn string(entry: &Value, key: &str) -> Option<String> {
    entry.get(key)?.as_str().map(str::to_string)
}

fn int(entry: &Value, key: &str) -> Option<i64> {
    entry.get(key)?.as_i64()
}

/// An integer list sent either as a typed int array or as an array of integer values.
fn int_list(entry: &Value, key: &str) -> Option<Vec<i64>> {
    match entry.get(key)? {
        Value::IntArray(items) => Some(items.iter().map(|&v| v.into()).collect()),
        Value::Array(items) => items.iter().map(Value::as_i64).collect(),
        _ => None,
    }
}

/// The array at `key` read with `entry`, as a panel list arrived at `time`. `None` when no
/// entry could be read: the message was not understood, which says nothing about the panel.
fn read_list<T>(
    data: &Value,
    key: &str,
    entry: impl Fn(&Value, String) -> T,
    time: Duration,
) -> Option<Panel<T>> {
    let total = data
        .get(key)
        .and_then(Value::as_array)
        .map_or(0, <[Value]>::len);
    let entries: Vec<T> = entries(data, key).map(|(e, uid)| entry(e, uid)).collect();
    if entries.is_empty() {
        return None;
    }
    Some(Panel {
        time,
        complete: entries.len() == total,
        entries,
    })
}

/// A row of `al.rank`, the alliance member list.
fn member(m: &Value, uid: String) -> Member {
    Member {
        uid,
        name: string(m, "name"),
        rank: int(m, "rank"),
        power: int(m, "power"),
        army_kill: int(m, "armyKill"),
        server_id: int(m, "serverId").or_else(|| string(m, "serverId")?.parse().ok()),
    }
}

/// A row of `dragon.assign.player.info`, the Desert Storm participants panel.
fn participant(u: &Value, uid: String) -> Participant {
    Participant {
        uid,
        hero_power: int(u, "heroPower"),
        choose_time_list: int_list(u, "chooseTimeList"),
        group: int(u, "group"),
    }
}

/// A row of `al.battle.rank.info`'s `rankInfo`.
fn vs_score(r: &Value, uid: String) -> VsScore {
    VsScore {
        uid,
        score: int(r, "score"),
        alliance_id: string(r, "aid"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(entries: Vec<(&str, Value)>) -> Value {
        Value::Object(
            entries
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    fn message(command: &str, data: Value, secs: u64) -> Message {
        Message::command_for_test(command, data, secs)
    }

    /// Friday 2026-10-02 20:00 UTC.
    const FRIDAY: u64 = 1_790_971_200;
    const DAY: u64 = 86_400;
    /// Saturday 2026-10-03 02:00 UTC, when sign-ups close.
    const SATURDAY: u64 = FRIDAY - 18 * 3600 + DAY;
    /// Monday 2026-10-05 02:00 UTC, the weekly reset.
    const MONDAY: u64 = SATURDAY + 2 * DAY;

    fn member_entry(uid: &str, name: &str, rank: i32, power: i64) -> Value {
        obj(vec![
            ("uid", Value::Str(uid.into())),
            ("name", Value::Str(name.into())),
            ("rank", Value::Int(rank)),
            ("power", Value::Long(power)),
            ("armyKill", Value::Int(7)),
        ])
    }

    fn participant_entry(uid: &str, group: i32) -> Value {
        obj(vec![
            ("uid", Value::Str(uid.into())),
            ("name", Value::Str("Ann".into())),
            ("heroPower", Value::Long(60)),
            ("power", Value::Long(110)),
            (
                "chooseTimeList",
                Value::Array(vec![Value::Int(2), Value::Int(1)]),
            ),
            ("group", Value::Int(group)),
        ])
    }

    fn participants(entries: Vec<Value>) -> Value {
        obj(vec![("users", Value::Array(entries))])
    }

    /// An `al.rank` member list of `alliance` listing `members`.
    fn roster(alliance: &str, members: Vec<Value>) -> Value {
        obj(vec![
            ("allianceId", Value::Str(alliance.into())),
            ("list", Value::Array(members)),
        ])
    }

    fn ranking(day: i32, scores: &[(&str, i32, &str)]) -> Value {
        let list = scores
            .iter()
            .map(|(uid, score, aid)| {
                obj(vec![
                    ("uid", Value::Str((*uid).into())),
                    ("score", Value::Int(*score)),
                    ("aid", Value::Str((*aid).into())),
                ])
            })
            .collect();
        obj(vec![
            ("day", Value::Int(day)),
            ("rankInfo", Value::Array(list)),
        ])
    }

    fn battle(time: u64, won: bool, players: &[(&str, i64)]) -> DsBattle {
        DsBattle {
            time: Duration::from_secs(time),
            won,
            alliance_id: "ours".into(),
            score: 1,
            enemy_abbr: "X".into(),
            enemy_name: "X".into(),
            enemy_score: 2,
            players: players.iter().map(|(u, s)| (u.to_string(), *s)).collect(),
            players_complete: true,
        }
    }

    /// A view holding alliance "ours" with one member, Ann (uid 1).
    fn view_with_ann() -> View {
        let mut view = View::default();
        let members = roster("ours", vec![member_entry("1", "Ann", 4, 100)]);
        view.apply(&message("al.rank", members, FRIDAY));
        view
    }

    fn uids<T>(panel: &Option<Panel<T>>, uid: impl Fn(&T) -> &str) -> Vec<&str> {
        panel
            .as_ref()
            .map(|p| p.entries.iter().map(uid).collect())
            .unwrap_or_default()
    }

    fn roster_uids(view: &View) -> Vec<&str> {
        uids(&view.roster, |m| m.uid.as_str())
    }

    #[test]
    fn panels_are_kept_apart_as_sent() {
        let mut view = view_with_ann();
        let users = participants(vec![participant_entry("1", 2)]);
        assert!(view.apply(&message("dragon.assign.player.info", users, FRIDAY + 1)));

        let roster = view.roster.as_ref().unwrap();
        assert_eq!(roster.time, Duration::from_secs(FRIDAY));
        assert!(roster.complete);
        assert_eq!(
            roster.entries,
            [Member {
                uid: "1".into(),
                name: Some("Ann".into()),
                rank: Some(4),
                power: Some(100),
                army_kill: Some(7),
                server_id: None,
            }]
        );
        let signups = view.ds_signups.as_ref().unwrap();
        assert_eq!(signups.time, Duration::from_secs(FRIDAY + 1));
        assert_eq!(
            signups.entries,
            [Participant {
                uid: "1".into(),
                hero_power: Some(60),
                choose_time_list: Some(vec![2, 1]),
                group: Some(2),
            }]
        );
        assert_eq!(
            view.roster.as_ref().unwrap().entries[0].power,
            Some(100),
            "the member list's power is the one kept"
        );
    }

    #[test]
    fn the_newest_list_replaces_the_last() {
        let mut view = View::default();
        let both = roster(
            "ours",
            vec![
                member_entry("1", "Ann", 4, 100),
                member_entry("2", "Bob", 3, 90),
            ],
        );
        view.apply(&message("al.rank", both, FRIDAY));
        let one = roster("ours", vec![member_entry("1", "Ann", 5, 100)]);
        view.apply(&message("al.rank", one, FRIDAY + 1));
        assert_eq!(roster_uids(&view), ["1"]);
        assert_eq!(view.roster.as_ref().unwrap().entries[0].rank, Some(5));
    }

    /// A member of server `server`, sent as `value`.
    fn on_server(uid: &str, value: Value) -> Value {
        obj(vec![("uid", Value::Str(uid.into())), ("serverId", value)])
    }

    #[test]
    fn warzone_from_the_members_server() {
        let mut view = View::default();
        assert_eq!(view.warzone(), None);
        // Sent as a number or as text; a member without one doesn't count.
        let members = vec![
            on_server("1", Value::Int(901)),
            on_server("2", Value::Str("901".into())),
            member_entry("3", "Cat", 1, 1),
        ];
        view.apply(&message("al.rank", roster("ours", members), FRIDAY));
        assert_eq!(view.warzone(), Some(901));

        // Members on different servers give no single warzone.
        let mixed = vec![
            on_server("1", Value::Int(901)),
            on_server("2", Value::Int(902)),
        ];
        view.apply(&message("al.rank", roster("ours", mixed), FRIDAY + 1));
        assert_eq!(view.warzone(), None);
    }

    #[test]
    fn an_unreadable_entry_marks_the_list_incomplete() {
        let mut view = View::default();
        let unreadable = obj(vec![("name", Value::Str("Bob".into()))]);
        let partial = roster("ours", vec![member_entry("1", "Ann", 5, 100), unreadable]);
        view.apply(&message("al.rank", partial, FRIDAY));
        let roster = view.roster.as_ref().unwrap();
        assert_eq!(roster.entries.len(), 1);
        assert!(!roster.complete);
    }

    #[test]
    fn empty_list_changes_nothing() {
        let mut view = view_with_ann();
        assert!(!view.apply(&message("al.rank", obj(vec![]), FRIDAY + 1)));
        assert!(!view.apply(&message(
            "dragon.assign.player.info",
            participants(vec![]),
            FRIDAY + 1
        )));
        assert_eq!(
            view.roster.as_ref().unwrap().time,
            Duration::from_secs(FRIDAY),
            "an empty list is not an update"
        );
        assert_eq!(view.ds_signups, None);
    }

    #[test]
    fn ignores_other_commands() {
        let mut view = View::default();
        assert!(!view.apply(&message("push.al.sign", obj(vec![]), FRIDAY)));
        assert_eq!(view.roster, None);
    }

    #[test]
    fn ds_signups_close_from_saturday_to_the_reset() {
        let mut view = view_with_ann();
        let users = || participants(vec![participant_entry("1", 1)]);
        view.apply(&message("dragon.assign.player.info", users(), FRIDAY));
        assert!(view.ds_signups.is_some());

        // Saturday 02:00 UTC passes on the window's clock: Friday's sign-ups go.
        assert!(!view.roll_vs_week(Duration::from_secs(SATURDAY - 1)));
        assert!(view.roll_vs_week(Duration::from_secs(SATURDAY)));
        assert_eq!(view.ds_signups, None);
        // The weekend's lists are ignored.
        assert!(!view.apply(&message(
            "dragon.assign.player.info",
            users(),
            SATURDAY + 60
        )));
        assert_eq!(view.ds_signups, None);
        assert!(view.roster.is_some(), "the member list stays");

        // After the Monday reset, sign-ups are read again.
        assert!(view.apply(&message("dragon.assign.player.info", users(), MONDAY)));
        assert!(view.ds_signups.is_some());
    }

    #[test]
    fn a_delayed_weekend_message_keeps_the_new_weeks_sign_ups() {
        let mut view = view_with_ann();
        let users = participants(vec![participant_entry("1", 1)]);
        assert!(view.apply(&message("dragon.assign.player.info", users, MONDAY + 60)));
        // A message captured on Sunday is handled after Monday's sign-ups.
        let sunday = MONDAY - 3600;
        assert!(!view.apply(&message("push.al.sign", obj(vec![]), sunday)));
        assert!(!view.roll_vs_week(Duration::from_secs(sunday)));
        assert_eq!(
            view.ds_signups.as_ref().map(|p| p.time),
            Some(Duration::from_secs(MONDAY + 60))
        );
    }

    #[test]
    fn vs_rankings_for_completed_days() {
        let mut view = view_with_ann();
        let thursday = ranking(4, &[("1", 300, "ours"), ("opponent", 999, "theirs")]);
        assert!(view.apply(&message("al.battle.rank.info", thursday, FRIDAY)));
        let today = ranking(5, &[("1", 50, "ours")]);
        assert!(
            !view.apply(&message("al.battle.rank.info", today, FRIDAY)),
            "current day skipped"
        );
        let empty = ranking(3, &[]);
        assert!(!view.apply(&message("al.battle.rank.info", empty, FRIDAY)));

        let loaded: Vec<bool> = view.vs_days.iter().map(Option::is_some).collect();
        assert_eq!(loaded, [false, false, false, true, false, false]);
        let thursday = view.vs_days[3].as_ref().unwrap();
        assert_eq!(
            thursday.entries,
            [
                VsScore {
                    uid: "1".into(),
                    score: Some(300),
                    alliance_id: Some("ours".into()),
                },
                VsScore {
                    uid: "opponent".into(),
                    score: Some(999),
                    alliance_id: Some("theirs".into()),
                },
            ],
            "both alliances, as the ranking lists them; the payload keeps ours"
        );
    }

    #[test]
    fn weekly_reset_clears_the_weeks_data() {
        let mut view = view_with_ann();
        view.apply(&message(
            "dragon.assign.player.info",
            participants(vec![participant_entry("1", 1)]),
            FRIDAY,
        ));
        view.apply(&message(
            "al.battle.rank.info",
            ranking(4, &[("1", 300, "ours")]),
            FRIDAY,
        ));
        let battles = || vec![battle(FRIDAY - 9 * 3600, true, &[("1", 50)])];
        assert_eq!(
            view.set_ds_battles(battles(), Duration::from_secs(FRIDAY)),
            1
        );

        // Monday 02:00 UTC passes on the window's clock, with no game messages.
        view.roll_vs_week(Duration::from_secs(MONDAY - 1));
        assert!(view.ds_battles().count() == 1 && view.vs_days[3].is_some());
        assert!(view.roll_vs_week(Duration::from_secs(MONDAY)));
        assert_eq!(view.ds_battles().count(), 0);
        assert_eq!(view.ds_signups, None);
        assert!(view.vs_days.iter().all(Option::is_none));
        assert_eq!(roster_uids(&view), ["1"], "the member list stays");

        // Loading mail in the new week finds only last week's battles: nothing is kept.
        assert_eq!(
            view.set_ds_battles(battles(), Duration::from_secs(MONDAY + 60)),
            0
        );
    }

    #[test]
    fn messages_from_before_the_reset_are_ignored() {
        let mut view = view_with_ann();
        // The window's clock runs the reset first; then a ranking captured on Sunday is handled.
        view.roll_vs_week(Duration::from_secs(MONDAY));
        assert!(!view.apply(&message(
            "al.battle.rank.info",
            ranking(4, &[("1", 300, "ours")]),
            MONDAY - 3600
        )));
        assert!(view.vs_days.iter().all(Option::is_none));
    }

    #[test]
    fn ds_battles_of_this_week_and_our_alliance() {
        let mut view = view_with_ann();
        let mut theirs = battle(FRIDAY, true, &[("x", 5)]);
        theirs.alliance_id = "theirs".into();
        let kept = view.set_ds_battles(
            vec![
                battle(FRIDAY - 9 * 3600, true, &[("1", 50)]),
                battle(FRIDAY, false, &[("2", 30)]),
                battle(FRIDAY - 7 * DAY, true, &[("1", 99)]),
                theirs,
            ],
            Duration::from_secs(SATURDAY + DAY),
        );
        assert_eq!(kept, 3, "last week's battle is dropped");
        let won: Vec<bool> = view.ds_battles().map(|b| b.won).collect();
        assert_eq!(won, [true, false], "another alliance's battle is left out");
    }

    #[test]
    fn battles_count_only_once_our_alliance_is_known() {
        let mut view = View::default();
        // Player 1 fought in another alliance's battle (say, before moving to ours).
        let mut other = battle(FRIDAY, true, &[("1", 50)]);
        other.alliance_id = "other alliance".into();
        let ours = battle(FRIDAY, false, &[("1", 30)]);
        view.set_ds_battles(vec![other, ours], Duration::from_secs(FRIDAY));
        assert_eq!(view.ds_battles().count(), 0, "alliance unknown: none count");
        assert_eq!(view.unattributed_battles(), 2);

        // The member list names the alliance.
        let members = roster("ours", vec![member_entry("1", "Ann", 4, 100)]);
        view.apply(&message("al.rank", members, FRIDAY));
        let battles: Vec<_> = view.ds_battles().map(|b| b.alliance_id.as_str()).collect();
        assert_eq!(battles, ["ours"]);
        assert_eq!(view.unattributed_battles(), 0);
    }

    fn gold_tree(uid: &str) -> Value {
        obj(vec![(
            "userGoldTreeDataInfo",
            obj(vec![(
                "userGoldTeeInfo",
                obj(vec![("uid", Value::Str(uid.into()))]),
            )]),
        )])
    }

    /// A player profile in alliance "ours".
    fn profile(uid: &str, name: &str) -> Value {
        profile_in(uid, name, "ours", "Example Alliance", "EXA")
    }

    fn profile_in(uid: &str, name: &str, alliance: &str, alliance_name: &str, abbr: &str) -> Value {
        obj(vec![
            ("uid", Value::Str(uid.into())),
            ("name", Value::Str(name.into())),
            ("allianceId", Value::Str(alliance.into())),
            ("allianceName", Value::Str(alliance_name.into())),
            ("abbr", Value::Str(abbr.into())),
        ])
    }

    #[test]
    fn identifies_account_from_own_messages_only() {
        let mut view = view_with_ann();
        assert_eq!(view.account(), None);
        view.apply(&message(
            "get.new.user.info",
            profile("other", "Bob"),
            FRIDAY,
        ));
        assert_eq!(
            view.account(),
            None,
            "a profile alone does not say whose account this is"
        );
        view.apply(&message("gold.tree.act.view", gold_tree("me"), FRIDAY));
        view.apply(&message(
            "get.new.user.info",
            profile("other", "Bob"),
            FRIDAY,
        ));
        view.apply(&message(
            "get.new.user.info",
            profile("me", "Player One"),
            FRIDAY,
        ));
        let account = view.account().unwrap();
        assert_eq!(account.uid, "me");
        assert_eq!(account.name.as_deref(), Some("Player One"));
        assert_eq!(account.alliance_abbr.as_deref(), Some("EXA"));
        assert_eq!(
            roster_uids(&view),
            ["1"],
            "identifying the first account keeps the data"
        );
    }

    #[test]
    fn switching_account_clears_everything() {
        let mut view = view_with_ann();
        view.apply(&message("gold.tree.act.view", gold_tree("me"), FRIDAY));
        view.apply(&message(
            "al.battle.rank.info",
            ranking(4, &[("1", 300, "ours")]),
            FRIDAY,
        ));
        view.set_ds_battles(
            vec![battle(FRIDAY, true, &[("1", 5)])],
            Duration::from_secs(FRIDAY),
        );
        assert!(view.apply(&message(
            "push.mail",
            obj(vec![("toUser", Value::Str("alt".into()))]),
            FRIDAY
        )));
        assert_eq!(view.account().map(|a| a.uid.as_str()), Some("alt"));
        assert_eq!(view.roster, None);
        assert!(view.vs_days.iter().all(Option::is_none));
        assert_eq!(view.ds_battles().count(), 0);
        assert_eq!(
            view.alliance_id(),
            None,
            "the old roster's alliance is gone"
        );
    }

    #[test]
    fn another_alliances_member_list_is_ignored_once_ours_is_known() {
        let mut view = view_with_ann();
        view.apply(&message("gold.tree.act.view", gold_tree("me"), FRIDAY));
        view.apply(&message(
            "get.new.user.info",
            profile("me", "Player One"),
            FRIDAY,
        ));
        let foreign = roster("theirs", vec![member_entry("9", "Zed", 5, 999)]);
        assert!(!view.apply(&message("al.rank", foreign, FRIDAY + 60)));
        assert_eq!(roster_uids(&view), ["1"]);
        assert_eq!(
            view.roster.as_ref().unwrap().time,
            Duration::from_secs(FRIDAY)
        );
    }

    #[test]
    fn a_new_roster_alliance_replaces_the_old_one_while_the_account_is_unknown() {
        let mut view = view_with_ann();
        view.set_ds_battles(
            vec![battle(FRIDAY, true, &[("1", 5)])],
            Duration::from_secs(FRIDAY),
        );
        let other = roster("theirs", vec![member_entry("9", "Zed", 5, 999)]);
        assert!(view.apply(&message("al.rank", other, FRIDAY + 60)));
        assert_eq!(view.alliance_id(), Some("theirs"));
        assert_eq!(roster_uids(&view), ["9"]);
        assert_eq!(view.ds_battles().count(), 0);
        assert_eq!(view.unattributed_battles(), 0, "the old load is dropped");
    }

    #[test]
    fn changing_alliance_clears_the_old_alliances_data() {
        let mut view = view_with_ann();
        view.apply(&message("gold.tree.act.view", gold_tree("me"), FRIDAY));
        view.apply(&message(
            "get.new.user.info",
            profile("me", "Player One"),
            FRIDAY,
        ));
        view.apply(&message(
            "al.battle.rank.info",
            ranking(4, &[("1", 300, "ours")]),
            FRIDAY,
        ));
        view.apply(&message(
            "dragon.assign.player.info",
            participants(vec![participant_entry("1", 1)]),
            FRIDAY,
        ));
        view.set_ds_battles(
            vec![battle(FRIDAY, true, &[("1", 5)])],
            Duration::from_secs(FRIDAY),
        );
        assert_eq!(view.ds_battles().count(), 1);

        // Same account, now in another alliance.
        let moved = profile_in("me", "Player One", "new", "New Alliance", "NEW");
        assert!(view.apply(&message("get.new.user.info", moved, FRIDAY + 60)));
        let account = view.account().unwrap();
        assert_eq!(account.uid, "me");
        assert_eq!(
            (
                account.alliance_id.as_deref(),
                account.alliance_name.as_deref(),
                account.alliance_abbr.as_deref()
            ),
            (Some("new"), Some("New Alliance"), Some("NEW"))
        );
        assert_eq!(view.roster, None);
        assert_eq!(view.ds_signups, None);
        assert!(view.vs_days.iter().all(Option::is_none));
        assert_eq!(view.ds_battles().count(), 0);
        assert_eq!(view.unattributed_battles(), 0);

        // The old alliance's list no longer applies; the new one's does.
        let old = roster("ours", vec![member_entry("1", "Ann", 4, 100)]);
        assert!(!view.apply(&message("al.rank", old, FRIDAY + 120)));
        let new = roster("new", vec![member_entry("1", "Ann", 1, 100)]);
        assert!(view.apply(&message("al.rank", new, FRIDAY + 120)));
        assert_eq!(roster_uids(&view), ["1"]);
    }

    #[test]
    fn a_new_alliance_without_a_name_does_not_keep_the_old_name() {
        let mut view = View::default();
        view.apply(&message("gold.tree.act.view", gold_tree("me"), FRIDAY));
        view.apply(&message(
            "get.new.user.info",
            profile("me", "Player One"),
            FRIDAY,
        ));
        let bare = obj(vec![
            ("uid", Value::Str("me".into())),
            ("allianceId", Value::Str("new".into())),
        ]);
        view.apply(&message("get.new.user.info", bare, FRIDAY));
        let account = view.account().unwrap();
        assert_eq!(account.alliance_name, None);
        assert_eq!(account.alliance_abbr, None);
        assert_eq!(account.name.as_deref(), Some("Player One"));
    }
}
