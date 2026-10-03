//! The scanner API: plain JSON over HTTPS under `https://<site>/scanner-api/v1/`. A success is
//! `200` with the data as its JSON body, or `204` with none. A failure has an HTTP status and
//! `{"error": {"code": "unauthorized", "message": "..."}}`; it is told apart by the status and
//! `code`, and `message` is only shown.

use std::fmt;

use crate::auth::SITE_HOST;
use crate::net::http;
use crate::util::json::{self, Json, escape};

/// Answers are a few KiB at most.
const MAX_REPLY: usize = 1 << 20;

/// Why a call failed.
#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    /// `401`: the token is missing, revoked or expired. Delete it and sign in again.
    Unauthorized,
    /// `400 invalid_grant` from `token`: the code is unknown, used, expired or doesn't match
    /// the verifier. Start the sign-in again.
    InvalidGrant,
    /// `400 bad_request`: the body was refused; the message says what's wrong. Sending the
    /// same again would be refused again.
    BadRequest(String),
    /// `404 unknown_alliance` from `sync`: the alliance hasn't been added on LastWarHQ, or
    /// the user doesn't manage it.
    UnknownAlliance,
    /// `429 too_many_requests`: wait this many seconds (`Retry-After`), if given.
    TooManyRequests(Option<u64>),
    Failed(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Unauthorized => f.write_str("LastWarHQ no longer accepts this PC's sign-in"),
            ApiError::InvalidGrant => f.write_str("the sign-in expired or was refused"),
            ApiError::BadRequest(why) => write!(f, "LastWarHQ refused the data: {why}"),
            ApiError::UnknownAlliance => {
                f.write_str("LastWarHQ doesn't have this alliance for this account")
            }
            ApiError::TooManyRequests(_) => f.write_str("LastWarHQ asked to wait before syncing"),
            ApiError::Failed(why) => f.write_str(why),
        }
    }
}

/// `POST /v1/sync`'s answer: what happened to each section (`applied`, `unchanged`, `stale`,
/// `pending` or `missing`), and how many seconds to wait before the next sync.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SyncReply {
    pub alliance: String,
    pub roster: String,
    pub ds_signups: String,
    pub ds_results: String,
    /// Monday to Saturday.
    pub vs: Vec<String>,
    pub next_sync_after: Option<u64>,
}

/// A token from a finished sign-in, and who it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub token: String,
    pub username: String,
}

/// `GET /v1/me`: who the token belongs to, and the ids of the alliances they manage.
#[derive(Debug, Clone, PartialEq)]
pub struct Me {
    pub username: String,
    pub alliances: Vec<String>,
}

/// `POST /v1/token`: swaps the code from the browser's callback for a token.
pub fn token(code: &str, verifier: &str) -> Result<Session, ApiError> {
    let body = format!(
        "{{\"code\":{},\"verifier\":{}}}",
        escape(code),
        escape(verifier)
    );
    session(&call("POST", "token", None, Some(&body))?)
}

/// `GET /v1/me`, with the stored token.
pub fn me(token: &str) -> Result<Me, ApiError> {
    parse_me(&call("GET", "me", Some(token), None)?)
}

/// `POST /v1/sync`: uploads the payload.
pub fn sync(token: &str, payload: &str) -> Result<SyncReply, ApiError> {
    Ok(parse_sync(&call(
        "POST",
        "sync",
        Some(token),
        Some(payload),
    )?))
}

/// `POST /v1/logout`: disconnects this PC, so its token stops working.
pub fn logout(token: &str) -> Result<(), ApiError> {
    call("POST", "logout", Some(token), None).map(|_| ())
}

/// Calls `endpoint` under `/scanner-api/v1/`, with the token if given and `body` as JSON if
/// given. Returns the answer's JSON body, or `null` for a `204`.
fn call(
    method: &str,
    endpoint: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> Result<Json, ApiError> {
    let bearer = token.map(|t| format!("Authorization: Bearer {t}"));
    let mut headers = Vec::new();
    if body.is_some() {
        headers.push("Content-Type: application/json");
    }
    if let Some(bearer) = &bearer {
        headers.push(bearer.as_str());
    }
    let response = http::request(
        method,
        SITE_HOST,
        &format!("/scanner-api/v1/{endpoint}"),
        &headers,
        body.unwrap_or_default().as_bytes(),
        MAX_REPLY,
    )
    .map_err(ApiError::Failed)?;
    reply(response.status, &response.body, response.retry_after)
}

/// A successful answer's JSON body (`null` for `204`), or the error its status and `code`
/// name. Any `401` means the token is no good, and any `429` means wait, whatever the body
/// says.
fn reply(status: u32, body: &[u8], retry_after: Option<u32>) -> Result<Json, ApiError> {
    let parsed = std::str::from_utf8(body)
        .ok()
        .and_then(|text| json::parse(text).ok());
    match status {
        200 => parsed
            .ok_or_else(|| ApiError::Failed("LastWarHQ sent an answer that isn't readable".into())),
        204 => Ok(Json::Null),
        401 => Err(ApiError::Unauthorized),
        429 => Err(ApiError::TooManyRequests(retry_after.map(u64::from))),
        _ => {
            let error = parsed.as_ref().and_then(|p| p.get("error"));
            let field = |key| error.and_then(|e| e.get(key)).and_then(Json::as_str);
            match (field("code"), field("message")) {
                (Some("invalid_grant"), _) if status == 400 => Err(ApiError::InvalidGrant),
                (Some("unknown_alliance"), _) if status == 404 => Err(ApiError::UnknownAlliance),
                (Some("bad_request"), message) if status == 400 => Err(ApiError::BadRequest(
                    message.unwrap_or("no reason given").to_string(),
                )),
                (_, Some(message)) => Err(ApiError::Failed(format!("LastWarHQ: {message}"))),
                (Some(code), None) => Err(ApiError::Failed(format!("LastWarHQ answered {code}"))),
                (None, None) => Err(ApiError::Failed(format!(
                    "LastWarHQ answered HTTP {status}"
                ))),
            }
        }
    }
}

fn text<'a>(data: &'a Json, key: &str) -> Result<&'a str, ApiError> {
    data.get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| ApiError::Failed(format!("LastWarHQ's answer has no {key}")))
}

fn session(data: &Json) -> Result<Session, ApiError> {
    Ok(Session {
        token: text(data, "token")?.to_string(),
        username: text(data, "username")?.to_string(),
    })
}

fn parse_sync(data: &Json) -> SyncReply {
    let sections = data.get("sections");
    let status = |key: &str| {
        sections
            .and_then(|s| s.get(key))
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let vs = sections
        .and_then(|s| s.get("vs"))
        .and_then(Json::as_array)
        .unwrap_or_default()
        .iter()
        .map(|day| day.as_str().unwrap_or_default().to_string())
        .collect();
    let next = match data.get("nextSyncAfter") {
        Some(Json::Number(n)) if *n >= 0.0 => Some(*n as u64),
        _ => None,
    };
    SyncReply {
        alliance: status("alliance"),
        roster: status("roster"),
        ds_signups: status("dsSignups"),
        ds_results: status("dsResults"),
        vs,
        next_sync_after: next,
    }
}

fn parse_me(data: &Json) -> Result<Me, ApiError> {
    let alliances = data
        .get("alliances")
        .and_then(Json::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|a| a.get("id")?.as_str().map(str::to_string))
        .collect();
    Ok(Me {
        username: text(data, "username")?.to_string(),
        alliances,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_answers_give_their_body() {
        let data = reply(
            200,
            br#"{"token":"t0k3n","username":"example","deviceName":"PC"}"#,
            None,
        )
        .unwrap();
        assert_eq!(
            session(&data),
            Ok(Session {
                token: "t0k3n".into(),
                username: "example".into(),
            })
        );
        // `logout` answers 204 with no body.
        assert_eq!(reply(204, b"", None), Ok(Json::Null));
        assert!(reply(200, b"<html>", None).is_err());
        assert!(session(&Json::Null).is_err());
    }

    #[test]
    fn errors_are_told_apart_by_status_and_code() {
        let unauthorized = br#"{"error":{"code":"unauthorized","message":"Sign in again"}}"#;
        assert_eq!(reply(401, unauthorized, None), Err(ApiError::Unauthorized));
        // Any 401 means the token is no good, even without the API's body.
        assert_eq!(reply(401, b"", None), Err(ApiError::Unauthorized));
        let invalid_grant = br#"{"error":{"code":"invalid_grant","message":"Code expired"}}"#;
        assert_eq!(reply(400, invalid_grant, None), Err(ApiError::InvalidGrant));
        let bad_request =
            br#"{"error":{"code":"bad_request","message":"roster.players[3].name is too long"}}"#;
        assert_eq!(
            reply(400, bad_request, None),
            Err(ApiError::BadRequest(
                "roster.players[3].name is too long".into()
            ))
        );
        let unknown = br#"{"error":{"code":"unknown_alliance","message":"Not yours"}}"#;
        assert_eq!(reply(404, unknown, None), Err(ApiError::UnknownAlliance));
        let too_many = br#"{"error":{"code":"too_many_requests","message":"Wait"}}"#;
        assert_eq!(
            reply(429, too_many, Some(42)),
            Err(ApiError::TooManyRequests(Some(42)))
        );
        assert_eq!(reply(429, b"", None), Err(ApiError::TooManyRequests(None)));
        assert_eq!(
            reply(500, br#"{"error":{"code":"internal_error"}}"#, None),
            Err(ApiError::Failed("LastWarHQ answered internal_error".into()))
        );
        assert_eq!(
            reply(502, b"Bad gateway", None),
            Err(ApiError::Failed("LastWarHQ answered HTTP 502".into()))
        );
    }

    #[test]
    fn sync_answers_say_what_happened_to_each_section() {
        let data = reply(
            200,
            br#"{"sections":{"alliance":"applied","roster":"pending","dsSignups":"pending","dsResults":"missing","vs":["pending","pending","missing","missing","missing","missing"]},"nextSyncAfter":60}"#,
            None,
        )
        .unwrap();
        assert_eq!(
            parse_sync(&data),
            SyncReply {
                alliance: "applied".into(),
                roster: "pending".into(),
                ds_signups: "pending".into(),
                ds_results: "missing".into(),
                vs: [
                    "pending", "pending", "missing", "missing", "missing", "missing"
                ]
                .map(String::from)
                .to_vec(),
                next_sync_after: Some(60),
            }
        );
        assert_eq!(parse_sync(&Json::Null), SyncReply::default());
    }

    #[test]
    fn me_lists_the_alliances_ids() {
        let data = reply(
            200,
            br#"{"username":"example","deviceName":"PC","alliances":[{"id":"0123456789abcdef0123456789abcdef","name":"Example Alliance","abbr":"EXA","warzoneId":901},{"id":"fedcba9876543210fedcba9876543210","name":null,"abbr":null,"warzoneId":null}]}"#,
            None,
        )
        .unwrap();
        assert_eq!(
            parse_me(&data),
            Ok(Me {
                username: "example".into(),
                alliances: vec![
                    "0123456789abcdef0123456789abcdef".into(),
                    "fedcba9876543210fedcba9876543210".into()
                ],
            })
        );
        assert!(parse_me(&Json::Null).is_err());
    }
}
