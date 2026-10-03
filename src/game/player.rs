//! One alliance member, and their JSON form.

use std::fmt::Write;

use crate::util::json::escape;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Player {
    pub uid: String,
    pub name: Option<String>,
    /// Alliance rank 1–5, R5 highest. From `al.rank`.
    pub rank: Option<i64>,
    pub power: Option<i64>,
    /// "Total Hero Power". From `dragon.assign.player.info`.
    pub hero_power: Option<i64>,
    /// From `al.rank`.
    pub army_kill: Option<i64>,
    /// Desert Storm time slots picked, in the order sent. From `dragon.assign.player.info`.
    pub choose_time_list: Option<Vec<i64>>,
    /// VS duel score per completed day, Monday–Saturday. From `al.battle.rank.info`.
    pub vs_scores: [Option<i64>; 6],
    /// Desert Storm team assigned in the participants panel: 1 or 2, 0 = none.
    /// From `dragon.assign.player.info`.
    pub ds_group: Option<i64>,
}

/// A player's part in this week's Desert Storm, from the result mails.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DsResult {
    /// The battle's team (panel `group`), when the participants panel has been seen.
    pub team: Option<i64>,
    /// Scored more than 0 in the battle.
    pub attended: bool,
    /// Personal score; `None` for assigned players missing from the battle's list.
    pub score: Option<i64>,
    pub won: bool,
}

impl Player {
    /// Takes every field `update` has; VS scores are merged separately.
    pub(crate) fn merge(&mut self, update: Player) {
        let Player {
            uid: _,
            name,
            rank,
            power,
            hero_power,
            army_kill,
            choose_time_list,
            vs_scores: _,
            ds_group,
        } = update;
        if choose_time_list.is_some() {
            self.choose_time_list = choose_time_list;
        }
        if ds_group.is_some() {
            self.ds_group = ds_group;
        }
        if name.is_some() {
            self.name = name;
        }
        if rank.is_some() {
            self.rank = rank;
        }
        if power.is_some() {
            self.power = power;
        }
        if hero_power.is_some() {
            self.hero_power = hero_power;
        }
        if army_kill.is_some() {
            self.army_kill = army_kill;
        }
    }

    /// One JSON object:
    /// `{uid, name, rank, stats: {power, heroPower, armyKill},
    ///   desertStorm: {chooseTimeList, result}, vsScores}`. Unknown values are `null`.
    pub fn to_json(&self, ds: Option<DsResult>) -> String {
        let opt = |v: Option<i64>| v.map_or("null".into(), |v| v.to_string());
        let name = self.name.as_deref().map_or_else(|| "null".into(), escape);
        let slots = self
            .choose_time_list
            .as_ref()
            .map_or("null".into(), |list| {
                let items: Vec<String> = list.iter().map(i64::to_string).collect();
                format!("[{}]", items.join(","))
            });
        let result = ds.map_or("null".into(), |d| {
            format!(
                "{{\"team\":{},\"attended\":{},\"score\":{},\"won\":{}}}",
                opt(d.team),
                d.attended,
                opt(d.score),
                d.won
            )
        });
        let vs: Vec<String> = self.vs_scores.iter().map(|&s| opt(s)).collect();

        let mut out = String::from("{");
        let _ = write!(
            out,
            "\"uid\":{},\"name\":{name},\"rank\":{}",
            escape(&self.uid),
            opt(self.rank)
        );
        let _ = write!(
            out,
            ",\"stats\":{{\"power\":{},\"heroPower\":{},\"armyKill\":{}}}",
            opt(self.power),
            opt(self.hero_power),
            opt(self.army_kill)
        );
        let _ = write!(
            out,
            ",\"desertStorm\":{{\"chooseTimeList\":{slots},\"result\":{result}}}"
        );
        let _ = write!(out, ",\"vsScores\":[{}]", vs.join(","));
        out.push('}');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_escapes_and_nulls() {
        let p = Player {
            uid: "9".into(),
            name: Some("a\"b\\c\u{1}".into()),
            ..Player::default()
        };
        assert_eq!(
            p.to_json(None),
            r#"{"uid":"9","name":"a\"b\\c\u0001","rank":null,"stats":{"power":null,"heroPower":null,"armyKill":null},"desertStorm":{"chooseTimeList":null,"result":null},"vsScores":[null,null,null,null,null,null]}"#
        );
        let p = Player {
            uid: "9".into(),
            choose_time_list: Some(vec![2, 1, 3]),
            ..Player::default()
        };
        assert!(
            p.to_json(None)
                .contains(r#""desertStorm":{"chooseTimeList":[2,1,3],"#)
        );
        let p = Player {
            uid: "9".into(),
            choose_time_list: Some(vec![]),
            ..Player::default()
        };
        assert!(p.to_json(None).contains(r#""chooseTimeList":[],"#));
        let mut p = Player {
            uid: "9".into(),
            power: Some(5),
            hero_power: Some(3),
            army_kill: Some(1),
            ..Player::default()
        };
        p.vs_scores[1] = Some(42);
        assert!(
            p.to_json(None)
                .contains(r#""stats":{"power":5,"heroPower":3,"armyKill":1}"#)
        );
        assert!(
            p.to_json(None)
                .ends_with(r#""vsScores":[null,42,null,null,null,null]}"#)
        );
        let ds = DsResult {
            team: Some(2),
            attended: true,
            score: Some(7),
            won: false,
        };
        assert!(
            p.to_json(Some(ds))
                .contains(r#""result":{"team":2,"attended":true,"score":7,"won":false}}"#)
        );
    }
}
