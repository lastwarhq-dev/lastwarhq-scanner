//! HTTPS requests through WinHTTP, the HTTP client built into Windows: TLS, the system proxy
//! and redirects come from Windows, through hand-written declarations, no crates.

#![allow(clippy::upper_case_acronyms)]

use std::ffi::c_void;

use crate::update::Version;

type HINTERNET = *mut c_void;

#[link(name = "winhttp")]
unsafe extern "system" {
    fn WinHttpOpen(
        agent: *const u16,
        access_type: u32,
        proxy: *const u16,
        bypass: *const u16,
        flags: u32,
    ) -> HINTERNET;
    fn WinHttpSetTimeouts(
        handle: HINTERNET,
        resolve: i32,
        connect: i32,
        send: i32,
        receive: i32,
    ) -> i32;
    fn WinHttpConnect(
        session: HINTERNET,
        server: *const u16,
        port: u16,
        reserved: u32,
    ) -> HINTERNET;
    fn WinHttpOpenRequest(
        connect: HINTERNET,
        verb: *const u16,
        object: *const u16,
        version: *const u16,
        referrer: *const u16,
        accept_types: *const *const u16,
        flags: u32,
    ) -> HINTERNET;
    fn WinHttpSendRequest(
        request: HINTERNET,
        headers: *const u16,
        headers_len: u32,
        optional: *const c_void,
        optional_len: u32,
        total_len: u32,
        context: usize,
    ) -> i32;
    fn WinHttpReceiveResponse(request: HINTERNET, reserved: *mut c_void) -> i32;
    fn WinHttpQueryHeaders(
        request: HINTERNET,
        info_level: u32,
        name: *const u16,
        buffer: *mut c_void,
        buffer_len: *mut u32,
        index: *mut u32,
    ) -> i32;
    fn WinHttpReadData(
        request: HINTERNET,
        buffer: *mut c_void,
        to_read: u32,
        read: *mut u32,
    ) -> i32;
    fn WinHttpCloseHandle(handle: HINTERNET) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLastError() -> u32;
}

/// Uses the system's proxy settings, or none.
const WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY: u32 = 4;
const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
const WINHTTP_QUERY_RETRY_AFTER: u32 = 36;
const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;
const HTTPS_PORT: u16 = 443;

/// Closes a WinHTTP handle when dropped.
struct Handle(HINTERNET);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from WinHTTP and is closed only here.
        unsafe { WinHttpCloseHandle(self.0) };
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Why the last WinHTTP call failed, for `what`.
fn failure(what: &str) -> String {
    // SAFETY: GetLastError has no preconditions.
    let code = unsafe { GetLastError() };
    let reason = match code {
        12002 => "timed out",
        12007 | 12029 => "no connection",
        12175 => "the secure connection failed",
        _ => return format!("{what} failed (WinHTTP error {code})"),
    };
    format!("{what}: {reason}")
}

/// An answer: its HTTP status and body.
pub struct Response {
    pub status: u32,
    pub body: Vec<u8>,
    /// The `Retry-After` header, in seconds, if the server sent one as a number.
    pub retry_after: Option<u32>,
}

/// Fetches `https://{host}{path}` with an extra request header (or `""` for none), following
/// redirects. Fails on any status but 200, or if the body is longer than `limit` bytes.
pub fn get(host: &str, path: &str, header: &str, limit: usize) -> Result<Vec<u8>, String> {
    let headers: &[&str] = if header.is_empty() { &[] } else { &[header] };
    let response = request("GET", host, path, headers, &[], limit)?;
    if response.status != 200 {
        return Err(format!("{host} answered HTTP {}", response.status));
    }
    Ok(response.body)
}

/// Sends `method` to `https://{host}{path}` with extra request headers (each `Name: value`)
/// and `body` (empty for none), following redirects. Returns the answer whatever its status;
/// fails if it can't be had, or if its body is longer than `limit` bytes.
pub fn request(
    method: &str,
    host: &str,
    path: &str,
    headers: &[&str],
    body: &[u8],
    limit: usize,
) -> Result<Response, String> {
    let agent = wide(&format!("LastWarHQ-Scanner/{}", Version::current()));
    let header = headers.join("\r\n");
    let (host_w, path_w, verb, header_w) = (wide(host), wide(path), wide(method), wide(&header));
    let body_len = u32::try_from(body.len()).map_err(|_| "request body too large")?;
    // SAFETY: every string is NUL-terminated UTF-16 that outlives the calls using it; every
    // handle is checked for null before use and closed by `Handle`, the request before the
    // connection before the session (locals drop in reverse order); out-parameters point at
    // locals of the size given; the body outlives the send, which reads exactly its length.
    unsafe {
        let session = WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            std::ptr::null(),
            std::ptr::null(),
            0,
        );
        if session.is_null() {
            return Err(failure("starting WinHTTP"));
        }
        let session = Handle(session);
        WinHttpSetTimeouts(session.0, 15_000, 15_000, 15_000, 30_000);
        let connect = WinHttpConnect(session.0, host_w.as_ptr(), HTTPS_PORT, 0);
        if connect.is_null() {
            return Err(failure(host));
        }
        let connect = Handle(connect);
        let request = WinHttpOpenRequest(
            connect.0,
            verb.as_ptr(),
            path_w.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        );
        if request.is_null() {
            return Err(failure(host));
        }
        let request = Handle(request);
        let (headers, headers_len) = if header.is_empty() {
            (std::ptr::null(), 0)
        } else {
            (header_w.as_ptr(), u32::MAX)
        };
        let body_ptr = if body.is_empty() {
            std::ptr::null()
        } else {
            body.as_ptr().cast()
        };
        if WinHttpSendRequest(
            request.0,
            headers,
            headers_len,
            body_ptr,
            body_len,
            body_len,
            0,
        ) == 0
            || WinHttpReceiveResponse(request.0, std::ptr::null_mut()) == 0
        {
            return Err(failure(host));
        }
        let mut status: u32 = 0;
        let mut len = size_of::<u32>() as u32;
        if WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            (&raw mut status).cast(),
            &mut len,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(failure(host));
        }
        // Absent unless the server asks to wait (`429`, `503`).
        let mut retry_after: u32 = 0;
        let mut len = size_of::<u32>() as u32;
        let retry_after = (WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_RETRY_AFTER | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            (&raw mut retry_after).cast(),
            &mut len,
            std::ptr::null_mut(),
        ) != 0)
            .then_some(retry_after);
        let mut received = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let mut read: u32 = 0;
            if WinHttpReadData(
                request.0,
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                &mut read,
            ) == 0
            {
                return Err(failure(host));
            }
            if read == 0 {
                return Ok(Response {
                    status,
                    retry_after,
                    body: received,
                });
            }
            if received.len() + read as usize > limit {
                return Err(format!("{host} sent more than {limit} bytes"));
            }
            received.extend_from_slice(&chunk[..read as usize]);
        }
    }
}
