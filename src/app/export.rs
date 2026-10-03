//! The data the tool hands on: today through Copy JSON, later through the upload API.

use std::time::Duration;

use crate::app::state::State;
use crate::game::week::{monday_days, vs_day};
use crate::util::json::escape;
use crate::util::time::utc_iso;

/// Version of the payload layout; raised whenever its fields change meaning or shape.
pub const SCHEMA_VERSION: u32 = 1;

/// The payload: the layout version, the VS week it covers, when it was made, the account,
/// when each part was last updated, and the players.
///
/// `{"schemaVersion": 1, "week": "YYYY-MM-DD", "generated", "account": {...} | null,
///   "updated": {"roster", "dsSignups", "dsResults", "vs"}, "players": [...]}`
///
/// The weekly reset runs first, so data from a week that has just ended is never labelled
/// with the new week. Desert Storm results count as loaded only once our alliance is known:
/// before that no battle can be attributed, and every player's `result` is `null`.
pub fn payload_json(state: &mut State, now: Duration) -> String {
    state.tick(now);
    let (week, _) = vs_day(now);
    let monday = Duration::from_secs(monday_days(week) * 86_400);
    let mail = &state.mail;
    let results_at = mail
        .loaded
        .filter(|_| mail.error.is_none() && state.view.alliance_id().is_some());
    format!(
        "{{\"schemaVersion\":{SCHEMA_VERSION},\"week\":{},\"generated\":{},\"account\":{},\"updated\":{{\"roster\":{},\"dsSignups\":{},\"dsResults\":{},\"vs\":{}}},\"players\":{}}}",
        escape(&utc_iso(monday)[..10]),
        time_json(Some(now)),
        account_json(state),
        time_json(state.view.roster_at),
        time_json(state.view.signups_at),
        time_json(results_at),
        time_json(state.view.vs_at),
        state.view.to_json()
    )
}

fn account_json(state: &State) -> String {
    let Some(a) = state.view.account() else {
        return "null".into();
    };
    let name = state.account_name();
    let opt = |v: Option<&str>| v.map_or_else(|| "null".into(), escape);
    format!(
        "{{\"uid\":{},\"name\":{},\"allianceId\":{},\"allianceName\":{},\"allianceAbbr\":{}}}",
        escape(&a.uid),
        opt(name.as_deref()),
        opt(state.view.alliance_id()),
        opt(a.alliance_name.as_deref()),
        opt(a.alliance_abbr.as_deref()),
    )
}

fn time_json(time: Option<Duration>) -> String {
    time.map_or_else(|| "null".into(), |t| escape(&utc_iso(t)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::message::Message;
    use crate::protocol::sfs::Value;
    use crate::util::json::{self, Json};

    /// Saturday 2026-10-03 10:00 UTC.
    const SATURDAY: u64 = 1_791_021_600;

    /// An `al.rank` member list of alliance "ours" with one member, uid 1.
    fn roster(secs: u64) -> Message {
        let object = |entries: Vec<(&str, Value)>| {
            Value::Object(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
        };
        let member = object(vec![("uid", Value::Str("1".into()))]);
        let data = object(vec![
            ("allianceId", Value::Str("ours".into())),
            ("list", Value::Array(vec![member])),
        ]);
        Message::command_for_test("al.rank", data, secs)
    }

    #[test]
    fn payload_has_week_and_update_times() {
        let mut state = State::default();
        state.mail.loaded = Some(Duration::from_secs(SATURDAY));
        let v = json::parse(&payload_json(&mut state, Duration::from_secs(SATURDAY))).unwrap();
        assert_eq!(v.get("schemaVersion"), Some(&Json::Number(1.0)));
        assert_eq!(v.get("week").and_then(Json::as_str), Some("2026-09-28"));
        assert_eq!(v.get("account"), Some(&Json::Null));
        let updated = v.get("updated").unwrap();
        assert_eq!(
            updated.get("dsResults"),
            Some(&Json::Null),
            "results can't be attributed before our alliance is known"
        );
        assert_eq!(updated.get("roster"), Some(&Json::Null));
        assert_eq!(v.get("players"), Some(&Json::Array(vec![])));

        state.record(&roster(SATURDAY));
        let v = json::parse(&payload_json(&mut state, Duration::from_secs(SATURDAY))).unwrap();
        let updated = v.get("updated").unwrap();
        assert_eq!(
            updated.get("dsResults").and_then(Json::as_str),
            Some("2026-10-03T10:00:00Z")
        );
        assert_eq!(
            updated.get("roster").and_then(Json::as_str),
            Some("2026-10-03T10:00:00Z")
        );
    }

    #[test]
    fn export_after_the_reset_carries_nothing_from_last_week() {
        let mut state = State::default();
        state.tick(Duration::from_secs(SATURDAY));
        state.mail.loaded = Some(Duration::from_secs(SATURDAY));
        // Copy JSON on Monday 03:00 UTC, before the window's next tick has run.
        let monday = Duration::from_secs(SATURDAY + 2 * 86_400 - 7 * 3600);
        let v = json::parse(&payload_json(&mut state, monday)).unwrap();
        assert_eq!(v.get("week").and_then(Json::as_str), Some("2026-10-05"));
        assert_eq!(
            v.get("updated").and_then(|u| u.get("dsResults")),
            Some(&Json::Null)
        );
    }
}
