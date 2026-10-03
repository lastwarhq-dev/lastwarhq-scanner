//! The data the tool syncs to LastWarHQ: each panel's newest list as the game sent it, keyed
//! by player uid. Joining panels together is left to the receiver.
//!
//! Times are on the game server's clock: they were moved onto it when recorded (see
//! [`State::game_clock`]), as LastWarHQ orders uploads from several PCs by them. Values the
//! API would refuse are sent as unknown (see [`game_text`] and [`count`]), so one odd name
//! can't get a whole upload refused.
//!
//! [`count`]: crate::game::panel::count

use std::time::Duration;

use crate::app::state::State;
use crate::game::panel::{MAX_NAME, MAX_TAG, Member, Panel, Participant, VsScore, game_text};
use crate::game::week::{monday_days, vs_day};
use crate::util::json::escape;
use crate::util::time::utc_iso;

/// Version of the payload layout; raised whenever its fields change meaning or shape.
/// 2: one section per panel instead of merged players; the alliance at the top, in place of
/// the account, which the roster already lists.
pub const SCHEMA_VERSION: u32 = 2;

/// The warzones the API takes.
const WARZONES: std::ops::RangeInclusive<i64> = 1..=99_999;

/// One of the alliance's name, tag and warzone as seen in the game: which, its value (as JSON)
/// and when it was seen.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub field: &'static str,
    pub value: String,
    pub time: Duration,
}

/// A built payload.
pub struct Payload {
    /// The payload as sent.
    pub json: String,
    /// Everything but its `generated` time: two payloads with the same content say the same.
    pub content: String,
    /// The alliance fields it carries, all seen at its `alliance.updated`.
    pub alliance: Vec<Seen>,
}

/// The payload:
///
/// `{"schemaVersion": 2, "week": "YYYY-MM-DD", "generated", "alliance": {...} | null,
///   "roster": panel | null, "dsSignups": panel | null, "dsResults": {...} | null,
///   "vs": [panel | null; 6]}`
///
/// where a panel is `{"updated", "complete", "players": [...]}`.
///
/// `now` is this PC's time. The weekly reset runs first, on the game server's clock, so data
/// from a week that has just ended is never labelled with the new week. Desert Storm results
/// count as loaded only once our alliance is known: before that no battle can be told to be
/// ours.
pub fn payload(state: &mut State, now: Duration) -> Payload {
    state.tick(now);
    let state = &*state;
    let now = state.game_clock(now);
    let (week, _) = vs_day(now);
    let monday = Duration::from_secs(monday_days(week) * 86_400);
    let view = &state.view;
    let vs: Vec<String> = view
        .vs_days
        .iter()
        .map(|day| panel_json(&ours(day, view.alliance_id()), VsScore::to_json))
        .collect();
    let (alliance_json, alliance) = alliance_json(state, now);
    let content = format!(
        "\"alliance\":{alliance_json},\"roster\":{},\"dsSignups\":{},\"dsResults\":{},\"vs\":[{}]",
        panel_json(&view.roster, Member::to_json),
        panel_json(&view.ds_signups, Participant::to_json),
        ds_results_json(state),
        vs.join(",")
    );
    let week = escape(&utc_iso(monday)[..10]);
    Payload {
        json: format!(
            "{{\"schemaVersion\":{SCHEMA_VERSION},\"week\":{week},\"generated\":{},{content}}}",
            escape(&utc_iso(now))
        ),
        content: format!("{week},{content}"),
        alliance,
    }
}

/// The payload as sent; see [`payload`].
pub fn payload_json(state: &mut State, now: Duration) -> String {
    payload(state, now).json
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

/// The alliance's name, tag and warzone as last seen in the game, each with its own time:
/// name and tag with the account's profile or the Desert Storm panel, the warzone (the
/// members' `serverId`, see [`View::warzone`]) with the member list.
///
/// [`View::warzone`]: crate::game::view::View::warzone
fn observations(state: &State) -> Vec<Seen> {
    let view = &state.view;
    let account = view.account();
    let name = account
        .and_then(|a| a.alliance_name.as_ref())
        .and_then(|(n, t)| Some((game_text(Some(n), MAX_NAME)?, *t)));
    let abbr = account
        .and_then(|a| a.alliance_abbr.as_ref())
        .and_then(|(a, t)| Some((game_text(Some(a), MAX_TAG)?, *t)));
    let warzone = view
        .warzone()
        .filter(|w| WARZONES.contains(w))
        .zip(view.roster.as_ref().map(|r| r.time));
    [
        name.map(|(n, t)| ("name", escape(&n), t)),
        abbr.map(|(a, t)| ("abbr", escape(&a), t)),
        warzone.map(|(w, t)| ("warzone", w.to_string(), t)),
    ]
    .into_iter()
    .flatten()
    .map(|(field, value, time)| Seen { field, value, time })
    .collect()
}

/// `{"id", "name", "abbr", "warzone", "updated"}`, or `null` until the alliance is known:
/// from the account's own messages, or else from the member list, which carries the id but
/// no name. Returns the fields it carries too.
///
/// LastWarHQ takes one `updated` for all three fields, and keeps each field's most recently
/// seen value by it. So an upload carries only the fields seen at one time, that time as
/// `updated`, and `null` (no observation) for the others. The oldest group LastWarHQ hasn't
/// accepted yet goes first; once all have been, the newest stays, so the payload stops
/// changing. With nothing seen, `updated` is the member list's time, or else `now`.
fn alliance_json(state: &State, now: Duration) -> (String, Vec<Seen>) {
    let Some(id) = state.view.alliance_id() else {
        return ("null".into(), Vec::new());
    };
    let seen = observations(state);
    let accepted = &state.sync.alliance_sent;
    let time = seen
        .iter()
        .filter(|s| !accepted.contains(s))
        .map(|s| s.time)
        .min()
        .or_else(|| seen.iter().map(|s| s.time).max());
    let group: Vec<Seen> = seen.into_iter().filter(|s| Some(s.time) == time).collect();
    let field = |name: &str| {
        group
            .iter()
            .find(|s| s.field == name)
            .map_or_else(|| "null".to_string(), |s| s.value.clone())
    };
    let updated = time
        .or_else(|| state.view.roster.as_ref().map(|r| r.time))
        .unwrap_or(now);
    let json = format!(
        "{{\"id\":{},\"name\":{},\"abbr\":{},\"warzone\":{},\"updated\":{}}}",
        escape(id),
        field("name"),
        field("abbr"),
        field("warzone"),
        escape(&utc_iso(updated))
    );
    (json, group)
}

/// `{"updated", "battles": [{"time", "won", "complete", "players": [{"uid", "score"}]}]}`:
/// each of this week's battles as its result mail gives it. `null` until the mail has been
/// read and our alliance is known. Battle times are the mails' own, already on the game
/// server's clock.
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

    /// Identifies account 7 at `secs`, with a profile naming alliance "ours" as `name` [`abbr`].
    fn profile(state: &mut State, secs: u64, name: &str, abbr: &str) {
        let text = |s: &str| Value::Str(s.into());
        let mail = object(vec![("toUser", text("7"))]);
        state.record(&Message::command_for_test("push.mail", mail, secs));
        let profile = object(vec![
            ("uid", text("7")),
            ("allianceId", text("ours")),
            ("allianceName", text(name)),
            ("abbr", text(abbr)),
        ]);
        state.record(&Message::command_for_test(
            "get.new.user.info",
            profile,
            secs,
        ));
    }

    #[test]
    fn alliance_updated_is_when_its_name_tag_and_warzone_were_seen() {
        let alliance = |state: &mut State| {
            payload(state, SATURDAY + 600)
                .get("alliance")
                .unwrap()
                .clone()
        };

        // Only the member list so far: its warzone, seen with it.
        let mut state = State::default();
        state.record(&roster(SATURDAY));
        let a = alliance(&mut state);
        assert_eq!(a.get("warzone"), Some(&Json::Number(901.0)));
        assert_eq!(
            a.get("updated").and_then(Json::as_str),
            Some("2026-10-03T10:00:00Z")
        );

        // The name and tag seen earlier than the warzone: the earlier time covers all three.
        let mut state = State::default();
        profile(&mut state, SATURDAY - 300, "Example Alliance", "EXA");
        state.record(&roster(SATURDAY));
        let a = alliance(&mut state);
        assert_eq!(
            a.get("name").and_then(Json::as_str),
            Some("Example Alliance")
        );
        assert_eq!(
            a.get("updated").and_then(Json::as_str),
            Some("2026-10-03T09:55:00Z")
        );
    }

    fn field(v: &Json, section: &str, key: &str) -> Option<String> {
        v.get(section)
            .and_then(|s| s.get(key))
            .and_then(Json::as_str)
            .map(str::to_string)
    }

    #[test]
    fn data_from_before_the_first_ping_is_corrected_once() {
        // This PC's clock is 10 minutes fast. The member list arrives before any ping reply,
        // at 10:10:00 on the PC, which is 10:00:00 on the server.
        let mut state = State::default();
        state.record(&roster(SATURDAY + 600));
        // The first ping reply: the server reads 10:00:05 when the PC reads 10:10:05.
        state.record(&Message::ping_for_test(
            (SATURDAY as i64 + 5) * 1000,
            SATURDAY + 605,
        ));
        let v = payload(&mut state, SATURDAY + 610);
        assert_eq!(
            field(&v, "roster", "updated").as_deref(),
            Some("2026-10-03T10:00:00Z"),
            "moved onto the server's clock, not left 10 minutes in its future"
        );
        assert_eq!(
            field(&v, "alliance", "updated").as_deref(),
            Some("2026-10-03T10:00:00Z")
        );
        assert_eq!(
            v.get("generated").and_then(Json::as_str),
            Some("2026-10-03T10:00:10Z")
        );

        // Only once: Windows then puts the PC's clock right, and the difference changes.
        state.record(&Message::ping_for_test(
            (SATURDAY as i64 + 20) * 1000,
            SATURDAY + 20,
        ));
        let v = payload(&mut state, SATURDAY + 20);
        assert_eq!(
            field(&v, "roster", "updated").as_deref(),
            Some("2026-10-03T10:00:00Z")
        );
    }

    /// One day's VS ranking (1 = Monday), one player of alliance "ours".
    fn ranking(day: i32, secs: u64) -> Message {
        let row = object(vec![
            ("uid", Value::Str("1".into())),
            ("score", Value::Int(5)),
            ("aid", Value::Str("ours".into())),
        ]);
        let data = object(vec![
            ("day", Value::Int(day)),
            ("rankInfo", Value::Array(vec![row])),
        ]);
        Message::command_for_test("al.battle.rank.info", data, secs)
    }

    #[test]
    fn vs_days_still_on_when_captured_before_the_first_ping_are_dropped() {
        // Wednesday 02:00 UTC ends Tuesday's VS day. This PC's clock is 10 minutes fast: at
        // 02:05 on the PC it is 01:55 on the server, and Tuesday's scores are still changing.
        let wednesday = SATURDAY - 3 * 86_400 - 8 * 3600;
        let mut state = State::default();
        state.record(&roster(wednesday + 290));
        // Before any ping reply, both look finished by the PC's clock, and are taken.
        state.record(&ranking(1, wednesday + 300));
        state.record(&ranking(2, wednesday + 300));
        assert!(state.view.vs_days[0].is_some() && state.view.vs_days[1].is_some());
        // The first ping reply shows the server 10 minutes behind.
        state.record(&Message::ping_for_test(
            (wednesday as i64 - 295) * 1000,
            wednesday + 305,
        ));
        let v = payload(&mut state, wednesday + 310);
        let vs = v.get("vs").and_then(Json::as_array).unwrap();
        assert_ne!(vs[0], Json::Null, "Monday was over: kept");
        assert_eq!(
            vs[0].get("updated").and_then(Json::as_str),
            Some("2026-09-30T01:55:00Z")
        );
        assert_eq!(vs[1], Json::Null, "Tuesday was still on: dropped");

        // After the reset, Tuesday's ranking is captured again, and kept.
        state.record(&ranking(2, wednesday + 700));
        let v = payload(&mut state, wednesday + 700);
        let vs = v.get("vs").and_then(Json::as_array).unwrap();
        assert_eq!(
            vs[1].get("updated").and_then(Json::as_str),
            Some("2026-09-30T02:01:40Z")
        );
    }

    #[test]
    fn clock_corrections_leave_captured_times_alone() {
        // The game server's clock is 90 s ahead of the PC's from the start.
        let mut state = State::default();
        state.record(&Message::ping_for_test(
            (SATURDAY as i64 + 90) * 1000,
            SATURDAY,
        ));
        state.record(&roster(SATURDAY + 600));
        let v = payload(&mut state, SATURDAY + 600);
        assert_eq!(
            field(&v, "roster", "updated").as_deref(),
            Some("2026-10-03T10:11:30Z")
        );
        // The PC's clock is then put right (Windows syncs it): what was captured stays put.
        state.record(&Message::ping_for_test(
            (SATURDAY as i64 + 700) * 1000,
            SATURDAY + 700,
        ));
        let v = payload(&mut state, SATURDAY + 700);
        assert_eq!(
            field(&v, "roster", "updated").as_deref(),
            Some("2026-10-03T10:11:30Z")
        );
        assert_eq!(
            field(&v, "alliance", "updated").as_deref(),
            Some("2026-10-03T10:11:30Z")
        );
    }

    #[test]
    fn a_week_rolled_by_a_fast_pc_clock_before_the_first_ping_is_put_back() {
        // Monday 02:00 UTC is the weekly reset. This PC's clock is 10 minutes fast, and the
        // window's clock runs (as it does every second) before any ping reply: at 02:05 on the
        // PC it rolls into the new week, though the server is at 01:55, still Sunday.
        let monday = SATURDAY + 2 * 86_400 - 8 * 3600;
        let mut state = State::default();
        state.tick(Duration::from_secs(monday + 300));
        assert_eq!(
            state.view.week(),
            Some(vs_day(Duration::from_secs(monday)).0)
        );
        // The first ping reply shows the server at 01:55.
        state.record(&Message::ping_for_test(
            (monday as i64 - 300) * 1000,
            monday + 300,
        ));
        let sunday = vs_day(Duration::from_secs(monday - 1)).0;
        assert_eq!(state.view.week(), Some(sunday), "back to the server's week");
        // So the game's messages count again, the member list included.
        state.record(&roster(monday + 310));
        let v = payload(&mut state, monday + 310);
        assert_eq!(v.get("week").and_then(Json::as_str), Some("2026-09-28"));
        assert_eq!(
            field(&v, "roster", "updated").as_deref(),
            Some("2026-10-05T01:55:10Z")
        );
    }

    #[test]
    fn the_week_follows_the_game_servers_clock() {
        // Monday 02:00 UTC is the weekly reset. The PC's clock reads 02:00:30, 60 s fast:
        // on the server it is still 01:59:30, Sunday, the week of 2026-09-28.
        let monday = SATURDAY + 2 * 86_400 - 8 * 3600;
        let mut state = State::default();
        state.record(&Message::ping_for_test(
            (monday as i64 - 30 - 3600) * 1000,
            monday + 30 - 3600,
        ));
        // Saturday's VS ranking, opened on Sunday.
        let saturday = object(vec![
            ("day", Value::Int(6)),
            (
                "rankInfo",
                Value::Array(vec![object(vec![
                    ("uid", Value::Str("1".into())),
                    ("score", Value::Int(5)),
                ])]),
            ),
        ]);
        state.record(&Message::command_for_test(
            "al.battle.rank.info",
            saturday,
            monday + 30 - 3600,
        ));
        let v = payload(&mut state, monday + 30);
        assert_eq!(v.get("week").and_then(Json::as_str), Some("2026-09-28"));
        assert_ne!(
            v.get("vs").and_then(Json::as_array).unwrap()[5],
            Json::Null,
            "the server hasn't reached the reset: the week's rankings stay"
        );
        assert_eq!(
            v.get("generated").and_then(Json::as_str),
            Some("2026-10-05T01:59:30Z")
        );
        // A PC clock 60 s slow, reading 01:59:30 when the server is past the reset.
        let mut state = State::default();
        state.record(&Message::ping_for_test(
            (monday as i64 + 30) * 1000,
            monday - 30,
        ));
        let v = payload(&mut state, monday - 30);
        assert_eq!(v.get("week").and_then(Json::as_str), Some("2026-10-05"));
        assert_eq!(
            v.get("generated").and_then(Json::as_str),
            Some("2026-10-05T02:00:30Z")
        );
    }

    #[test]
    fn alliance_values_the_api_would_refuse_are_unknown() {
        let mut state = State::default();
        profile(&mut state, SATURDAY, "\u{1}", "ABCDEFGHIJKLMNOPQ");
        let member = object(vec![
            ("uid", Value::Str("1".into())),
            ("serverId", Value::Int(0)),
        ]);
        let data = object(vec![
            ("allianceId", Value::Str("ours".into())),
            ("list", Value::Array(vec![member])),
        ]);
        state.record(&Message::command_for_test("al.rank", data, SATURDAY));
        let a = payload(&mut state, SATURDAY)
            .get("alliance")
            .unwrap()
            .clone();
        for key in ["name", "abbr", "warzone"] {
            assert_eq!(a.get(key), Some(&Json::Null), "{key}");
        }
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
