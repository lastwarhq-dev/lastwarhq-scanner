//! The sign-in token, kept in Windows Credential Manager as a generic credential named
//! "LastWarHQ Scanner". Windows encrypts it for the signed-in Windows user; it shows under
//! Control Panel → Credential Manager → Windows Credentials, where it can also be removed.

#![allow(clippy::upper_case_acronyms)]

use std::ffi::c_void;

use crate::auth::api::Session;

#[repr(C)]
struct FILETIME {
    low: u32,
    high: u32,
}

#[repr(C)]
struct CREDENTIALW {
    flags: u32,
    kind: u32,
    target_name: *mut u16,
    comment: *mut u16,
    last_written: FILETIME,
    blob_size: u32,
    blob: *mut u8,
    persist: u32,
    attribute_count: u32,
    attributes: *mut c_void,
    target_alias: *mut u16,
    user_name: *mut u16,
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn CredWriteW(credential: *const CREDENTIALW, flags: u32) -> i32;
    fn CredReadW(
        target: *const u16,
        kind: u32,
        flags: u32,
        credential: *mut *mut CREDENTIALW,
    ) -> i32;
    fn CredDeleteW(target: *const u16, kind: u32, flags: u32) -> i32;
    fn CredFree(buffer: *mut c_void);
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLastError() -> u32;
}

const CRED_TYPE_GENERIC: u32 = 1;
const ERROR_NOT_FOUND: u32 = 1168;
/// Kept for this Windows user on this PC; not roamed to other PCs.
const CRED_PERSIST_LOCAL_MACHINE: u32 = 2;
/// The most a credential can hold.
const CRED_MAX_CREDENTIAL_BLOB_SIZE: usize = 5 * 512;

const TARGET: &str = "LastWarHQ Scanner";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Saves the token, with its username, in place of any saved before.
pub fn save(session: &Session) -> Result<(), String> {
    let token = session.token.as_bytes();
    if token.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE {
        return Err("the token is too long for Credential Manager".into());
    }
    let mut target = wide(TARGET);
    let mut comment = wide("LastWarHQ sign-in for LastWarHQ Scanner");
    let mut user = wide(&session.username);
    let mut blob = token.to_vec();
    let credential = CREDENTIALW {
        flags: 0,
        kind: CRED_TYPE_GENERIC,
        target_name: target.as_mut_ptr(),
        comment: comment.as_mut_ptr(),
        last_written: FILETIME { low: 0, high: 0 },
        blob_size: blob.len() as u32,
        blob: blob.as_mut_ptr(),
        persist: CRED_PERSIST_LOCAL_MACHINE,
        attribute_count: 0,
        attributes: std::ptr::null_mut(),
        target_alias: std::ptr::null_mut(),
        user_name: user.as_mut_ptr(),
    };
    // SAFETY: every pointer in `credential` points at a live buffer of the size given, and
    // CredWriteW copies what it keeps.
    if unsafe { CredWriteW(&credential, 0) } == 0 {
        // SAFETY: GetLastError has no preconditions.
        let code = unsafe { GetLastError() };
        return Err(format!("Credential Manager refused it (error {code})"));
    }
    Ok(())
}

/// The saved token and username: `None` if nothing is saved, an error if something is saved
/// but can't be read (Credential Manager refused, or the token isn't text). Only "nothing
/// saved" may be taken to mean there is no token.
pub fn load() -> Result<Option<Session>, String> {
    let target = wide(TARGET);
    let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `target` is NUL-terminated; on success Windows hands back a credential that is
    // read here and then freed with CredFree, once. GetLastError has no preconditions.
    unsafe {
        if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) == 0 {
            return match GetLastError() {
                ERROR_NOT_FOUND => Ok(None),
                code => Err(format!(
                    "Credential Manager refused to read it (error {code})"
                )),
            };
        }
        let c = &*credential;
        let token = (!c.blob.is_null()).then(|| {
            let bytes = std::slice::from_raw_parts(c.blob, c.blob_size as usize);
            String::from_utf8(bytes.to_vec()).ok()
        });
        let token = token.flatten();
        let user = (!c.user_name.is_null()).then(|| {
            let len = (0..).take_while(|&i| *c.user_name.add(i) != 0).count();
            String::from_utf16_lossy(std::slice::from_raw_parts(c.user_name, len))
        });
        CredFree(credential.cast());
        let token = token
            .filter(|t| !t.is_empty())
            .ok_or("the saved token isn't readable")?;
        Ok(Some(Session {
            token,
            username: user.unwrap_or_default(),
        }))
    }
}

/// Removes the saved token. Having none to remove counts as removed.
pub fn delete() -> Result<(), String> {
    let target = wide(TARGET);
    // SAFETY: `target` is NUL-terminated; GetLastError has no preconditions.
    unsafe {
        if CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) != 0 {
            return Ok(());
        }
        match GetLastError() {
            ERROR_NOT_FOUND => Ok(()),
            code => Err(format!(
                "Credential Manager refused to remove it (error {code})"
            )),
        }
    }
}
