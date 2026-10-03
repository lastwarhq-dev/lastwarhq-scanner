//! Syncing to LastWarHQ: uploads the payload with `POST /v1/sync` whenever its content has
//! changed, once an upload can be accepted, and no sooner than LastWarHQ allows (one a
//! minute, or what `nextSyncAfter` and `Retry-After` say).
//!
//! An upload needs a sign-in, a known alliance and the alliance's full member list. Whether
//! the user manages the alliance is LastWarHQ's to say (`404 unknown_alliance`).
//!
//! Waits are counted on a monotonic clock from when the answer arrived, so neither a slow
//! answer nor the PC's clock changing can shorten them.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::app::export::{Seen, payload};
use crate::app::lock;
use crate::app::state::{State, SyncStatus};
use crate::auth::{self, api, api::ApiError, store};
use crate::util::time::now;

/// How often to look for something to upload.
const CHECK_EVERY: Duration = Duration::from_secs(5);
/// The least time between uploads, unless LastWarHQ says otherwise.
const MIN_GAP: Duration = Duration::from_secs(60);
/// After `404 unknown_alliance`: the alliance may be added on the site meanwhile.
const UNKNOWN_ALLIANCE_WAIT: Duration = Duration::from_secs(5 * 60);

/// Why nothing can be uploaded yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    SignedOut,
    NoAlliance,
    /// No member list yet, or one with an unreadable entry.
    NoFullRoster,
    /// No ping reply yet: times are still on this PC's clock, which LastWarHQ might find in
    /// the future.
    NoGameClock,
}

/// Whether an upload could be accepted now; see [`Hold`].
pub fn ready(state: &State) -> Result<(), Hold> {
    if state.auth.user.is_none() || auth::busy(&state.auth) {
        return Err(Hold::SignedOut);
    }
    if state.view.alliance_id().is_none() {
        return Err(Hold::NoAlliance);
    }
    if !state.view.roster.as_ref().is_some_and(|r| r.complete) {
        return Err(Hold::NoFullRoster);
    }
    if state.capture.clock_offset_ms.is_none() {
        return Err(Hold::NoGameClock);
    }
    Ok(())
}

/// Uploads on a background thread for the life of the process.
pub fn watch(state: Arc<Mutex<State>>) {
    thread::spawn(move || {
        loop {
            thread::sleep(CHECK_EVERY);
            upload_if_due(&state);
        }
    });
}

/// What an upload carried, to record once LastWarHQ has answered.
struct Sent {
    content: String,
    alliance: Vec<Seen>,
}

/// One upload, if one is due: built under the lock, sent without it, and applied only if the
/// sign-in it was made for is still the current one.
fn upload_if_due(state: &Mutex<State>) {
    let (json, sent, generation) = {
        let mut s = lock(state);
        let built = payload(&mut s, now());
        if !due(&s, &built.content, Instant::now()) {
            return;
        }
        s.sync.uploading = true;
        let sent = Sent {
            content: built.content,
            alliance: built.alliance,
        };
        (built.json, sent, s.auth.generation)
    };
    let result = match store::load() {
        Ok(Some(saved)) => api::sync(&saved.token, &json),
        // Signed in, but the token is gone (removed in Credential Manager): sign in again.
        Ok(None) => Err(ApiError::Unauthorized),
        Err(why) => Err(ApiError::Failed(format!(
            "the saved sign-in can't be read: {why}"
        ))),
    };
    let (answered, answered_at) = (Instant::now(), now());
    let mut s = lock(state);
    s.sync.uploading = false;
    if s.auth.generation != generation {
        return;
    }
    if apply(&mut s.sync, result, sent, answered, answered_at) {
        auth::token_refused(&mut s.auth);
        s.sync = SyncStatus::default();
    }
}

/// Whether to upload `content` at `now`: it can be accepted, it hasn't been sent (or refused)
/// already, and the wait LastWarHQ asked for is over.
fn due(state: &State, content: &str, now: Instant) -> bool {
    let sync = &state.sync;
    ready(state).is_ok()
        && !sync.uploading
        && sync.next_at.is_none_or(|t| now >= t)
        && sync.sent.as_deref() != Some(content)
        && sync.refused.as_deref() != Some(content)
}

/// Records how an upload went, answered at `answered` (`answered_at` on this PC's clock, for
/// showing), and when the next may be. Returns whether LastWarHQ refused the token (`401`), so
/// the user must sign in again.
fn apply(
    sync: &mut SyncStatus,
    result: Result<api::SyncReply, ApiError>,
    sent: Sent,
    answered: Instant,
    answered_at: Duration,
) -> bool {
    let wait =
        |secs: Option<u64>| answered + secs.map_or(MIN_GAP, Duration::from_secs).max(MIN_GAP);
    match result {
        Ok(reply) => {
            sync.next_at = Some(wait(reply.next_sync_after));
            sync.synced = Some(answered_at);
            sync.reply = Some(reply);
            sync.sent = Some(sent.content);
            sync.refused = None;
            sync.error = None;
            // Accepted (applied, unchanged or stale): those fields needn't be sent again.
            for field in sent.alliance {
                sync.alliance_sent.retain(|s| s.field != field.field);
                sync.alliance_sent.push(field);
            }
        }
        Err(ApiError::Unauthorized) => return true,
        Err(ApiError::TooManyRequests(secs)) => sync.next_at = Some(wait(secs)),
        Err(ApiError::UnknownAlliance) => {
            sync.next_at = Some(answered + UNKNOWN_ALLIANCE_WAIT);
            sync.error = Some(ApiError::UnknownAlliance.to_string());
        }
        Err(refused @ ApiError::BadRequest(_)) => {
            sync.next_at = Some(wait(None));
            sync.refused = Some(sent.content);
            sync.error = Some(refused.to_string());
        }
        Err(other) => {
            sync.next_at = Some(wait(None));
            sync.error = Some(other.to_string());
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::AuthStep;
    use crate::protocol::message::Message;
    use crate::protocol::sfs::Value;

    /// Saturday 2026-10-03 10:00 UTC.
    const T: Duration = Duration::from_secs(1_791_021_600);

    fn object(entries: Vec<(&str, Value)>) -> Value {
        Value::Object(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// The member list of alliance "ours", one member on server 901, captured at `secs`.
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

    /// Account 7's profile at `secs`, naming alliance "ours" as `name` and, if given, `abbr`.
    fn profile(state: &mut State, secs: u64, name: &str, abbr: Option<&str>) {
        let text = |s: &str| Value::Str(s.into());
        let mail = object(vec![("toUser", text("7"))]);
        state.record(&Message::command_for_test("push.mail", mail, secs));
        let mut fields = vec![
            ("uid", text("7")),
            ("allianceId", text("ours")),
            ("allianceName", text(name)),
        ];
        if let Some(abbr) = abbr {
            fields.push(("abbr", text(abbr)));
        }
        state.record(&Message::command_for_test(
            "get.new.user.info",
            object(fields),
            secs,
        ));
    }

    /// A state whose clock a ping reply has set (to this PC's: no difference).
    fn calibrated() -> State {
        let mut state = State::default();
        state.record(&Message::ping_for_test(T.as_millis() as i64, T.as_secs()));
        state
    }

    /// Signed in, with alliance "ours" and its full member list.
    fn ready_state() -> State {
        let mut state = calibrated();
        state.auth.user = Some("example".into());
        state.record(&roster(T.as_secs()));
        state
    }

    fn sent(content: &str) -> Sent {
        Sent {
            content: content.into(),
            alliance: Vec::new(),
        }
    }

    /// Uploads what is due at `at` and LastWarHQ accepts it; returns the alliance sent.
    fn accept(state: &mut State, at: Instant) -> String {
        let built = payload(state, T);
        let json = crate::util::json::parse(&built.json).unwrap();
        let alliance = json.get("alliance").unwrap().clone();
        let sent = Sent {
            content: built.content,
            alliance: built.alliance,
        };
        apply(&mut state.sync, Ok(api::SyncReply::default()), sent, at, T);
        format!(
            "{} {} {} {}",
            text(&alliance, "name"),
            text(&alliance, "abbr"),
            text(&alliance, "warzone"),
            text(&alliance, "updated"),
        )
    }

    fn text(json: &crate::util::json::Json, key: &str) -> String {
        match json.get(key) {
            Some(crate::util::json::Json::Str(s)) => s.clone(),
            Some(crate::util::json::Json::Number(n)) => n.to_string(),
            _ => "null".into(),
        }
    }

    #[test]
    fn uploads_wait_for_a_sign_in_an_alliance_and_the_full_roster() {
        let mut state = State::default();
        assert_eq!(ready(&state), Err(Hold::SignedOut));
        state.auth.user = Some("example".into());
        assert_eq!(ready(&state), Err(Hold::NoAlliance));

        let mut state = ready_state();
        assert_eq!(ready(&state), Ok(()));
        state.view.roster.as_mut().unwrap().complete = false;
        assert_eq!(ready(&state), Err(Hold::NoFullRoster));
        state.view.roster = None;
        assert_eq!(ready(&state), Err(Hold::NoFullRoster));

        let mut state = ready_state();
        state.auth.step = AuthStep::SigningOut;
        assert_eq!(ready(&state), Err(Hold::SignedOut));

        // Everything in place but the game server's clock: its times could be in the future.
        let mut state = State::default();
        state.auth.user = Some("example".into());
        state.record(&roster(T.as_secs()));
        assert_eq!(ready(&state), Err(Hold::NoGameClock));
    }

    #[test]
    fn the_same_content_is_not_uploaded_twice() {
        let mut state = ready_state();
        let start = Instant::now();
        let content = payload(&mut state, T).content;
        assert!(due(&state, &content, start));
        accept(&mut state, start);
        // Later, with nothing new: the same content, so nothing to send.
        let later = start + Duration::from_secs(600);
        let again = payload(&mut state, T + Duration::from_secs(600)).content;
        assert_eq!(again, content);
        assert!(!due(&state, &again, later));
        assert!(due(&state, "something new", later));
    }

    #[test]
    fn waits_start_when_the_answer_arrives() {
        let mut state = ready_state();
        let sent_at = Instant::now();
        // The upload takes 30 s to be answered, and LastWarHQ asks for 60 s more.
        let answered = sent_at + Duration::from_secs(30);
        let reply = api::SyncReply {
            next_sync_after: Some(60),
            ..api::SyncReply::default()
        };
        apply(&mut state.sync, Ok(reply), sent("a"), answered, T);
        assert!(!due(&state, "b", sent_at + Duration::from_secs(60)));
        assert!(!due(&state, "b", answered + Duration::from_secs(59)));
        assert!(due(&state, "b", answered + Duration::from_secs(60)));

        // Retry-After also counts from the answer, and a shorter wait than a minute is a minute.
        apply(
            &mut state.sync,
            Err(ApiError::TooManyRequests(Some(5))),
            sent("b"),
            answered,
            T,
        );
        assert!(!due(&state, "b", answered + Duration::from_secs(59)));
        apply(
            &mut state.sync,
            Err(ApiError::TooManyRequests(Some(120))),
            sent("b"),
            answered,
            T,
        );
        assert!(!due(&state, "b", answered + Duration::from_secs(119)));
        assert!(
            due(&state, "b", answered + Duration::from_secs(120)),
            "a 429 is not a refusal of the content"
        );
    }

    #[test]
    fn refused_content_is_not_sent_again_until_it_changes() {
        let mut state = ready_state();
        let now = Instant::now();
        let token_refused = apply(
            &mut state.sync,
            Err(ApiError::BadRequest(
                "roster.players[0].name is too long".into(),
            )),
            sent("a"),
            now,
            T,
        );
        assert!(!token_refused);
        assert_eq!(
            state.sync.error.as_deref(),
            Some("LastWarHQ refused the data: roster.players[0].name is too long")
        );
        let later = now + Duration::from_secs(120);
        assert!(!due(&state, "a", later));
        assert!(due(&state, "b", later));
    }

    #[test]
    fn failures_are_shown_and_retried() {
        let now = Instant::now();
        let mut state = ready_state();
        assert!(apply(
            &mut state.sync,
            Err(ApiError::Unauthorized),
            sent("a"),
            now,
            T
        ));

        let mut state = ready_state();
        assert!(!apply(
            &mut state.sync,
            Err(ApiError::Failed("lastwarhq.dev: no connection".into())),
            sent("a"),
            now,
            T
        ));
        assert_eq!(
            state.sync.error.as_deref(),
            Some("lastwarhq.dev: no connection")
        );
        assert!(due(&state, "a", now + MIN_GAP), "retried after a minute");

        let mut state = ready_state();
        apply(
            &mut state.sync,
            Err(ApiError::UnknownAlliance),
            sent("a"),
            now,
            T,
        );
        assert!(!due(&state, "a", now + MIN_GAP));
        assert!(due(&state, "a", now + UNKNOWN_ALLIANCE_WAIT));
    }

    #[test]
    fn alliance_fields_are_sent_with_the_time_each_was_seen() {
        // Name and tag seen at 09:55, the member list (and so the warzone) at 10:00.
        let mut state = calibrated();
        state.auth.user = Some("example".into());
        profile(
            &mut state,
            T.as_secs() - 300,
            "Example Alliance",
            Some("EXA"),
        );
        state.record(&roster(T.as_secs()));
        let mut at = Instant::now();
        // The oldest observation goes first, alone with its own time; the rest are null.
        assert_eq!(
            accept(&mut state, at),
            "Example Alliance EXA null 2026-10-03T09:55:00Z"
        );
        // Accepted: next the warzone, with the member list's time.
        at += MIN_GAP;
        assert_eq!(accept(&mut state, at), "null null 901 2026-10-03T10:00:00Z");
        // Everything accepted: the payload stays put, so nothing more is uploaded.
        at += MIN_GAP;
        let content = payload(&mut state, T).content;
        assert!(!due(&state, &content, at));
    }

    #[test]
    fn a_name_seen_alone_does_not_make_an_old_tag_look_new() {
        let mut state = calibrated();
        state.auth.user = Some("example".into());
        state.record(&roster(T.as_secs() - 600));
        profile(&mut state, T.as_secs() - 300, "Old Name", Some("EXA"));
        let mut at = Instant::now();
        accept(&mut state, at);
        at += MIN_GAP;
        accept(&mut state, at);
        // The alliance is renamed; this profile carries the name but no tag.
        profile(&mut state, T.as_secs(), "New Name", None);
        at += MIN_GAP;
        // Only the name goes, with its time; the tag, last seen at 09:55, isn't re-dated.
        assert_eq!(
            accept(&mut state, at),
            "New Name null null 2026-10-03T10:00:00Z"
        );
    }

    #[test]
    fn an_older_member_list_does_not_make_a_newer_name_look_older() {
        // The member list at 09:50, then the name at 10:00 (another PC may have sent a name
        // seen at 09:55 meanwhile; ours, seen later, must still win).
        let mut state = calibrated();
        state.auth.user = Some("example".into());
        state.record(&roster(T.as_secs() - 600));
        profile(&mut state, T.as_secs(), "Example Alliance", Some("EXA"));
        let at = Instant::now();
        assert_eq!(accept(&mut state, at), "null null 901 2026-10-03T09:50:00Z");
        assert_eq!(
            accept(&mut state, at + MIN_GAP),
            "Example Alliance EXA null 2026-10-03T10:00:00Z"
        );
    }
}
