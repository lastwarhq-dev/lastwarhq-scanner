//! The current view: one record per player, merged from every panel message seen.
//! Each message updates only the fields it carries; the newest value wins.
//! The member and participant panels list the whole alliance, so players missing from the
//! newest of those lists have left.
//!
//! The view belongs to one account, one alliance and one VS week. When any of them changes,
//! [`View::clear`] drops what belonged to the old one:
//!
//! | Change | Cleared |
//! |---|---|
//! | VS week | VS scores, Desert Storm results, sign-ups and team assignments |
//! | Alliance | the above, and every player record |
//! | Account | everything, including the account itself |

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use crate::game::account::{Account, own_uid};
use crate::game::battle::DsBattle;
use crate::game::player::{DsResult, Player};
use crate::game::week::vs_day;
use crate::protocol::message::Message;
use crate::protocol::sfs::Value;

#[derive(Debug, Default)]
pub struct View {
    account: Option<Account>,
    /// The alliance whose member list the roster came from. Used as our alliance until the
    /// account's own messages name it.
    roster_alliance: Option<String>,
    players: BTreeMap<String, Player>,
    /// VS week the stored `vs_scores` belong to.
    vs_week: Option<u64>,
    /// Every VS score seen this week by uid, so a ranking opened before the member list still
    /// counts. Opponents are kept here too but never become players.
    vs_seen: HashMap<String, [Option<i64>; 6]>,
    /// This week's Desert Storm battles, from the last mail load.
    ds_battles: Vec<DsBattle>,
    /// When each part of the view was last updated (capture time).
    pub roster_at: Option<Duration>,
    pub signups_at: Option<Duration>,
    pub vs_at: Option<Duration>,
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
    /// Merges a panel message into the view. Returns false for messages that change nothing.
    pub fn apply(&mut self, message: &Message) -> bool {
        let new_week = self.roll_vs_week(message.time);
        // A message captured before the weekly reset but handled after it (the window's clock
        // can run the reset first) belongs to the week that has ended.
        if self.vs_week.is_some_and(|w| vs_day(message.time).0 < w) {
            return new_week;
        }
        let Some(data) = message.data() else {
            return new_week;
        };
        let command = message.command().unwrap_or_default();
        let changed = self.note_account(command, data) || new_week;
        let (list, stamp, changed) = match command {
            "al.rank" => {
                let list = alliance_members(data);
                // An empty list means the message was not understood; it says nothing about
                // which alliance this is.
                if list.players.is_empty() {
                    return changed;
                }
                match self.note_roster_alliance(data) {
                    Some(moved) => (list, &mut self.roster_at, changed || moved),
                    None => return changed,
                }
            }
            "dragon.assign.player.info" => (
                desert_storm_participants(data),
                &mut self.signups_at,
                changed,
            ),
            "al.battle.rank.info" => return self.apply_vs(data, message.time) || changed,
            _ => return changed,
        };
        if !list.players.is_empty() {
            *stamp = Some(message.time);
        }
        self.merge_list(list) || changed
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

    /// Drops what belonged to the old week, alliance or account. The only place the view's
    /// data is cleared.
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
        self.vs_seen.clear();
        self.ds_battles.clear();
        self.vs_at = None;
        self.signups_at = None;
        for player in self.players.values_mut() {
            player.vs_scores = Default::default();
            player.choose_time_list = None;
            player.ds_group = None;
        }
        if let Scope::Alliance = scope {
            self.players.clear();
            self.roster_alliance = None;
            self.roster_at = None;
        }
    }

    /// Tracks the logged-in account. A different uid means the user switched accounts: every
    /// player, score and result belongs to the old account, so the view starts again. A
    /// different alliance on the same account clears the old alliance's data.
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

    /// Merges a whole-alliance list (member list or participants). Players it no longer lists
    /// are dropped, but only when every entry could be read: an unreadable entry might be one
    /// of the players who would otherwise be removed.
    fn merge_list(&mut self, list: AllianceList) -> bool {
        let AllianceList {
            players: updates,
            complete,
        } = list;
        // An empty list means the message was not understood, not that the alliance is empty.
        if updates.is_empty() {
            return false;
        }
        if complete {
            let listed: HashSet<&str> = updates.iter().map(|u| u.uid.as_str()).collect();
            self.players.retain(|uid, _| listed.contains(uid.as_str()));
        }
        for update in updates {
            let vs_seen = &self.vs_seen;
            self.players
                .entry(update.uid.clone())
                .or_insert_with(|| Player {
                    uid: update.uid.clone(),
                    vs_scores: vs_seen.get(&update.uid).copied().unwrap_or_default(),
                    ..Player::default()
                })
                .merge(update);
        }
        true
    }

    /// Clears everything that belongs to one VS week once `time` falls in a later week: VS
    /// scores, Desert Storm results, sign-up time slots and team assignments. The roster (rank,
    /// power, kills) stays. Called for every message and, from the window's clock, every second,
    /// so the reset happens even while the game is closed.
    pub fn roll_vs_week(&mut self, time: Duration) -> bool {
        let (week, _) = vs_day(time);
        if self.vs_week.is_some_and(|w| w >= week) {
            return false;
        }
        let previous = self.vs_week.replace(week);
        if previous.is_none() {
            return false;
        }
        self.clear(Scope::Week);
        true
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

    /// This week's battles fought by our alliance, with the team each was fought by. The mail
    /// database holds every account's mail, so it can hold other alliances' battles too; a
    /// battle counts only when it belongs to our alliance. Until our alliance is known, none
    /// do: a player's past battle for another alliance must not pass as ours.
    pub fn ds_battles(&self) -> impl Iterator<Item = (&DsBattle, Option<i64>)> {
        let alliance = self.alliance_id();
        self.ds_battles
            .iter()
            .filter(move |b| alliance == Some(b.alliance_id.as_str()))
            .map(|b| (b, self.battle_team(b)))
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

    /// The team whose assigned players make up most of a battle's list.
    fn battle_team(&self, battle: &DsBattle) -> Option<i64> {
        let mut counts: HashMap<i64, usize> = HashMap::new();
        for (uid, _) in &battle.players {
            if let Some(group) = self
                .players
                .get(uid)
                .and_then(|p| p.ds_group)
                .filter(|&g| g > 0)
            {
                *counts.entry(group).or_default() += 1;
            }
        }
        counts
            .into_iter()
            .max_by_key(|&(group, n)| (n, std::cmp::Reverse(group)))
            .map(|(g, _)| g)
    }

    /// The player's Desert Storm result this week: from the battle they are listed in, or else
    /// as absent from their assigned team's battle. `None` when there's no result, or when a
    /// battle's player list couldn't be fully read and absence can't be told.
    pub fn ds_result(&self, player: &Player) -> Option<DsResult> {
        let battles: Vec<(&DsBattle, Option<i64>)> = self.ds_battles().collect();
        for &(battle, team) in &battles {
            if let Some(&(_, score)) = battle.players.iter().find(|(uid, _)| *uid == player.uid) {
                return Some(DsResult {
                    team,
                    attended: score > 0,
                    score: Some(score),
                    won: battle.won,
                });
            }
        }
        // Absence is only concluded from complete lists: an entry that couldn't be read may be
        // this player, in either battle.
        if !battles.iter().all(|(battle, _)| battle.players_complete) {
            return None;
        }
        let assigned = player.ds_group.filter(|&g| g > 0)?;
        battles
            .iter()
            .find(|(_, team)| *team == Some(assigned))
            .map(|&(battle, team)| DsResult {
                team,
                attended: false,
                score: None,
                won: battle.won,
            })
    }

    /// `al.battle.rank.info`: one day's VS ranking for both alliances. Scores are kept by uid
    /// and shown only on players in the view, which leaves out the opponent. The current day is
    /// skipped because its scores keep changing until the reset.
    fn apply_vs(&mut self, data: &Value, time: Duration) -> bool {
        let Some(day) = int(data, "day") else {
            return false;
        };
        let (_, today) = vs_day(time);
        if !(1..=6).contains(&day) || day >= today {
            return false;
        }
        let slot = (day - 1) as usize;
        let mut recorded = false;
        let mut changed = false;
        for (entry, uid) in entries(data, "rankInfo") {
            let Some(score) = int(entry, "score") else {
                continue;
            };
            if let Some(player) = self.players.get_mut(&uid) {
                changed |= player.vs_scores[slot] != Some(score);
                player.vs_scores[slot] = Some(score);
            }
            let seen = &mut self.vs_seen.entry(uid).or_default()[slot];
            changed |= *seen != Some(score);
            *seen = Some(score);
            recorded = true;
        }
        // Stamped whenever a ranking was read, even before the roster has arrived to show it.
        if recorded {
            self.vs_at = Some(time);
        }
        changed
    }

    /// Days (0 = Monday) for which any player in the view has a VS score.
    pub fn vs_days_loaded(&self) -> [bool; 6] {
        let mut days = [false; 6];
        for player in self.players.values() {
            for (day, score) in player.vs_scores.iter().enumerate() {
                days[day] |= score.is_some();
            }
        }
        days
    }

    pub fn players(&self) -> impl Iterator<Item = &Player> {
        self.players.values()
    }

    pub fn get(&self, uid: &str) -> Option<&Player> {
        self.players.get(uid)
    }

    /// A JSON array of all players, one per line.
    pub fn to_json(&self) -> String {
        let rows: Vec<String> = self
            .players()
            .map(|p| p.to_json(self.ds_result(p)))
            .collect();
        format!("[\n{}\n]", rows.join(",\n"))
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

/// A whole-alliance list from one message.
struct AllianceList {
    players: Vec<Player>,
    /// Every entry had a readable uid.
    complete: bool,
}

/// Reads the array at `key` with `player`, noting whether any entry was unreadable.
fn alliance_list(
    data: &Value,
    key: &str,
    player: impl Fn(&Value, String) -> Player,
) -> AllianceList {
    let total = data
        .get(key)
        .and_then(Value::as_array)
        .map_or(0, <[Value]>::len);
    let players: Vec<Player> = entries(data, key)
        .map(|(entry, uid)| player(entry, uid))
        .collect();
    AllianceList {
        complete: players.len() == total,
        players,
    }
}

/// `al.rank`: the alliance member list.
fn alliance_members(data: &Value) -> AllianceList {
    alliance_list(data, "list", |m, uid| Player {
        uid,
        name: string(m, "name"),
        rank: int(m, "rank"),
        power: int(m, "power"),
        army_kill: int(m, "armyKill"),
        ..Player::default()
    })
}

/// `dragon.assign.player.info`: the Desert Storm participants panel.
fn desert_storm_participants(data: &Value) -> AllianceList {
    alliance_list(data, "users", |u, uid| Player {
        uid,
        name: string(u, "name"),
        power: int(u, "power"),
        hero_power: int(u, "heroPower"),
        choose_time_list: int_list(u, "chooseTimeList"),
        ds_group: int(u, "group"),
        ..Player::default()
    })
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

    fn member(uid: &str, name: &str, rank: i32, power: i64) -> Value {
        obj(vec![
            ("uid", Value::Str(uid.into())),
            ("name", Value::Str(name.into())),
            ("rank", Value::Int(rank)),
            ("power", Value::Long(power)),
            ("armyKill", Value::Int(7)),
        ])
    }

    fn participant(uid: &str, name: &str, hero: i64, power: i64) -> Value {
        obj(vec![
            ("uid", Value::Str(uid.into())),
            ("name", Value::Str(name.into())),
            ("heroPower", Value::Long(hero)),
            ("power", Value::Long(power)),
            (
                "chooseTimeList",
                Value::Array(vec![Value::Int(2), Value::Int(1)]),
            ),
        ])
    }

    #[test]
    fn merges_fields_from_both_panels() {
        let mut view = View::default();
        let members = obj(vec![(
            "list",
            Value::Array(vec![member("1", "Ann", 4, 100)]),
        )]);
        let users = obj(vec![(
            "users",
            Value::Array(vec![participant("1", "Ann", 60, 110)]),
        )]);
        assert!(view.apply(&message("al.rank", members, 1)));
        assert!(view.apply(&message("dragon.assign.player.info", users, 2)));

        let p = view.get("1").unwrap();
        assert_eq!(p.rank, Some(4));
        assert_eq!(p.army_kill, Some(7));
        assert_eq!(p.hero_power, Some(60));
        assert_eq!(p.power, Some(110), "newest message wins");
        assert_eq!(p.choose_time_list, Some(vec![2, 1]));
        assert_eq!(view.roster_at, Some(Duration::from_secs(1)));
        assert_eq!(view.signups_at, Some(Duration::from_secs(2)));
    }

    #[test]
    fn drops_players_missing_from_newer_list() {
        let mut view = View::default();
        let both = obj(vec![(
            "list",
            Value::Array(vec![member("1", "Ann", 4, 100), member("2", "Bob", 3, 90)]),
        )]);
        let one = obj(vec![(
            "users",
            Value::Array(vec![participant("1", "Ann", 60, 110)]),
        )]);
        view.apply(&message("al.rank", both, 1));
        view.apply(&message("dragon.assign.player.info", one, 2));
        let uids: Vec<&str> = view.players().map(|p| p.uid.as_str()).collect();
        assert_eq!(uids, ["1"]);
        assert_eq!(
            view.get("1").unwrap().rank,
            Some(4),
            "kept players keep their fields"
        );
    }

    #[test]
    fn empty_list_changes_nothing() {
        let mut view = View::default();
        let members = obj(vec![(
            "list",
            Value::Array(vec![member("1", "Ann", 4, 100)]),
        )]);
        view.apply(&message("al.rank", members, 1));
        assert!(!view.apply(&message("al.rank", obj(vec![]), 2)));
        assert_eq!(view.players().count(), 1);
        assert_eq!(
            view.roster_at,
            Some(Duration::from_secs(1)),
            "an empty list is not an update"
        );
    }

    #[test]
    fn ignores_other_commands() {
        let mut view = View::default();
        assert!(!view.apply(&message("push.al.sign", obj(vec![]), 1)));
        assert_eq!(view.players().count(), 0);
    }

    fn assigned(uid: &str, group: i64) -> Value {
        obj(vec![
            ("uid", Value::Str(uid.into())),
            ("group", Value::Int(group as i32)),
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

    #[test]
    fn ds_results_from_this_weeks_battles() {
        let mut view = View::default();
        let members = ["a", "b", "c", "d", "e"].map(|uid| member(uid, uid, 1, 1));
        view.apply(&message(
            "al.rank",
            roster("ours", members.to_vec()),
            FRIDAY,
        ));
        let users = vec![
            assigned("a", 1),
            assigned("b", 1),
            assigned("c", 2),
            assigned("d", 2),
            assigned("e", 0),
        ];
        view.apply(&message(
            "dragon.assign.player.info",
            obj(vec![("users", Value::Array(users))]),
            FRIDAY,
        ));
        let morning = FRIDAY - 9 * 3600;
        let last_week = FRIDAY - 7 * DAY;
        let mut theirs = battle(FRIDAY, true, &[("x", 5)]);
        theirs.alliance_id = "theirs".into();
        let kept = view.set_ds_battles(
            vec![
                battle(morning, true, &[("c", 50), ("d", 0)]),
                battle(FRIDAY, false, &[("a", 30)]),
                battle(last_week, true, &[("a", 99)]),
                theirs,
            ],
            Duration::from_secs(FRIDAY + 2 * DAY),
        );
        assert_eq!(kept, 3, "last week's battle is dropped");
        assert_eq!(
            view.ds_battles().count(),
            2,
            "another alliance's battle is not shown"
        );

        let result = |uid: &str| view.ds_result(view.get(uid).unwrap());
        assert_eq!(
            result("a"),
            Some(DsResult {
                team: Some(1),
                attended: true,
                score: Some(30),
                won: false
            })
        );
        assert_eq!(
            result("b"),
            Some(DsResult {
                team: Some(1),
                attended: false,
                score: None,
                won: false
            })
        );
        assert_eq!(
            result("c"),
            Some(DsResult {
                team: Some(2),
                attended: true,
                score: Some(50),
                won: true
            })
        );
        assert_eq!(
            result("d"),
            Some(DsResult {
                team: Some(2),
                attended: false,
                score: Some(0),
                won: true
            })
        );
        assert_eq!(result("e"), None);
    }

    /// Friday 2026-10-02 20:00 UTC.
    const FRIDAY: u64 = 1_790_971_200;
    const DAY: u64 = 86_400;

    fn ranking(day: i32, scores: &[(&str, i32)]) -> Value {
        let list = scores
            .iter()
            .map(|(uid, score)| {
                obj(vec![
                    ("uid", Value::Str((*uid).into())),
                    ("score", Value::Int(*score)),
                ])
            })
            .collect();
        obj(vec![
            ("day", Value::Int(day)),
            ("rankInfo", Value::Array(list)),
        ])
    }

    /// An `al.rank` member list of `alliance` listing `members`.
    fn roster(alliance: &str, members: Vec<Value>) -> Value {
        obj(vec![
            ("allianceId", Value::Str(alliance.into())),
            ("list", Value::Array(members)),
        ])
    }

    /// A view holding alliance "ours" with one member, Ann (uid 1).
    fn view_with_ann() -> View {
        let mut view = View::default();
        let members = roster("ours", vec![member("1", "Ann", 4, 100)]);
        view.apply(&message("al.rank", members, FRIDAY));
        view
    }

    #[test]
    fn vs_scores_for_completed_days_of_known_players() {
        let mut view = view_with_ann();
        let thursday = ranking(4, &[("1", 300), ("opponent", 999)]);
        assert!(view.apply(&message("al.battle.rank.info", thursday, FRIDAY)));
        let today = ranking(5, &[("1", 50)]);
        assert!(
            !view.apply(&message("al.battle.rank.info", today, FRIDAY)),
            "current day skipped"
        );

        assert_eq!(
            view.get("1").unwrap().vs_scores,
            [None, None, None, Some(300), None, None]
        );
        assert!(view.get("opponent").is_none());
    }

    #[test]
    fn vs_scores_clear_in_a_new_week() {
        let mut view = view_with_ann();
        view.apply(&message(
            "al.battle.rank.info",
            ranking(4, &[("1", 300)]),
            FRIDAY,
        ));
        let next_tuesday = FRIDAY + 4 * DAY;
        assert!(view.apply(&message("push.al.sign", obj(vec![]), next_tuesday)));
        assert_eq!(view.get("1").unwrap().vs_scores, [None; 6]);
        view.apply(&message(
            "al.battle.rank.info",
            ranking(1, &[("1", 7)]),
            next_tuesday,
        ));
        assert_eq!(view.get("1").unwrap().vs_scores[0], Some(7));
    }

    #[test]
    fn weekly_reset_clears_all_desert_storm_data() {
        let mut view = view_with_ann();
        let users = vec![obj(vec![
            ("uid", Value::Str("1".into())),
            ("group", Value::Int(1)),
            ("chooseTimeList", Value::Array(vec![Value::Int(2)])),
        ])];
        view.apply(&message(
            "dragon.assign.player.info",
            obj(vec![("users", Value::Array(users))]),
            FRIDAY,
        ));
        let battles = || vec![battle(FRIDAY - 9 * 3600, true, &[("1", 50)])];
        view.set_ds_battles(battles(), Duration::from_secs(FRIDAY));
        assert!(view.ds_result(view.get("1").unwrap()).is_some());
        assert!(view.signups_at.is_some());

        // Monday 02:00 UTC passes on the window's clock, with no game messages.
        let monday = FRIDAY - 20 * 3600 + 3 * DAY + 7200;
        assert!(!view.roll_vs_week(Duration::from_secs(monday - 1)));
        assert!(view.roll_vs_week(Duration::from_secs(monday)));
        let p = view.get("1").unwrap();
        assert_eq!((p.choose_time_list.clone(), p.ds_group), (None, None));
        assert_eq!(view.ds_result(p), None);
        assert_eq!(view.signups_at, None);
        assert_eq!(p.rank, Some(4), "the roster stays");

        // Loading mail in the new week finds only last week's battles: nothing is kept.
        assert_eq!(
            view.set_ds_battles(battles(), Duration::from_secs(monday + 60)),
            0
        );
    }

    #[test]
    fn vs_ranking_before_member_list_still_counts() {
        let mut view = View::default();
        view.apply(&message(
            "al.battle.rank.info",
            ranking(3, &[("1", 30), ("opponent", 9)]),
            FRIDAY,
        ));
        assert_eq!(view.players().count(), 0);
        let members = roster("ours", vec![member("1", "Ann", 4, 100)]);
        view.apply(&message("al.rank", members, FRIDAY));
        assert_eq!(view.get("1").unwrap().vs_scores[2], Some(30));
        assert!(view.get("opponent").is_none());
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
            view.players().count(),
            1,
            "identifying the first account keeps the data"
        );
    }

    #[test]
    fn switching_account_clears_everything() {
        let mut view = view_with_ann();
        view.apply(&message("gold.tree.act.view", gold_tree("me"), FRIDAY));
        view.apply(&message(
            "al.battle.rank.info",
            ranking(4, &[("1", 300)]),
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
        assert_eq!(view.players().count(), 0);
        assert_eq!(view.ds_battles().count(), 0);
        assert_eq!(
            view.alliance_id(),
            None,
            "the old roster's alliance is gone"
        );
        // A ranking seen before the switch must not come back with the new account's members.
        let members = roster("ours", vec![member("1", "Ann", 4, 100)]);
        view.apply(&message("al.rank", members, FRIDAY));
        assert_eq!(view.get("1").unwrap().vs_scores, [None; 6]);
    }

    #[test]
    fn vs_ranking_does_not_drop_players() {
        let mut view = view_with_ann();
        view.apply(&message(
            "al.battle.rank.info",
            ranking(4, &[("other", 1)]),
            FRIDAY,
        ));
        assert!(view.get("1").is_some());
    }

    #[test]
    fn vs_ranking_before_the_roster_is_stamped_as_loaded() {
        let mut view = View::default();
        view.apply(&message(
            "al.battle.rank.info",
            ranking(3, &[("1", 30)]),
            FRIDAY,
        ));
        assert_eq!(view.vs_at, Some(Duration::from_secs(FRIDAY)));
    }

    #[test]
    fn messages_from_before_the_reset_are_ignored() {
        let mut view = view_with_ann();
        let monday = FRIDAY - 20 * 3600 + 3 * DAY + 7200;
        // The window's clock runs the reset first; then a ranking captured on Sunday is handled.
        view.roll_vs_week(Duration::from_secs(monday));
        let sunday = monday - 3600;
        assert!(!view.apply(&message(
            "al.battle.rank.info",
            ranking(4, &[("1", 300)]),
            sunday
        )));
        assert_eq!(view.get("1").unwrap().vs_scores, [None; 6]);
        assert_eq!(view.vs_at, None);
    }

    #[test]
    fn a_list_with_an_unreadable_entry_removes_nobody() {
        let mut view = View::default();
        let both = obj(vec![(
            "list",
            Value::Array(vec![member("1", "Ann", 4, 100), member("2", "Bob", 3, 90)]),
        )]);
        view.apply(&message("al.rank", both, FRIDAY));
        // Bob's entry arrives without a readable uid: he must not be treated as having left.
        let unreadable = obj(vec![("name", Value::Str("Bob".into()))]);
        let partial = obj(vec![(
            "list",
            Value::Array(vec![member("1", "Ann", 5, 100), unreadable]),
        )]);
        view.apply(&message("al.rank", partial, FRIDAY));
        assert!(view.get("2").is_some());
        assert_eq!(
            view.get("1").unwrap().rank,
            Some(5),
            "readable entries still update"
        );
    }

    #[test]
    fn nobody_is_judged_absent_from_a_partly_read_battle() {
        let mut view = View::default();
        let members = ["a", "b"].map(|uid| member(uid, uid, 1, 1));
        view.apply(&message(
            "al.rank",
            roster("ours", members.to_vec()),
            FRIDAY,
        ));
        view.apply(&message(
            "dragon.assign.player.info",
            obj(vec![(
                "users",
                Value::Array(vec![assigned("a", 1), assigned("b", 1)]),
            )]),
            FRIDAY,
        ));
        // Player b's entry in the mail couldn't be read.
        let mut partly = battle(FRIDAY, true, &[("a", 30)]);
        partly.players_complete = false;
        view.set_ds_battles(vec![partly], Duration::from_secs(FRIDAY));
        assert_eq!(
            view.ds_result(view.get("a").unwrap()).map(|r| r.score),
            Some(Some(30)),
            "listed players still have their result"
        );
        assert_eq!(view.ds_result(view.get("b").unwrap()), None);
    }

    #[test]
    fn battles_count_only_once_our_alliance_is_known() {
        // Only the participants panel so far: it doesn't say which alliance this is.
        let mut view = View::default();
        view.apply(&message(
            "dragon.assign.player.info",
            obj(vec![("users", Value::Array(vec![assigned("1", 1)]))]),
            FRIDAY,
        ));
        // Player 1 fought in another alliance's battle (say, before moving to ours).
        let mut other = battle(FRIDAY, true, &[("1", 50)]);
        other.alliance_id = "other alliance".into();
        let ours = battle(FRIDAY, false, &[("1", 30)]);
        view.set_ds_battles(vec![other, ours], Duration::from_secs(FRIDAY));
        assert_eq!(view.ds_battles().count(), 0, "alliance unknown: none count");
        assert_eq!(view.unattributed_battles(), 2);
        assert_eq!(view.ds_result(view.get("1").unwrap()), None);

        // The member list names the alliance.
        let members = roster("ours", vec![member("1", "Ann", 4, 100)]);
        view.apply(&message("al.rank", members, FRIDAY));
        let battles: Vec<_> = view
            .ds_battles()
            .map(|(b, _)| b.alliance_id.as_str())
            .collect();
        assert_eq!(battles, ["ours"]);
        assert_eq!(view.unattributed_battles(), 0);
        assert_eq!(
            view.ds_result(view.get("1").unwrap()).map(|r| r.score),
            Some(Some(30))
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
        let foreign = roster("theirs", vec![member("9", "Zed", 5, 999)]);
        assert!(!view.apply(&message("al.rank", foreign, FRIDAY + 60)));
        let uids: Vec<&str> = view.players().map(|p| p.uid.as_str()).collect();
        assert_eq!(uids, ["1"]);
        assert_eq!(view.roster_at, Some(Duration::from_secs(FRIDAY)));
    }

    #[test]
    fn a_new_roster_alliance_replaces_the_old_one_while_the_account_is_unknown() {
        let mut view = view_with_ann();
        view.set_ds_battles(
            vec![battle(FRIDAY, true, &[("1", 5)])],
            Duration::from_secs(FRIDAY),
        );
        let other = roster("theirs", vec![member("9", "Zed", 5, 999)]);
        assert!(view.apply(&message("al.rank", other, FRIDAY + 60)));
        assert_eq!(view.alliance_id(), Some("theirs"));
        let uids: Vec<&str> = view.players().map(|p| p.uid.as_str()).collect();
        assert_eq!(uids, ["9"]);
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
            ranking(4, &[("1", 300)]),
            FRIDAY,
        ));
        view.apply(&message(
            "dragon.assign.player.info",
            obj(vec![("users", Value::Array(vec![assigned("1", 1)]))]),
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
        assert_eq!(view.players().count(), 0);
        assert_eq!(view.ds_battles().count(), 0);
        assert_eq!(view.unattributed_battles(), 0);
        assert_eq!(
            (view.roster_at, view.signups_at, view.vs_at),
            (None, None, None)
        );

        // The old alliance's list no longer applies; the new one's does, without the old
        // alliance's VS scores.
        let old = roster("ours", vec![member("1", "Ann", 4, 100)]);
        assert!(!view.apply(&message("al.rank", old, FRIDAY + 120)));
        let new = roster("new", vec![member("1", "Ann", 1, 100)]);
        assert!(view.apply(&message("al.rank", new, FRIDAY + 120)));
        assert_eq!(view.get("1").unwrap().vs_scores, [None; 6]);
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
