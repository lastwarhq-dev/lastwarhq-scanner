//! What each game panel says about players, kept as the panel sent it. Every entry carries the
//! player's uid; joining panels together is left to whoever receives them.

use std::time::Duration;

use crate::util::json::escape;
use crate::util::time::utc_iso;

/// One panel's newest list.
#[derive(Debug, Clone, PartialEq)]
pub struct Panel<T> {
    /// When the list arrived: its capture time, on the game server's clock.
    pub time: Duration,
    /// Every entry had a readable uid. When false, a player missing from `entries` may still
    /// be on the panel.
    pub complete: bool,
    pub entries: Vec<T>,
}

/// A list entry, which names its player by uid.
pub trait Entry {
    fn uid(&self) -> &str;
}

impl<T: Entry> Panel<T> {
    /// `{"updated", "complete", "players": [...]}`, each entry through `entry`, cut to what the
    /// API takes (see [`players_for_api`]).
    pub fn to_json(&self, entry: impl Fn(&T) -> String) -> String {
        let (kept, complete) = players_for_api(&self.entries, |e| e.uid());
        let players: Vec<String> = kept.into_iter().map(entry).collect();
        format!(
            "{{\"updated\":{},\"complete\":{},\"players\":[{}]}}",
            escape(&utc_iso(self.time)),
            self.complete && complete,
            players.join(",")
        )
    }
}

/// The most players a list may hold.
pub const MAX_PLAYERS: usize = 200;

/// A player uid as the API takes it: the game's id, 1 to 20 digits.
pub fn valid_uid(uid: &str) -> bool {
    (1..=20).contains(&uid.len()) && uid.bytes().all(|b| b.is_ascii_digit())
}

/// A list's entries as the API takes them, which would refuse the whole upload otherwise:
/// entries whose uid isn't 1 to 20 digits, and any past the [`MAX_PLAYERS`]th, are left out,
/// and then the list can't be complete (`false` is returned); a player named again keeps only
/// their first entry.
pub fn players_for_api<T>(entries: &[T], uid: impl Fn(&T) -> &str) -> (Vec<&T>, bool) {
    let mut seen = std::collections::HashSet::new();
    let mut kept = Vec::new();
    let mut complete = true;
    for entry in entries {
        let id = uid(entry);
        if !valid_uid(id) {
            complete = false;
        } else if seen.contains(id) {
            // A repeat changes nothing, wherever it comes: the player is already in.
        } else if kept.len() == MAX_PLAYERS {
            complete = false;
        } else {
            seen.insert(id.to_string());
            kept.push(entry);
        }
    }
    (kept, complete)
}

/// The longest name the API takes, in characters.
pub const MAX_NAME: usize = 64;
/// The longest alliance tag the API takes, in characters.
pub const MAX_TAG: usize = 16;

/// Text from the game as the API takes it: without control characters, and 1 to `max`
/// characters long. Anything else would get the whole upload refused, so it is sent as
/// unknown (`null`).
pub fn game_text(value: Option<&str>, max: usize) -> Option<String> {
    let text: String = value?.chars().filter(|c| !c.is_control()).collect();
    let len = text.chars().count();
    (1..=max).contains(&len).then_some(text)
}

/// A power, kill count or score as the API takes it: 0 or more, else unknown.
pub fn count(value: Option<i64>) -> Option<i64> {
    value.filter(|v| *v >= 0)
}

/// A row of the alliance member list, `al.rank`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Member {
    pub uid: String,
    pub name: Option<String>,
    /// Alliance rank 1–5, R5 highest.
    pub rank: Option<i64>,
    pub power: Option<i64>,
    pub army_kill: Option<i64>,
    /// The member's `serverId`. Sent once for the whole alliance, as its warzone, rather than
    /// per member.
    pub server_id: Option<i64>,
}

impl Entry for Member {
    fn uid(&self) -> &str {
        &self.uid
    }
}

impl Member {
    /// `{"uid", "name", "rank", "power", "armyKill"}`
    pub fn to_json(&self) -> String {
        format!(
            "{{\"uid\":{},\"name\":{},\"rank\":{},\"power\":{},\"armyKill\":{}}}",
            escape(&self.uid),
            text(game_text(self.name.as_deref(), MAX_NAME).as_deref()),
            int(self.rank),
            int(count(self.power)),
            int(count(self.army_kill))
        )
    }
}

/// A row of the Desert Storm participants panel, `dragon.assign.player.info`. Only what the
/// member list doesn't already give: the panel's name and power are left out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Participant {
    pub uid: String,
    /// "Total Hero Power".
    pub hero_power: Option<i64>,
    /// Time slots picked, in the order clicked: 1 = 11:00, 2 = 20:00, 3 = 01:00 UTC.
    pub choose_time_list: Option<Vec<i64>>,
    /// Team assigned: 1 = Team A, 2 = Team B, 0 = none.
    pub group: Option<i64>,
    /// Role within the team: 1 = main (20 per team), 2 = sub (10 per team), 0 = not in a team.
    pub state: Option<i64>,
}

impl Entry for Participant {
    fn uid(&self) -> &str {
        &self.uid
    }
}

impl Participant {
    /// `{"uid", "heroPower", "chooseTimeList", "group", "state"}`. A `state` other than 0, 1
    /// or 2 would get the whole upload refused, so it is sent as unknown (`null`).
    pub fn to_json(&self) -> String {
        let slots = self
            .choose_time_list
            .as_ref()
            .map_or("null".into(), |list| {
                let items: Vec<String> = list.iter().map(i64::to_string).collect();
                format!("[{}]", items.join(","))
            });
        format!(
            "{{\"uid\":{},\"heroPower\":{},\"chooseTimeList\":{slots},\"group\":{},\"state\":{}}}",
            escape(&self.uid),
            int(count(self.hero_power)),
            int(self.group),
            int(self.state.filter(|s| (0..=2).contains(s)))
        )
    }
}

/// A row of one day's VS ranking, `al.battle.rank.info`. The ranking lists both alliances;
/// only our alliance's rows are sent, without the alliance, which the payload gives once.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VsScore {
    pub uid: String,
    pub score: Option<i64>,
    /// The player's alliance (`aid`).
    pub alliance_id: Option<String>,
}

impl Entry for VsScore {
    fn uid(&self) -> &str {
        &self.uid
    }
}

impl VsScore {
    /// `{"uid", "score"}`
    pub fn to_json(&self) -> String {
        format!(
            "{{\"uid\":{},\"score\":{}}}",
            escape(&self.uid),
            int(count(self.score))
        )
    }
}

fn text(value: Option<&str>) -> String {
    value.map_or_else(|| "null".into(), escape)
}

fn int(value: Option<i64>) -> String {
    value.map_or_else(|| "null".into(), |v| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_as_json() {
        let member = Member {
            uid: "9".into(),
            name: Some("a\"b\\c".into()),
            rank: Some(4),
            ..Member::default()
        };
        assert_eq!(
            member.to_json(),
            r#"{"uid":"9","name":"a\"b\\c","rank":4,"power":null,"armyKill":null}"#
        );
        let participant = Participant {
            uid: "9".into(),
            choose_time_list: Some(vec![2, 1, 3]),
            group: Some(0),
            state: Some(2),
            ..Participant::default()
        };
        assert_eq!(
            participant.to_json(),
            r#"{"uid":"9","heroPower":null,"chooseTimeList":[2,1,3],"group":0,"state":2}"#
        );
        for (state, sent) in [
            (None, "null"),
            (Some(0), "0"),
            (Some(3), "null"),
            (Some(-1), "null"),
        ] {
            let participant = Participant {
                uid: "9".into(),
                state,
                ..Participant::default()
            };
            assert!(
                participant
                    .to_json()
                    .ends_with(&format!(r#""state":{sent}}}"#))
            );
        }
        let empty = Participant {
            uid: "9".into(),
            choose_time_list: Some(vec![]),
            ..Participant::default()
        };
        assert!(empty.to_json().contains(r#""chooseTimeList":[],"#));
        let score = VsScore {
            uid: "9".into(),
            score: Some(42),
            alliance_id: Some("ours".into()),
        };
        assert_eq!(score.to_json(), r#"{"uid":"9","score":42}"#);
        let panel = Panel {
            time: Duration::from_secs(1_791_021_600),
            complete: false,
            entries: vec![member.clone()],
        };
        assert_eq!(
            panel.to_json(Member::to_json),
            format!(
                r#"{{"updated":"2026-10-03T10:00:00Z","complete":false,"players":[{}]}}"#,
                member.to_json()
            )
        );
    }

    #[test]
    fn lists_are_cut_to_the_players_the_api_takes() {
        let member = |uid: &str, rank| Member {
            uid: uid.into(),
            rank: Some(rank),
            ..Member::default()
        };
        let panel = |entries| Panel {
            time: Duration::from_secs(1_791_021_600),
            complete: true,
            entries,
        };
        let uids = |json: &str| {
            let list = crate::util::json::parse(json).unwrap();
            let players = list.get("players").and_then(|p| p.as_array()).unwrap();
            let uids: Vec<String> = players
                .iter()
                .map(|p| p.get("uid").and_then(|u| u.as_str()).unwrap().to_string())
                .collect();
            (uids, list.get("complete").cloned())
        };
        let complete = |b| Some(crate::util::json::Json::Bool(b));

        // A uid that isn't 1-20 digits is left out, and the list can't be complete.
        let json =
            panel(vec![member("1", 1), member("x1", 2), member("", 3)]).to_json(Member::to_json);
        assert_eq!(uids(&json), (vec!["1".to_string()], complete(false)));
        let longest = "9".repeat(20);
        let json =
            panel(vec![member(&longest, 1), member(&"9".repeat(21), 2)]).to_json(Member::to_json);
        assert_eq!(uids(&json), (vec![longest], complete(false)));

        // A player named twice keeps their first entry; nobody is missing.
        let json =
            panel(vec![member("1", 5), member("2", 1), member("1", 4)]).to_json(Member::to_json);
        assert_eq!(
            uids(&json),
            (vec!["1".to_string(), "2".to_string()], complete(true))
        );
        assert!(json.contains(r#""uid":"1","name":null,"rank":5"#));

        // At most 200 players.
        let many: Vec<Member> = (1..=201).map(|i| member(&i.to_string(), 1)).collect();
        let (kept, complete_flag) = uids(&panel(many).to_json(Member::to_json));
        assert_eq!((kept.len(), complete_flag), (MAX_PLAYERS, complete(false)));

        // Exactly 200 players and a repeat of one of them: nobody is left out, wherever the
        // repeat comes.
        let full: Vec<Member> = (1..=200).map(|i| member(&i.to_string(), 1)).collect();
        let mut repeat_last = full.clone();
        repeat_last.push(member("7", 2));
        let mut repeat_first = full;
        repeat_first.insert(0, member("7", 2));
        let last = uids(&panel(repeat_last).to_json(Member::to_json));
        let first = uids(&panel(repeat_first).to_json(Member::to_json));
        assert_eq!((last.0.len(), last.1), (MAX_PLAYERS, complete(true)));
        assert_eq!((first.0.len(), first.1), (MAX_PLAYERS, complete(true)));
    }

    #[test]
    fn values_the_api_would_refuse_are_sent_as_unknown() {
        assert_eq!(
            game_text(Some("Ex\u{1}am\nple"), MAX_NAME),
            Some("Example".into())
        );
        assert_eq!(game_text(Some("\u{7}"), MAX_NAME), None, "nothing left");
        assert_eq!(game_text(Some(""), MAX_NAME), None);
        assert_eq!(game_text(None, MAX_NAME), None);
        let long = "é".repeat(MAX_NAME);
        assert_eq!(
            game_text(Some(&long), MAX_NAME),
            Some(long.clone()),
            "64 characters"
        );
        assert_eq!(game_text(Some(&format!("{long}x")), MAX_NAME), None);
        assert_eq!(
            game_text(Some("ABCDEFGHIJKLMNOPQ"), MAX_TAG),
            None,
            "17 characters"
        );
        assert_eq!(count(Some(0)), Some(0));
        assert_eq!(count(Some(-1)), None);

        let member = Member {
            uid: "9".into(),
            name: Some("\u{1}".into()),
            power: Some(-5),
            army_kill: Some(7),
            ..Member::default()
        };
        assert_eq!(
            member.to_json(),
            r#"{"uid":"9","name":null,"rank":null,"power":null,"armyKill":7}"#
        );
    }
}
