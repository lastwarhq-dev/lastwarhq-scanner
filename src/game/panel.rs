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

impl<T> Panel<T> {
    /// `{"updated", "complete", "players": [...]}`, each entry through `entry`.
    pub fn to_json(&self, entry: impl Fn(&T) -> String) -> String {
        let players: Vec<String> = self.entries.iter().map(entry).collect();
        format!(
            "{{\"updated\":{},\"complete\":{},\"players\":[{}]}}",
            escape(&utc_iso(self.time)),
            self.complete,
            players.join(",")
        )
    }
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
}

impl Participant {
    /// `{"uid", "heroPower", "chooseTimeList", "group"}`
    pub fn to_json(&self) -> String {
        let slots = self
            .choose_time_list
            .as_ref()
            .map_or("null".into(), |list| {
                let items: Vec<String> = list.iter().map(i64::to_string).collect();
                format!("[{}]", items.join(","))
            });
        format!(
            "{{\"uid\":{},\"heroPower\":{},\"chooseTimeList\":{slots},\"group\":{}}}",
            escape(&self.uid),
            int(count(self.hero_power)),
            int(self.group)
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
            ..Participant::default()
        };
        assert_eq!(
            participant.to_json(),
            r#"{"uid":"9","heroPower":null,"chooseTimeList":[2,1,3],"group":0}"#
        );
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
