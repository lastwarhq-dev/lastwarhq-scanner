//! Installing a release: download its exe, check it, and put it in place of the running one.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::net::http;
use crate::update::{REPO, Version};
use crate::util::crypto::sha256;

/// The release asset that is the tool, and its checksum file, `<name>.sha256`.
const EXE_NAME: &str = "lastwarhq-scanner.exe";

/// The exe is under 1 MiB; anything far larger is not it.
const MAX_EXE: usize = 64 << 20;

/// Starting the tool with this argument makes it wait for the version it replaced to close.
pub const UPDATED_ARG: &str = "--updated";

/// Downloads release `version`'s exe from GitHub, checks it against the release's SHA-256,
/// and puts it in place of the running exe. Returns the exe's path, to start the new version
/// from: once the running exe has been renamed, Windows still reports it by its old path.
pub fn install(version: Version) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find the running exe: {e}"))?;
    let url = format!("/{REPO}/releases/download/v{version}/{EXE_NAME}");
    let checksum = http::get("github.com", &format!("{url}.sha256"), "", 1024)?;
    let expected = parse_checksum(&checksum)?;
    let bytes = http::get("github.com", &url, "", MAX_EXE)?;
    if !bytes.starts_with(b"MZ") {
        return Err("the download is not a Windows program".into());
    }
    if sha256(&bytes)? != expected {
        return Err("the download doesn't match its SHA-256".into());
    }
    replace_exe(&exe, &bytes)?;
    Ok(exe)
}

/// The hash in a `sha256sum` line: 64 hex digits, then the file name.
fn parse_checksum(file: &[u8]) -> Result<[u8; 32], String> {
    let bad = || "the release's SHA-256 file is not readable".to_string();
    let hex = file.get(..64).ok_or_else(bad)?;
    if file.get(64).is_some_and(|b| !b.is_ascii_whitespace()) {
        return Err(bad());
    }
    let mut hash = [0u8; 32];
    for (byte, pair) in hash.iter_mut().zip(hex.chunks(2)) {
        let pair = std::str::from_utf8(pair).map_err(|_| bad())?;
        *byte = u8::from_str_radix(pair, 16).map_err(|_| bad())?;
    }
    Ok(hash)
}

/// `exe` with `suffix` added: `x.exe` → `x.exe.old`.
fn beside(exe: &Path, suffix: &str) -> PathBuf {
    let mut path = exe.as_os_str().to_owned();
    path.push(suffix);
    path.into()
}

/// Puts `bytes` in place of `exe`. Windows won't overwrite a running exe but lets it be
/// renamed, so the new exe is written beside it as `<exe>.new`, the running exe moves to
/// `<exe>.old` (removed by [`clean_up`] at the next start), and the new one takes its name.
/// If that fails, the running exe is moved back.
pub fn replace_exe(exe: &Path, bytes: &[u8]) -> Result<(), String> {
    let (new, old) = (beside(exe, ".new"), beside(exe, ".old"));
    fs::write(&new, bytes).map_err(|e| format!("cannot save the update beside the exe: {e}"))?;
    // An older update's exe, if it could not be removed at the last start.
    let _ = fs::remove_file(&old);
    if let Err(e) = fs::rename(exe, &old) {
        let _ = fs::remove_file(&new);
        return Err(format!("cannot move the running exe aside: {e}"));
    }
    if let Err(e) = fs::rename(&new, exe) {
        let _ = fs::rename(&old, exe);
        let _ = fs::remove_file(&new);
        return Err(format!("cannot put the update in place: {e}"));
    }
    Ok(())
}

/// Removes what an update left beside `exe`: an unfinished download, and the exe it replaced.
/// That one stays locked until its process has fully closed, so removing it is retried for a
/// few seconds on a background thread.
pub fn clean_up(exe: PathBuf) {
    let _ = fs::remove_file(beside(&exe, ".new"));
    let old = beside(&exe, ".old");
    if !old.exists() {
        return;
    }
    thread::spawn(move || {
        for _ in 0..20 {
            if fs::remove_file(&old).is_ok() || !old.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(500));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_file_is_read() {
        let line = b"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  lastwarhq-scanner.exe\n";
        assert_eq!(parse_checksum(line), sha256(b"abc"));
        // Upper case is hex too.
        let upper = line.to_ascii_uppercase();
        assert_eq!(parse_checksum(&upper), sha256(b"abc"));
        for bad in [
            &b""[..],
            &line[..63],
            b"zz7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  x",
            b"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad0 x",
        ] {
            assert!(parse_checksum(bad).is_err());
        }
    }

    /// A folder of its own under the system temp folder, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir = std::env::temp_dir().join(format!(
                "lastwarhq-scanner-test-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn replacing_moves_the_old_exe_aside() {
        let dir = TempDir::new("replace");
        let exe = dir.0.join("tool.exe");
        fs::write(&exe, b"old").unwrap();
        // Left over from an earlier update.
        fs::write(beside(&exe, ".old"), b"older").unwrap();
        replace_exe(&exe, b"new").unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"new");
        assert_eq!(fs::read(beside(&exe, ".old")).unwrap(), b"old");
        assert!(!beside(&exe, ".new").exists());
    }

    #[test]
    fn failed_replace_keeps_the_running_exe() {
        let dir = TempDir::new("replace-fails");
        // There is no exe to move aside.
        let exe = dir.0.join("missing.exe");
        assert!(replace_exe(&exe, b"new").is_err());
        assert!(!exe.exists());
        assert!(!beside(&exe, ".new").exists());
    }

    #[test]
    fn clean_up_removes_leftovers() {
        let dir = TempDir::new("clean-up");
        let exe = dir.0.join("tool.exe");
        fs::write(&exe, b"exe").unwrap();
        fs::write(beside(&exe, ".new"), b"partial").unwrap();
        clean_up(exe.clone());
        assert!(!beside(&exe, ".new").exists());
        assert!(exe.exists());
    }
}
