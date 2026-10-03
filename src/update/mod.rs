//! Updates from the project's GitHub releases: a check at start-up and every hour after, and,
//! when the user agrees, [`install::install`] swaps in the new exe.

mod http;
pub mod install;

use std::fmt;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::app::state::State;
use crate::util::json::{self, Json};
use crate::util::time::now;

/// The GitHub repository releases come from.
pub const REPO: &str = "lastwarhq-dev/lastwarhq-scanner";

/// How often to look for a new release.
const CHECK_EVERY: Duration = Duration::from_secs(60 * 60);

/// The GitHub API's "latest release" reply is a few KiB.
const MAX_REPLY: usize = 1 << 20;

/// A release version, `major.minor.patch`. Releases are tagged `v` + the version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}

impl Version {
    /// `1.2.3`: three numbers, nothing else.
    pub fn parse(text: &str) -> Option<Version> {
        let mut numbers = [0u32; 3];
        let mut parts = text.split('.');
        for n in &mut numbers {
            let part = parts.next()?;
            if part.is_empty() || part.len() > 9 || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            *n = part.parse().ok()?;
        }
        let [major, minor, patch] = numbers;
        parts.next().is_none().then_some(Version {
            major,
            minor,
            patch,
        })
    }

    /// The version of this exe, from Cargo.toml.
    pub fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION"))
            .expect("Cargo.toml's version is major.minor.patch")
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The version a GitHub "latest release" reply names in its tag. `None` for a tag of another
/// form, such as the `v0.1.0-build.1` of releases made before updates existed.
pub fn release_version(reply: &str) -> Result<Option<Version>, String> {
    let release = json::parse(reply)?;
    let tag = release
        .get("tag_name")
        .and_then(Json::as_str)
        .ok_or("the release has no tag")?;
    Ok(tag.strip_prefix('v').and_then(Version::parse))
}

/// Asks GitHub for the latest release's version.
pub fn latest() -> Result<Option<Version>, String> {
    let reply = http::get(
        "api.github.com",
        &format!("/repos/{REPO}/releases/latest"),
        "Accept: application/vnd.github+json",
        MAX_REPLY,
    )?;
    let text = String::from_utf8(reply).map_err(|_| "the release reply is not UTF-8")?;
    release_version(&text)
}

/// Checks for a new release now and every hour after, for the life of the process. A failed
/// check changes nothing; the next one tries again.
pub fn watch(state: Arc<Mutex<State>>) {
    thread::spawn(move || {
        loop {
            if let Ok(latest) = latest() {
                let mut s = crate::app::lock(&state);
                s.update.latest = latest;
                s.update.checked = Some(now());
            }
            thread::sleep(CHECK_EVERY);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_order() {
        let v = |s| Version::parse(s).unwrap();
        assert_eq!(v("0.2.0").to_string(), "0.2.0");
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert!(v("0.2.1") > v("0.2.0"));
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "1.2.x",
            "1..3",
            "+1.2.3",
            "1.2.3-rc.1",
            " 1.2.3",
            "1.2.9999999999",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn release_reply_gives_its_tag_version() {
        let reply = r#"{"tag_name":"v1.4.2","name":"Example","assets":[{"name":"x.exe"}],"body":"Line\nwith é"}"#;
        assert_eq!(release_version(reply), Ok(Version::parse("1.4.2")));
        // Tags from before updates existed name no version.
        let reply = r#"{"tag_name":"v0.1.0-build.7"}"#;
        assert_eq!(release_version(reply), Ok(None));
        assert_eq!(release_version(r#"{"tag_name":"1.4.2"}"#), Ok(None));
        assert!(release_version(r#"{"message":"Not Found"}"#).is_err());
        assert!(release_version("<html>").is_err());
    }
}
