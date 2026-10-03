//! The browser half of signing in: open the site's connect page, then wait on `127.0.0.1` for
//! the browser to come back to `/callback` with a one-time code (RFC 8252 loopback redirect).

use std::ffi::c_void;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use crate::auth::SITE_HOST;

#[link(name = "shell32")]
unsafe extern "system" {
    fn ShellExecuteW(
        hwnd: *mut c_void,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show: i32,
    ) -> *mut c_void;
}

const SW_SHOWNORMAL: i32 = 1;

/// A request line and headers longer than this are not a callback.
const MAX_REQUEST: usize = 8 * 1024;

/// `https://<site>/scanner/connect?...`: the page where the user approves this PC.
pub fn connect_url(port: u16, state: &str, challenge: &str, name: &str) -> String {
    format!(
        "https://{SITE_HOST}/scanner/connect?port={port}&state={}&challenge={}&name={}",
        url_encode(state),
        url_encode(challenge),
        url_encode(name)
    )
}

/// The PC's name, as the user will see it when approving and in their list of PCs: 1–64
/// characters, no control characters.
pub fn device_name() -> String {
    clean_device_name(&std::env::var("COMPUTERNAME").unwrap_or_default())
}

fn clean_device_name(raw: &str) -> String {
    let name: String = raw.chars().filter(|c| !c.is_control()).take(64).collect();
    let name = name.trim();
    if name.is_empty() {
        "Windows PC".to_string()
    } else {
        name.to_string()
    }
}

/// Percent-encodes everything but the unreserved characters `A-Z a-z 0-9 - . _ ~`.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Opens `url` in the user's default browser.
pub fn open_browser(url: &str) -> Result<(), String> {
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let (open, url) = (wide("open"), wide(url));
    // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call; the others are
    // optional and null.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            open.as_ptr(),
            url.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success.
    if result as usize <= 32 {
        return Err(format!(
            "cannot open the browser (error {})",
            result as usize
        ));
    }
    Ok(())
}

/// The browser's callback: its code, and the connection to answer once the code has been
/// swapped for a token and stored, so the browser only says "connected" when that is true.
pub struct Callback {
    pub code: String,
    stream: TcpStream,
}

impl Callback {
    /// Tells the browser how signing in ended: the username, or why it didn't finish.
    pub fn answer(mut self, outcome: Result<&str, &str>) {
        respond(&mut self.stream, "200 OK", &result_page(outcome));
    }
}

/// Waits up to `timeout` for the browser's `GET /callback` carrying `state`, and returns it.
/// Other requests (a favicon, a wrong or missing state) are answered "not found" and
/// otherwise ignored. The timeout covers everything: waiting for connections and reading
/// them, however many there are and however slowly they arrive.
pub fn wait_for_callback(
    listener: &TcpListener,
    state: &str,
    timeout: Duration,
) -> Result<Callback, String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("the sign-in listener failed: {e}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            return Err("timed out waiting for the browser".into());
        }
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(callback) = serve(stream, state, deadline) {
                    return Ok(callback);
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("the sign-in listener failed: {e}")),
        }
    }
}

/// Reads one connection until `deadline`. The callback is returned unanswered; anything else
/// is answered "not found".
fn serve(mut stream: TcpStream, state: &str, deadline: Instant) -> Option<Callback> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let code = read_head(&mut stream, deadline).and_then(|head| callback_code(&head, state));
    match code {
        Some(code) => Some(Callback { code, stream }),
        None => {
            respond(&mut stream, "404 Not Found", "Not found");
            None
        }
    }
}

fn respond(stream: &mut TcpStream, status: &str, page: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );
    let _ = stream.flush();
}

/// The page that ends a sign-in in the browser.
fn result_page(outcome: Result<&str, &str>) -> String {
    let (title, text) = match outcome {
        Ok(user) => (
            "Connected".to_string(),
            format!(
                "LastWarHQ Scanner is signed in as {}. You can close this tab.",
                html_escape(user)
            ),
        ),
        Err(why) => (
            "Sign-in didn't finish".to_string(),
            format!(
                "{}. Go back to LastWarHQ Scanner and try again.",
                html_escape(why)
            ),
        ),
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>LastWarHQ Scanner</title></head><body style=\"font-family:Segoe UI,sans-serif;margin:3em\"><h1>{title}</h1><p>{text}</p></body></html>"
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The request line and headers, up to the blank line, if they all arrive by `deadline`.
fn read_head(stream: &mut TcpStream, deadline: Instant) -> Option<String> {
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let left = deadline.checked_duration_since(Instant::now())?;
        if left.is_zero() {
            return None;
        }
        stream.set_read_timeout(Some(left)).ok()?;
        let n = stream.read(&mut buf).ok()?;
        if n == 0 || head.len() + n > MAX_REQUEST {
            return None;
        }
        head.extend_from_slice(&buf[..n]);
    }
    String::from_utf8(head).ok()
}

/// The code from a request for `GET /callback?code=...&state=...` whose state is exactly
/// `state`.
fn callback_code(head: &str, state: &str) -> Option<String> {
    let mut parts = head.lines().next()?.split(' ');
    let (method, target) = (parts.next()?, parts.next()?);
    let query = target.strip_prefix("/callback?")?;
    if method != "GET" {
        return None;
    }
    let mut code = None;
    let mut matched = false;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        match key {
            "code" => code = Some(percent_decode(value)?),
            "state" => matched = percent_decode(value)? == state,
            _ => {}
        }
    }
    code.filter(|c| matched && !c.is_empty())
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn connect_url_encodes_its_parts() {
        assert_eq!(
            connect_url(51234, "s-_x", "c~.9", "Example PC #1"),
            "https://lastwarhq.dev/scanner/connect?port=51234&state=s-_x&challenge=c~.9&name=Example%20PC%20%231"
        );
        assert_eq!(url_encode("é"), "%C3%A9");
    }

    #[test]
    fn device_names_are_cleaned() {
        assert_eq!(clean_device_name("EXAMPLE-PC"), "EXAMPLE-PC");
        assert_eq!(clean_device_name(" a\u{7}b\n "), "ab");
        assert_eq!(clean_device_name(""), "Windows PC");
        assert_eq!(clean_device_name(&"x".repeat(100)).len(), 64);
    }

    #[test]
    fn only_the_callback_with_our_state_gives_a_code() {
        let req = |line: &str| format!("{line}\r\nHost: 127.0.0.1\r\n\r\n");
        let code = |line: &str| callback_code(&req(line), "st4te");
        assert_eq!(
            code("GET /callback?code=abc%2Fd&state=st4te HTTP/1.1"),
            Some("abc/d".into())
        );
        assert_eq!(
            code("GET /callback?state=st4te&code=abc HTTP/1.1"),
            Some("abc".into())
        );
        for refused in [
            "GET /callback?code=abc&state=other HTTP/1.1",
            "GET /callback?code=abc&state=st4tex HTTP/1.1",
            "GET /callback?code=abc HTTP/1.1",
            "GET /callback?code=&state=st4te HTTP/1.1",
            "POST /callback?code=abc&state=st4te HTTP/1.1",
            "GET /favicon.ico HTTP/1.1",
            "GET /other?code=abc&state=st4te HTTP/1.1",
            "GET /callback?code=%zz&state=st4te HTTP/1.1",
        ] {
            assert_eq!(code(refused), None, "{refused}");
        }
    }

    /// Sends `line` to the listener at `port` and returns the whole answer.
    fn send(port: u16, line: &str) -> String {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        write!(stream, "{line}\r\nHost: 127.0.0.1:{port}\r\n\r\n").unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer
    }

    fn status_line(answer: &str) -> &str {
        answer.lines().next().unwrap_or_default()
    }

    #[test]
    fn the_listener_waits_through_other_requests_for_the_callback() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let browser = thread::spawn(move || {
            [
                send(port, "GET /favicon.ico HTTP/1.1"),
                send(port, "GET /callback?code=abc&state=wrong HTTP/1.1"),
                send(port, "GET /callback?code=abc&state=st4te HTTP/1.1"),
            ]
        });
        let callback = wait_for_callback(&listener, "st4te", Duration::from_secs(10)).unwrap();
        assert_eq!(callback.code, "abc");
        // The browser hears nothing until the sign-in has finished.
        callback.answer(Ok("example"));
        let answers = browser.join().unwrap();
        let statuses: Vec<&str> = answers.iter().map(|a| status_line(a)).collect();
        assert_eq!(
            statuses,
            [
                "HTTP/1.1 404 Not Found",
                "HTTP/1.1 404 Not Found",
                "HTTP/1.1 200 OK"
            ]
        );
        assert!(
            answers[2].contains("signed in as example"),
            "{}",
            answers[2]
        );
    }

    #[test]
    fn the_browser_hears_when_signing_in_fails_after_the_callback() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let browser =
            thread::spawn(move || send(port, "GET /callback?code=abc&state=st4te HTTP/1.1"));
        let callback = wait_for_callback(&listener, "st4te", Duration::from_secs(10)).unwrap();
        callback.answer(Err("the sign-in expired or was refused"));
        let answer = browser.join().unwrap();
        assert!(answer.contains("Sign-in didn't finish"), "{answer}");
        assert!(!answer.contains("Connected"), "{answer}");
    }

    #[test]
    fn failure_pages_never_claim_success() {
        for why in [
            "the sign-in expired or was refused",
            "lastwarhq.dev: no connection",
            "signed in, but the token couldn't be saved: Credential Manager refused it (error 5)",
        ] {
            let page = result_page(Err(why));
            assert!(page.contains("Sign-in didn't finish"), "{page}");
            assert!(
                !page.contains("Connected") && !page.contains("signed in as"),
                "{page}"
            );
        }
        assert!(result_page(Ok("<b>")).contains("signed in as &lt;b&gt;"));
    }

    #[test]
    fn the_listener_gives_up_after_the_timeout() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let result = wait_for_callback(&listener, "st4te", Duration::from_millis(200));
        assert_eq!(
            result.err(),
            Some("timed out waiting for the browser".into())
        );
    }

    #[test]
    fn a_slow_callback_cannot_outlast_the_timeout() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let browser = thread::spawn(move || {
            let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            // Half the request, then the rest well after the deadline.
            let _ = write!(stream, "GET /callback?code=abc&state=st4te HTTP/1.1\r\n");
            thread::sleep(Duration::from_millis(1500));
            let _ = write!(stream, "Host: 127.0.0.1\r\n\r\n");
        });
        let started = Instant::now();
        let result = wait_for_callback(&listener, "st4te", Duration::from_millis(300));
        assert_eq!(
            result.err(),
            Some("timed out waiting for the browser".into())
        );
        assert!(
            started.elapsed() < Duration::from_millis(1200),
            "gave up after {:?}",
            started.elapsed()
        );
        browser.join().unwrap();
    }
}
