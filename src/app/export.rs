//! The data the tool hands on, through the sync to come: each panel's newest list as the game
//! sent it, keyed by player uid. Joining panels together is left to the receiver.

use std::time::Duration;

use crate::app::state::State;
use crate::game::panel::{Member, Panel, Participant, VsScore};
use crate::game::week::{monday_days, vs_day};
use crate::util::json::escape;
use crate::util::time::utc_iso;

/// Version of the payload layout; raised whenever its fields change meaning or shape.
/// 2: one section per panel instead of merged players; the alliance at the top, in place of
/// the account, which the roster already lists.
pub const SCHEMA_VERSION: u32 = 2;

/// The payload:
///
/// `{"schemaVersion": 2, "week": "YYYY-MM-DD", "generated", "alliance": {...} | null,
///   "roster": panel | null, "dsSignups": panel | null, "dsResults": {...} | null,
///   "vs": [panel | null; 6]}`
///
/// where a panel is `{"updated", "complete", "players": [...]}`.
///
/// The weekly reset runs first, so data from a week that has just ended is never labelled
/// with the new week. Desert Storm results count as loaded only once our alliance is known:
/// before that no battle can be told to be ours.
pub fn payload_json(state: &mut State, now: Duration) -> String {
    state.tick(now);
    let (week, _) = vs_day(now);
    let monday = Duration::from_secs(monday_days(week) * 86_400);
    let view = &state.view;
    let vs: Vec<String> = view
        .vs_days
        .iter()
        .map(|day| panel_json(&ours(day, view.alliance_id()), VsScore::to_json))
        .collect();
    format!(
        "{{\"schemaVersion\":{SCHEMA_VERSION},\"week\":{},\"generated\":{},\"alliance\":{},\"roster\":{},\"dsSignups\":{},\"dsResults\":{},\"vs\":[{}]}}",
        escape(&utc_iso(monday)[..10]),
        escape(&utc_iso(now)),
        alliance_json(state),
        panel_json(&view.roster, Member::to_json),
        panel_json(&view.ds_signups, Participant::to_json),
        ds_results_json(state),
        vs.join(",")
    )
}

fn opt(value: Option<&str>) -> String {
    value.map_or_else(|| "null".into(), escape)
}

fn panel_json<T>(panel: &Option<Panel<T>>, entry: impl Fn(&T) -> String) -> String {
    panel
        .as_ref()
        .map_or_else(|| "null".into(), |p| p.to_json(entry))
}

/// A VS ranking cut down to our alliance's players. The ranking lists both alliances; until
/// ours is known, no row can be told to be ours. A row without a readable alliance can't be
/// told either way, so it makes the day incomplete: it may be one of ours.
fn ours(day: &Option<Panel<VsScore>>, alliance: Option<&str>) -> Option<Panel<VsScore>> {
    let day = day.as_ref()?;
    Some(Panel {
        time: day.time,
        complete: day.complete && day.entries.iter().all(|e| e.alliance_id.is_some()),
        entries: day
            .entries
            .iter()
            .filter(|e| alliance.is_some() && e.alliance_id.as_deref() == alliance)
            .cloned()
            .collect(),
    })
}

/// `{"id", "name", "abbr", "warzone"}`, or `null` until the alliance is known: from the
/// account's own messages, or else from the member list, which carries the id but no name.
/// `warzone` is the members' `serverId` (see [`View::warzone`]).
///
/// [`View::warzone`]: crate::game::view::View::warzone
fn alliance_json(state: &State) -> String {
    let Some(id) = state.view.alliance_id() else {
        return "null".into();
    };
    let account = state.view.account();
    format!(
        "{{\"id\":{},\"name\":{},\"abbr\":{},\"warzone\":{}}}",
        escape(id),
        opt(account.and_then(|a| a.alliance_name.as_deref())),
        opt(account.and_then(|a| a.alliance_abbr.as_deref())),
        state
            .view
            .warzone()
            .map_or_else(|| "null".into(), |w| w.to_string())
    )
}

/// `{"updated", "battles": [{"time", "won", "complete", "players": [{"uid", "score"}]}]}`:
/// each of this week's battles as its result mail gives it. `null` until the mail has been
/// read and our alliance is known.
fn ds_results_json(state: &State) -> String {
    let Some(loaded) = state
        .mail
        .loaded
        .filter(|_| state.view.alliance_id().is_some())
    else {
        return "null".into();
    };
    let battles: Vec<String> = state
        .view
        .ds_battles()
        .map(|b| {
            let players: Vec<String> = b
                .players
                .iter()
                .map(|(uid, score)| format!("{{\"uid\":{},\"score\":{score}}}", escape(uid)))
                .collect();
            format!(
                "{{\"time\":{},\"won\":{},\"complete\":{},\"players\":[{}]}}",
                escape(&utc_iso(b.time)),
                b.won,
                b.players_complete,
                players.join(",")
            )
        })
        .collect();
    format!(
        "{{\"updated\":{},\"battles\":[{}]}}",
        escape(&utc_iso(loaded)),
        battles.join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::battle::DsBattle;
    use crate::protocol::message::Message;
    use crate::protocol::sfs::Value;
    use crate::util::json::{self, Json};

    /// Saturday 2026-10-03 10:00 UTC.
    const SATURDAY: u64 = 1_791_021_600;

    fn object(entries: Vec<(&str, Value)>) -> Value {
        Value::Object(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// An `al.rank` member list of alliance "ours" with one member, uid 1.
    fn roster(secs: u64) -> Message {
        let member = object(vec![
            ("uid", Value::Str("1".into())),
            ("serverId", Value::Int(901)),
        ]);
        let data = object(vec![
            ("allianceId", Value::Str("ours".into())),
            ("list", Value::Array(vec![member])),
        ]);
        Message::command_for_test("al.rank", data, secs)
    }

    fn payload(state: &mut State, secs: u64) -> Json {
        json::parse(&payload_json(state, Duration::from_secs(secs))).unwrap()
    }

    #[test]
    fn payload_sections_start_empty() {
        let mut state = State::default();
        state.mail.loaded = Some(Duration::from_secs(SATURDAY));
        let v = payload(&mut state, SATURDAY);
        assert_eq!(v.get("schemaVersion"), Some(&Json::Number(2.0)));
        assert_eq!(v.get("week").and_then(Json::as_str), Some("2026-09-28"));
        assert_eq!(
            v.get("generated").and_then(Json::as_str),
            Some("2026-10-03T10:00:00Z")
        );
        for key in ["alliance", "roster", "dsSignups"] {
            assert_eq!(v.get(key), Some(&Json::Null), "{key}");
        }
        assert_eq!(
            v.get("dsResults"),
            Some(&Json::Null),
            "results can't be told to be ours before our alliance is known"
        );
        assert_eq!(v.get("vs"), Some(&Json::Array(vec![Json::Null; 6])));
    }

    #[test]
    fn payload_carries_each_panel_as_sent() {
        let mut state = State::default();
        state.record(&roster(SATURDAY));
        let ranking = object(vec![
            ("day", Value::Int(2)),
            (
                "rankInfo",
                Value::Array(vec![
                    object(vec![
                        ("uid", Value::Str("9".into())),
                        ("score", Value::Int(999)),
                        ("aid", Value::Str("theirs".into())),
                    ]),
                    object(vec![
                        ("uid", Value::Str("1".into())),
                        ("score", Value::Int(300)),
                        ("aid", Value::Str("ours".into())),
                    ]),
                ]),
            ),
        ]);
        state.record(&Message::command_for_test(
            "al.battle.rank.info",
            ranking,
            SATURDAY,
        ));
        let v = payload(&mut state, SATURDAY);
        // The member list names the alliance before the account is identified.
        let alliance = v.get("alliance").unwrap();
        assert_eq!(alliance.get("id").and_then(Json::as_str), Some("ours"));
        assert_eq!(alliance.get("name"), Some(&Json::Null));
        assert_eq!(alliance.get("warzone"), Some(&Json::Number(901.0)));

        let roster = v.get("roster").unwrap();
        assert_eq!(
            roster.get("updated").and_then(Json::as_str),
            Some("2026-10-03T10:00:00Z")
        );
        assert_eq!(roster.get("complete"), Some(&Json::Bool(true)));
        let players = roster.get("players").and_then(Json::as_array).unwrap();
        assert_eq!(players[0].get("uid").and_then(Json::as_str), Some("1"));

        let vs = v.get("vs").and_then(Json::as_array).unwrap();
        assert_eq!(vs[0], Json::Null);
        // Only our alliance's row, without the alliance.
        let tuesday = vs[1].get("players").unwrap();
        assert_eq!(
            *tuesday,
            json::parse(r#"[{"uid":"1","score":300}]"#).unwrap()
        );
    }

    #[test]
    fn a_vs_row_without_an_alliance_makes_the_day_incomplete() {
        let mut state = State::default();
        // Roster member 1's score row arrives without its `aid`.
        state.record(&roster(SATURDAY));
        let ranking = object(vec![
            ("day", Value::Int(2)),
            (
                "rankInfo",
                Value::Array(vec![object(vec![
                    ("uid", Value::Str("1".into())),
                    ("score", Value::Int(300)),
                ])]),
            ),
        ]);
        state.record(&Message::command_for_test(
            "al.battle.rank.info",
            ranking,
            SATURDAY,
        ));
        let v = payload(&mut state, SATURDAY);
        let tuesday = &v.get("vs").and_then(Json::as_array).unwrap()[1];
        assert_eq!(tuesday.get("complete"), Some(&Json::Bool(false)));
        assert_eq!(tuesday.get("players"), Some(&Json::Array(vec![])));
    }

    #[test]
    fn vs_rows_wait_for_our_alliance() {
        let mut state = State::default();
        let ranking = object(vec![
            ("day", Value::Int(2)),
            (
                "rankInfo",
                Value::Array(vec![object(vec![
                    ("uid", Value::Str("1".into())),
                    ("score", Value::Int(300)),
                    ("aid", Value::Str("ours".into())),
                ])]),
            ),
        ]);
        state.record(&Message::command_for_test(
            "al.battle.rank.info",
            ranking,
            SATURDAY,
        ));
        let v = payload(&mut state, SATURDAY);
        let tuesday = &v.get("vs").and_then(Json::as_array).unwrap()[1];
        assert_eq!(tuesday.get("players"), Some(&Json::Array(vec![])));
        // The member list names the alliance: the ranking already held is now ours.
        state.record(&roster(SATURDAY + 1));
        let v = payload(&mut state, SATURDAY + 1);
        let tuesday = &v.get("vs").and_then(Json::as_array).unwrap()[1];
        let players = tuesday.get("players").and_then(Json::as_array).unwrap();
        assert_eq!(players.len(), 1);
    }

    #[test]
    fn ds_results_carry_each_battles_result_and_scores_without_teams() {
        let mut state = State::default();
        state.record(&roster(SATURDAY));
        let started = state.start_mail_load(Duration::from_secs(SATURDAY));
        let battle = DsBattle {
            time: Duration::from_secs(SATURDAY - 9 * 3600),
            won: true,
            alliance_id: "ours".into(),
            score: 1,
            enemy_abbr: "X".into(),
            enemy_name: "X".into(),
            enemy_score: 0,
            players: vec![("1".into(), 50), ("2".into(), 0)],
            players_complete: false,
        };
        state.finish_mail_load(&started, Ok(vec![battle]), Duration::from_secs(SATURDAY));
        let v = payload(&mut state, SATURDAY);
        let results = v.get("dsResults").unwrap();
        assert_eq!(
            results.get("updated").and_then(Json::as_str),
            Some("2026-10-03T10:00:00Z")
        );
        let battles = results.get("battles").and_then(Json::as_array).unwrap();
        assert_eq!(
            battles[0],
            json::parse(
                r#"{"time":"2026-10-03T01:00:00Z","won":true,"complete":false,"players":[{"uid":"1","score":50},{"uid":"2","score":0}]}"#
            )
            .unwrap()
        );
    }

    #[test]
    fn payload_names_the_alliance_but_not_the_account() {
        let text = |s: &str| Value::Str(s.into());
        let mut state = State::default();
        let mail = object(vec![("toUser", text("7"))]);
        state.record(&Message::command_for_test("push.mail", mail, SATURDAY));
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
            SATURDAY + 1,
        ));
        let v = payload(&mut state, SATURDAY + 2);
        assert_eq!(
            v.get("account"),
            None,
            "the roster lists the account's player"
        );
        let alliance = v.get("alliance").unwrap();
        assert_eq!(alliance.get("id").and_then(Json::as_str), Some("ours"));
        assert_eq!(
            alliance.get("name").and_then(Json::as_str),
            Some("Example Alliance")
        );
        assert_eq!(alliance.get("abbr").and_then(Json::as_str), Some("EXA"));
    }

    #[test]
    fn export_after_the_reset_carries_nothing_from_last_week() {
        let mut state = State::default();
        state.tick(Duration::from_secs(SATURDAY));
        state.mail.loaded = Some(Duration::from_secs(SATURDAY));
        // Exported on Monday 03:00 UTC, before the window's next tick has run.
        let v = payload(&mut state, SATURDAY + 2 * 86_400 - 7 * 3600);
        assert_eq!(v.get("week").and_then(Json::as_str), Some("2026-10-05"));
        assert_eq!(v.get("dsResults"), Some(&Json::Null));
    }
}
