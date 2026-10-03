//! Signing in to LastWarHQ: the browser sign-in with PKCE (`loopback`), the scanner API
//! (`api`), and the token kept in Windows Credential Manager (`store`).
//!
//! Signing in opens the site's connect page in the browser; once the user approves this PC,
//! the browser comes back to a listener on `127.0.0.1` with a one-time code, which is swapped
//! for a token over HTTPS together with the PKCE verifier. A code seen by anything else is no
//! use without the verifier.
//!
//! Every sign-in and sign-out raises the sign-in generation, under the state lock. Work that
//! started in an earlier generation is dropped when it finishes, and the stored token is only
//! saved or deleted under the same lock, so a late answer can't undo a sign-out.

pub mod api;
pub mod loopback;
pub mod store;

use std::net::{Ipv4Addr, TcpListener};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::app::lock;
use crate::app::state::{AuthStatus, AuthStep, State, SyncStatus};
use crate::util::crypto::{base64url, random_bytes, sha256};
use api::{ApiError, Me, Session};

/// The LastWarHQ site, production.
pub const SITE_HOST: &str = "lastwarhq.dev";

/// How long to wait for the user to approve in the browser. Signing in to the site may mean
/// finding a sign-in link in their mail first.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// At start-up: picks up a saved sign-in and checks it with the site.
pub fn start(state: &Arc<Mutex<State>>) {
    let (saved, generation) = {
        let mut s = lock(state);
        let saved = match store::load() {
            Ok(Some(saved)) => saved,
            Ok(None) => return,
            Err(why) => {
                s.auth.step = AuthStep::Failed(format!("the saved sign-in can't be read: {why}"));
                return;
            }
        };
        s.auth.user = Some(saved.username.clone());
        (saved, s.auth.generation)
    };
    let state = Arc::clone(state);
    thread::spawn(move || check(&state, &saved.token, generation));
}

/// Whether a sign-in or sign-out is under way, so the row can't start another.
pub fn busy(auth: &AuthStatus) -> bool {
    matches!(auth.step, AuthStep::SigningIn | AuthStep::SigningOut)
}

/// Signs in through the browser, on a background thread.
pub fn sign_in(state: &Arc<Mutex<State>>) {
    let generation = {
        let mut s = lock(state);
        if busy(&s.auth) {
            return;
        }
        s.auth.generation += 1;
        s.auth.step = AuthStep::SigningIn;
        s.auth.generation
    };
    let state = Arc::clone(state);
    thread::spawn(move || match browser_sign_in(&state, generation) {
        Ok(token) => check(&state, &token, generation),
        Err(why) => {
            let mut s = lock(&state);
            if s.auth.generation == generation {
                s.auth.step = AuthStep::Failed(why);
            }
        }
    });
}

/// Signs out: removes the token here, and asks the site to disconnect this PC. If the token
/// can't be removed, the user stays signed in unless the site confirms the disconnect, since
/// otherwise the next start would sign them straight back in.
pub fn sign_out(state: &Arc<Mutex<State>>) {
    let (saved, removed, generation) = {
        let mut s = lock(state);
        if busy(&s.auth) {
            return;
        }
        s.auth.generation += 1;
        s.sync = SyncStatus::default();
        let saved = store::load();
        let removed = store::delete();
        let generation = s.auth.generation;
        if removed.is_ok() {
            s.auth = AuthStatus {
                generation,
                ..AuthStatus::default()
            };
        } else {
            s.auth.step = AuthStep::SigningOut;
        }
        (saved, removed, generation)
    };
    let state = Arc::clone(state);
    thread::spawn(move || {
        let revoked = revoke(saved, api::logout);
        let mut s = lock(&state);
        if s.auth.generation == generation {
            finish_sign_out(&mut s.auth, removed, revoked);
        }
    });
}

/// Asks the site to disconnect the token read for a sign-out, through `logout`. `Ok` only
/// when the site confirms the token no longer works: it disconnected the PC, or no longer
/// knew the token. A token that couldn't be read, or wasn't found, can't be revoked.
fn revoke(
    saved: Result<Option<Session>, String>,
    logout: impl FnOnce(&str) -> Result<(), ApiError>,
) -> Result<(), String> {
    match saved {
        Ok(Some(saved)) => match logout(&saved.token) {
            Ok(()) | Err(ApiError::Unauthorized) => Ok(()),
            Err(other) => Err(other.to_string()),
        },
        Ok(None) => Err("no saved token was found to revoke".into()),
        Err(why) => Err(format!("the saved token couldn't be read: {why}")),
    }
}

/// Settles a sign-out. Removing the token here is enough; failing that, only the site's
/// confirmation that the token no longer works signs the user out. Otherwise they are still
/// signed in, as the next start would use the token again.
fn finish_sign_out(
    auth: &mut AuthStatus,
    removed: Result<(), String>,
    revoked: Result<(), String>,
) {
    let Err(why) = removed else {
        return;
    };
    match revoked {
        Ok(()) => {
            *auth = AuthStatus {
                step: AuthStep::Failed(format!(
                    "signed out, but the old token is still in Credential Manager: {why}"
                )),
                generation: auth.generation,
                ..AuthStatus::default()
            };
        }
        Err(_) => auth.step = AuthStep::Failed(format!("sign-out failed: {why}")),
    }
}

/// Prepares PKCE, listens on `127.0.0.1`, opens the browser and waits for the code, swaps it
/// for a token and stores it, and only then tells the browser how it went. Returns the token.
fn browser_sign_in(state: &Mutex<State>, generation: u64) -> Result<String, String> {
    let verifier = base64url(&random_bytes::<32>()?);
    let challenge = base64url(&sha256(verifier.as_bytes())?);
    let pkce_state = base64url(&random_bytes::<32>()?);
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|e| format!("cannot listen for the browser: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("cannot listen for the browser: {e}"))?
        .port();
    let url = loopback::connect_url(port, &pkce_state, &challenge, &loopback::device_name());
    loopback::open_browser(&url)?;
    let callback = loopback::wait_for_callback(&listener, &pkce_state, SIGN_IN_TIMEOUT)?;
    drop(listener);
    let session = api::token(&callback.code, &verifier)
        .map_err(|e| e.to_string())
        .and_then(|session| keep(state, generation, session));
    callback.answer(
        session
            .as_ref()
            .map(|s| s.username.as_str())
            .map_err(String::as_str),
    );
    session.map(|s| s.token)
}

/// Stores a new sign-in, unless it has been overtaken by another sign-in or sign-out.
fn keep(state: &Mutex<State>, generation: u64, session: Session) -> Result<Session, String> {
    let mut s = lock(state);
    if s.auth.generation != generation {
        return Err("the sign-in was cancelled".into());
    }
    s.sync = SyncStatus::default();
    store::save(&session)
        .map_err(|e| format!("signed in, but the token couldn't be saved: {e}"))?;
    s.auth = AuthStatus {
        user: Some(session.username.clone()),
        generation,
        ..AuthStatus::default()
    };
    Ok(session)
}

/// Asks the site who the token belongs to and which alliances they manage. A token the site
/// refuses has been disconnected: it is deleted, and the user signs in again.
fn check(state: &Mutex<State>, token: &str, generation: u64) {
    {
        let mut s = lock(state);
        if s.auth.generation != generation {
            return;
        }
        s.auth.step = AuthStep::Checking;
    }
    let result = api::me(token);
    let mut s = lock(state);
    if apply_check(&mut s.auth, generation, result) {
        delete_refused(&mut s.auth);
    }
}

/// LastWarHQ answered `401` to the current sign-in's token (call under the state lock, after
/// checking the generation): the PC was disconnected. Signs out and deletes the token.
pub fn token_refused(auth: &mut AuthStatus) {
    mark_refused(auth);
    delete_refused(auth);
}

/// Marks the user signed out by a refused token. The generation is raised, as for a sign-out,
/// so a call still on its way with the old token (a `me` check, say) can't sign them back in.
fn mark_refused(auth: &mut AuthStatus) {
    *auth = AuthStatus {
        step: AuthStep::Failed("this PC was disconnected".into()),
        generation: auth.generation + 1,
        ..AuthStatus::default()
    };
}

fn delete_refused(auth: &mut AuthStatus) {
    if let Err(why) = store::delete() {
        auth.step = AuthStep::Failed(format!(
            "this PC was disconnected, but the old token couldn't be removed: {why}"
        ));
    }
}

/// Applies a `me` answer asked for in `generation`; an answer from an earlier generation
/// changes nothing. Returns whether the stored token must be deleted.
fn apply_check(auth: &mut AuthStatus, generation: u64, result: Result<Me, ApiError>) -> bool {
    if auth.generation != generation {
        return false;
    }
    match result {
        Ok(me) => {
            *auth = AuthStatus {
                user: Some(me.username),
                alliances: Some(me.alliances),
                step: AuthStep::Idle,
                generation,
            };
            false
        }
        Err(ApiError::Unauthorized) => {
            mark_refused(auth);
            true
        }
        Err(other) => {
            auth.step = AuthStep::Failed(other.to_string());
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_in(generation: u64) -> AuthStatus {
        AuthStatus {
            user: Some("example".into()),
            alliances: None,
            step: AuthStep::Checking,
            generation,
        }
    }

    fn me_answer() -> Me {
        Me {
            username: "example".into(),
            alliances: vec!["0123456789abcdef0123456789abcdef".into()],
        }
    }

    #[test]
    fn a_check_finishing_after_sign_out_changes_nothing() {
        // `me` was asked in generation 1; the user signed out (generation 2) meanwhile.
        let mut auth = AuthStatus {
            generation: 2,
            ..AuthStatus::default()
        };
        assert!(!apply_check(&mut auth, 1, Ok(me_answer())));
        assert_eq!(auth.user, None);
        assert_eq!(auth.step, AuthStep::Idle);
        // A refusal from the old generation doesn't delete whatever is stored now.
        assert!(!apply_check(&mut auth, 1, Err(ApiError::Unauthorized)));
        assert_eq!(auth.step, AuthStep::Idle);
    }

    #[test]
    fn a_check_finishing_after_a_sync_refused_the_token_changes_nothing() {
        // `me` was asked in generation 5; then a sync was answered 401.
        let mut auth = signed_in(5);
        mark_refused(&mut auth);
        assert_eq!(auth.user, None);
        assert_eq!(auth.generation, 6);
        // The `me` answer arrives late, and successful: it must not sign the user back in.
        assert!(!apply_check(&mut auth, 5, Ok(me_answer())));
        assert_eq!(auth.user, None);
        assert_eq!(
            auth.step,
            AuthStep::Failed("this PC was disconnected".into())
        );
    }

    #[test]
    fn a_current_check_is_applied() {
        let mut auth = signed_in(3);
        assert!(!apply_check(&mut auth, 3, Ok(me_answer())));
        assert_eq!(auth.step, AuthStep::Idle);
        assert_eq!(auth.alliances.as_ref().map(Vec::len), Some(1));

        let mut auth = signed_in(3);
        assert!(apply_check(&mut auth, 3, Err(ApiError::Unauthorized)));
        assert_eq!(auth.user, None, "a refused token signs the user out");

        let mut auth = signed_in(3);
        assert!(!apply_check(
            &mut auth,
            3,
            Err(ApiError::Failed("lastwarhq.dev: no connection".into()))
        ));
        assert_eq!(
            auth.user.as_deref(),
            Some("example"),
            "offline is not signed out"
        );
    }

    #[test]
    fn sign_out_is_only_claimed_once_the_token_is_dead() {
        let refused = || Err("Credential Manager refused to remove it (error 5)".to_string());
        let signing_out = || AuthStatus {
            step: AuthStep::SigningOut,
            ..signed_in(4)
        };
        let failed = AuthStep::Failed(
            "sign-out failed: Credential Manager refused to remove it (error 5)".into(),
        );

        // Removing the token failed, and so did telling the site.
        let mut auth = signing_out();
        let revoked = revoke(Ok(Some(session())), |_| {
            Err(ApiError::Failed("lastwarhq.dev: no connection".into()))
        });
        finish_sign_out(&mut auth, refused(), revoked);
        assert_eq!(auth.user.as_deref(), Some("example"));
        assert_eq!(auth.step, failed);

        // Reading and removing the token both failed: there was nothing to revoke it with.
        let mut auth = signing_out();
        let revoked = revoke(
            Err("Credential Manager refused to read it (error 5)".into()),
            |_| panic!("no token to send"),
        );
        assert_eq!(
            revoked,
            Err(
                "the saved token couldn't be read: Credential Manager refused to read it (error 5)"
                    .into()
            )
        );
        finish_sign_out(&mut auth, refused(), revoked);
        assert_eq!(auth.user.as_deref(), Some("example"));
        assert_eq!(auth.step, failed);

        // Nothing was found to revoke, and removing failed: not claimed either.
        let mut auth = signing_out();
        finish_sign_out(&mut auth, refused(), revoke(Ok(None), |_| Ok(())));
        assert_eq!(auth.user.as_deref(), Some("example"));

        // Removing it failed, but the site disconnected the PC (or no longer knew the token).
        for answer in [Ok(()), Err(ApiError::Unauthorized)] {
            let mut auth = signing_out();
            let revoked = revoke(Ok(Some(session())), |token| {
                assert_eq!(token, "t0k3n");
                answer.clone()
            });
            finish_sign_out(&mut auth, refused(), revoked);
            assert_eq!(auth.user, None);
            assert_eq!(auth.generation, 4);
        }

        // Removing it worked: signed out whatever the site says.
        let mut auth = AuthStatus {
            generation: 4,
            ..AuthStatus::default()
        };
        finish_sign_out(
            &mut auth,
            Ok(()),
            Err("lastwarhq.dev: no connection".into()),
        );
        assert_eq!(auth.user, None);
        assert_eq!(auth.step, AuthStep::Idle);
    }

    fn session() -> Session {
        Session {
            token: "t0k3n".into(),
            username: "example".into(),
        }
    }
}
