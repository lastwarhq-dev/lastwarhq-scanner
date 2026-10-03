//! The status window: plain Win32 through hand-written declarations, no crates.
//!
//! One window with a label and a value line per status, and two buttons. A one-second timer
//! rereads the shared state, runs the weekly reset from the clock, and updates any text that
//! changed. All window work happens on the thread that calls [`run`].

#![allow(clippy::upper_case_acronyms)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::app::export::payload_json;
use crate::app::state::State;
use crate::ui::status::{Health, StatusLines, status_lines};
use crate::util::time::now;

type HANDLE = *mut c_void;
type HWND = HANDLE;
type WndProc = unsafe extern "system" fn(HWND, u32, usize, isize) -> isize;

#[repr(C)]
struct WNDCLASSEXW {
    size: u32,
    style: u32,
    wnd_proc: WndProc,
    cls_extra: i32,
    wnd_extra: i32,
    instance: HANDLE,
    icon: HANDLE,
    cursor: HANDLE,
    background: HANDLE,
    menu_name: *const u16,
    class_name: *const u16,
    icon_small: HANDLE,
}

#[repr(C)]
struct MSG {
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt_x: i32,
    pt_y: i32,
    private: u32,
}

#[repr(C)]
struct RECT {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassExW(class: *const WNDCLASSEXW) -> u16;
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: HWND,
        menu: HANDLE,
        instance: HANDLE,
        param: *mut c_void,
    ) -> HWND;
    fn DefWindowProcW(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn GetMessageW(msg: *mut MSG, hwnd: HWND, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const MSG) -> i32;
    fn DispatchMessageW(msg: *const MSG) -> isize;
    fn PostQuitMessage(code: i32);
    fn PostMessageW(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn SendMessageW(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn SetWindowTextW(hwnd: HWND, text: *const u16) -> i32;
    fn SetTimer(hwnd: HWND, id: usize, ms: u32, func: *const c_void) -> usize;
    fn ShowWindow(hwnd: HWND, cmd: i32) -> i32;
    fn UpdateWindow(hwnd: HWND) -> i32;
    fn EnableWindow(hwnd: HWND, enable: i32) -> i32;
    fn LoadCursorW(instance: HANDLE, name: *const u16) -> HANDLE;
    fn LoadIconW(instance: HANDLE, name: *const u16) -> HANDLE;
    fn GetSysColorBrush(index: i32) -> HANDLE;
    fn GetSysColor(index: i32) -> u32;
    fn AdjustWindowRect(rect: *mut RECT, style: u32, menu: i32) -> i32;
    fn SetProcessDPIAware() -> i32;
    fn GetDpiForSystem() -> u32;
    fn MessageBoxW(hwnd: HWND, text: *const u16, caption: *const u16, kind: u32) -> i32;
    fn OpenClipboard(owner: HWND) -> i32;
    fn EmptyClipboard() -> i32;
    fn SetClipboardData(format: u32, data: HANDLE) -> HANDLE;
    fn CloseClipboard() -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike_out: u32,
        charset: u32,
        out_precision: u32,
        clip_precision: u32,
        quality: u32,
        pitch_and_family: u32,
        face: *const u16,
    ) -> HANDLE;
    fn SetTextColor(hdc: HANDLE, color: u32) -> u32;
    fn SetBkColor(hdc: HANDLE, color: u32) -> u32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> HANDLE;
    fn GlobalAlloc(flags: u32, bytes: usize) -> HANDLE;
    fn GlobalLock(mem: HANDLE) -> *mut c_void;
    fn GlobalUnlock(mem: HANDLE) -> i32;
    fn GlobalFree(mem: HANDLE) -> HANDLE;
    fn CreateMutexW(attributes: *const c_void, owner: i32, name: *const u16) -> HANDLE;
    fn GetLastError() -> u32;
}

const WS_CAPTION: u32 = 0x00C0_0000;
const WS_SYSMENU: u32 = 0x0008_0000;
const WS_MINIMIZEBOX: u32 = 0x0002_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
const SS_NOPREFIX: u32 = 0x80;
const SS_ENDELLIPSIS: u32 = 0x4000;
const WINDOW_STYLE: u32 = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;

const WM_DESTROY: u32 = 0x0002;
const WM_SETFONT: u32 = 0x0030;
const WM_COMMAND: u32 = 0x0111;
const WM_TIMER: u32 = 0x0113;
const WM_CTLCOLORSTATIC: u32 = 0x0138;
/// Posted by the mail loader thread when it finishes.
const WM_MAIL_DONE: u32 = 0x8001;

const COLOR_WINDOW: i32 = 5;
const COLOR_WINDOWTEXT: i32 = 8;
const COLOR_GRAYTEXT: i32 = 17;
const CF_UNICODETEXT: u32 = 13;
const GMEM_MOVEABLE: u32 = 0x0002;
const ERROR_ALREADY_EXISTS: u32 = 183;
const MB_ICONERROR: u32 = 0x10;
const CW_USEDEFAULT: i32 = 0x8000_0000_u32 as i32;
const IDC_ARROW: usize = 32512;
const IDI_APPLICATION: usize = 32512;

const ID_LOAD_MAIL: usize = 200;
const ID_COPY_JSON: usize = 201;
const TIMER_ID: usize = 1;

/// Row labels, in display order; each gets a value line beside it.
const LABELS: [&str; 9] = [
    "Status",
    "Account",
    "Week",
    "Roster",
    "DS sign-ups",
    "DS results",
    "VS scores",
    "Activity",
    "",
];
const ROW_STATUS: usize = 0;
const ROW_NOTE: usize = 8;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn rgb(r: u8, g: u8, b: u8) -> u32 {
    u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16)
}

/// Shared with the window procedure, which has no other way to reach it.
static STATE: OnceLock<Arc<Mutex<State>>> = OnceLock::new();

/// Window-thread data.
struct Ui {
    window: HWND,
    values: Vec<HWND>,
    shown: Vec<String>,
    load_button: HWND,
    health: Health,
    note_until: Option<Instant>,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE
        .get()
        .expect("state set before the window opens")
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Shows an error in a message box, for failures before or outside the window.
pub fn error_box(text: &str) {
    let (text, caption) = (wide(text), wide("LastWarHQ Scanner"));
    // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_ICONERROR,
        )
    };
}

/// True if another copy of the tool is already running. The named mutex lives as long as the
/// process.
pub fn already_running() -> bool {
    let name = wide("Local\\lastwarhq-scanner-single-instance");
    // SAFETY: `name` is a NUL-terminated UTF-16 string; no security attributes.
    unsafe {
        let mutex = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        !mutex.is_null() && GetLastError() == ERROR_ALREADY_EXISTS
    }
}

/// Opens the window and runs its message loop until it is closed.
pub fn run(shared: Arc<Mutex<State>>) -> Result<(), String> {
    STATE.set(shared).map_err(|_| "window already running")?;
    // SAFETY: plain Win32 calls with valid, NUL-terminated strings and zeroed out-parameters;
    // every handle used comes from the call that created it.
    unsafe {
        SetProcessDPIAware();
        let scale = |v: i32| v * GetDpiForSystem() as i32 / 96;
        let instance = GetModuleHandleW(std::ptr::null());
        let class_name = wide("LastWarHQScannerWindow");
        let class = WNDCLASSEXW {
            size: size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            wnd_proc: window_proc,
            cls_extra: 0,
            wnd_extra: 0,
            instance,
            icon: LoadIconW(std::ptr::null_mut(), IDI_APPLICATION as *const u16),
            cursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16),
            background: GetSysColorBrush(COLOR_WINDOW),
            menu_name: std::ptr::null(),
            class_name: class_name.as_ptr(),
            icon_small: std::ptr::null_mut(),
        };
        if RegisterClassExW(&class) == 0 {
            return Err("cannot register the window class".into());
        }

        let (margin, label_w, value_w, row_h, button_w, button_h) = (
            scale(14),
            scale(92),
            scale(470),
            scale(22),
            scale(96),
            scale(28),
        );
        let rows = LABELS.len() as i32 - 1;
        let buttons_y = margin + rows * row_h + scale(10);
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: margin * 2 + label_w + value_w,
            bottom: buttons_y + button_h + margin,
        };
        AdjustWindowRect(&mut rect, WINDOW_STYLE, 0);
        let title = wide("LastWarHQ Scanner");
        let window = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WINDOW_STYLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rect.right - rect.left,
            rect.bottom - rect.top,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null_mut(),
        );
        if window.is_null() {
            return Err("cannot create the window".into());
        }

        let face = wide("Segoe UI");
        let font = |weight: i32| {
            CreateFontW(
                -scale(13),
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                1,
                0,
                0,
                5,
                0,
                face.as_ptr(),
            )
        };
        let (regular, bold) = (font(400), font(600));
        let control = |class: &str,
                       text: &str,
                       style: u32,
                       x: i32,
                       y: i32,
                       w: i32,
                       h: i32,
                       id: usize,
                       f: HANDLE| {
            let (class, text) = (wide(class), wide(text));
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                text.as_ptr(),
                WS_CHILD | WS_VISIBLE | style,
                x,
                y,
                w,
                h,
                window,
                id as HANDLE,
                instance,
                std::ptr::null_mut(),
            );
            SendMessageW(hwnd, WM_SETFONT, f as usize, 0);
            hwnd
        };

        let mut values = Vec::new();
        for (i, label) in LABELS.iter().enumerate() {
            let y = if i == ROW_NOTE {
                buttons_y + scale(5)
            } else {
                margin + i as i32 * row_h
            };
            let x = if i == ROW_NOTE {
                margin + 2 * (button_w + scale(8))
            } else {
                margin + label_w
            };
            if i != ROW_NOTE {
                control(
                    "STATIC",
                    label,
                    SS_NOPREFIX,
                    margin,
                    y,
                    label_w,
                    row_h,
                    100 + i,
                    bold,
                );
            }
            values.push(control(
                "STATIC",
                "",
                SS_NOPREFIX | SS_ENDELLIPSIS,
                x,
                y,
                value_w,
                row_h,
                120 + i,
                regular,
            ));
        }
        let load_button = control(
            "BUTTON",
            "Load mail",
            WS_TABSTOP,
            margin,
            buttons_y,
            button_w,
            button_h,
            ID_LOAD_MAIL,
            regular,
        );
        control(
            "BUTTON",
            "Copy JSON",
            WS_TABSTOP,
            margin + button_w + scale(8),
            buttons_y,
            button_w,
            button_h,
            ID_COPY_JSON,
            regular,
        );

        UI.with(|ui| {
            *ui.borrow_mut() = Some(Ui {
                window,
                shown: vec![String::new(); values.len()],
                values,
                load_button,
                health: Health::Waiting,
                note_until: None,
            })
        });
        refresh();
        SetTimer(window, TIMER_ID, 1000, std::ptr::null());
        ShowWindow(window, 5);
        UpdateWindow(window);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

/// Rereads the shared state and updates every line whose text changed.
fn refresh() {
    let now = now();
    let lines = {
        let mut s = state();
        s.tick(now);
        status_lines(&s, now)
    };
    let StatusLines {
        connection,
        health,
        account,
        week,
        roster,
        signups,
        results,
        vs,
        activity,
    } = lines;
    let mut texts: Vec<(usize, String)> = [
        format!("● {connection}"),
        account,
        week,
        roster,
        signups,
        results,
        vs,
        activity,
    ]
    .into_iter()
    .enumerate()
    .collect();
    let note_expired = UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        let ui = ui.as_mut()?;
        ui.health = health;
        let expired = ui.note_until.is_some_and(|t| Instant::now() >= t);
        if expired {
            ui.note_until = None;
        }
        Some(expired)
    });
    if note_expired == Some(true) {
        texts.push((ROW_NOTE, String::new()));
    }
    set_texts(texts);
}

/// Sets each changed line's text. The window data is not borrowed while Windows is called,
/// because setting a text makes Windows call the window procedure straight back
/// (WM_CTLCOLORSTATIC), which reads it.
fn set_texts(texts: Vec<(usize, String)>) {
    let changed: Vec<(HWND, String)> = UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        let Some(ui) = ui.as_mut() else {
            return Vec::new();
        };
        let mut changed = Vec::new();
        for (row, text) in texts {
            if ui.shown[row] != text {
                ui.shown[row] = text.clone();
                changed.push((ui.values[row], text));
            }
        }
        changed
    });
    for (control, text) in changed {
        let w = wide(&text);
        // SAFETY: the control handle is alive for the window's lifetime; `w` is NUL-terminated.
        unsafe { SetWindowTextW(control, w.as_ptr()) };
    }
}

/// Shows a short message beside the buttons for a few seconds.
fn note(text: &str) {
    UI.with(|ui| {
        if let Some(ui) = ui.borrow_mut().as_mut() {
            ui.note_until = Some(Instant::now() + Duration::from_secs(6));
        }
    });
    set_texts(vec![(ROW_NOTE, text.to_string())]);
}

fn load_mail_clicked() {
    let Some((window, button)) = UI.with(|ui| {
        ui.borrow()
            .as_ref()
            .map(|u| (u.window as usize, u.load_button))
    }) else {
        return;
    };
    // SAFETY: the button belongs to this thread's window.
    unsafe { EnableWindow(button, 0) };
    note("Reading mail…");
    let shared = Arc::clone(STATE.get().expect("state set"));
    thread::spawn(move || {
        let message = crate::app::load_mail(&shared);
        let text = Box::into_raw(Box::new(message)) as isize;
        // SAFETY: the window outlives the loader thread (closing it ends the process); the
        // window procedure takes back ownership of the boxed message.
        unsafe { PostMessageW(window as HWND, WM_MAIL_DONE, 0, text) };
    });
}

fn copy_json_clicked() {
    let (payload, players) = {
        let mut s = state();
        (payload_json(&mut s, now()), s.view.players().count())
    };
    match set_clipboard(&payload) {
        Ok(()) => note(&format!("Copied {players} players")),
        Err(err) => note(&format!("Copy failed: {err}")),
    }
}

fn set_clipboard(text: &str) -> Result<(), &'static str> {
    let wide = wide(text);
    let window = UI
        .with(|ui| ui.borrow().as_ref().map(|u| u.window))
        .ok_or("no window")?;
    // SAFETY: the text is copied into a moveable global block sized for `wide` (NUL included),
    // through a pointer checked for null. Once SetClipboardData succeeds the system owns the
    // block; on every failure path it is freed here. The clipboard is always closed again.
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
        if mem.is_null() {
            return Err("out of memory");
        }
        let dest = GlobalLock(mem) as *mut u16;
        if dest.is_null() {
            GlobalFree(mem);
            return Err("cannot lock clipboard memory");
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), dest, wide.len());
        GlobalUnlock(mem);

        if OpenClipboard(window) == 0 {
            GlobalFree(mem);
            return Err("clipboard busy");
        }
        EmptyClipboard();
        let result = if SetClipboardData(CF_UNICODETEXT, mem).is_null() {
            GlobalFree(mem);
            Err("clipboard refused the data")
        } else {
            Ok(())
        };
        CloseClipboard();
        result
    }
}

/// The window procedure. A panic must not unwind into Windows (that aborts the process), so
/// one is caught, reported, and the window closed.
unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        handle_message(hwnd, msg, wparam, lparam)
    })) {
        Ok(result) => result,
        Err(panic) => {
            let what = crate::app::panic_text(panic.as_ref());
            error_box(&format!(
                "LastWarHQ Scanner hit an internal error and will close:\n{what}"
            ));
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(1) };
            0
        }
    }
}

fn handle_message(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_TIMER => refresh(),
        WM_COMMAND => match wparam & 0xffff {
            ID_LOAD_MAIL => load_mail_clicked(),
            ID_COPY_JSON => copy_json_clicked(),
            _ => {}
        },
        WM_MAIL_DONE => {
            // SAFETY: `lparam` is the Box<String> leaked by the loader thread for this message.
            let message = unsafe { *Box::from_raw(lparam as *mut String) };
            if let Some(button) = UI.with(|ui| ui.borrow().as_ref().map(|u| u.load_button)) {
                // SAFETY: the button belongs to this window.
                unsafe { EnableWindow(button, 1) };
            }
            note(&message);
            refresh();
        }
        WM_CTLCOLORSTATIC => {
            // Window-coloured background for every label; the status line takes its health colour.
            let hdc = wparam as HANDLE;
            let control = lparam as HWND;
            let color = UI.with(|ui| {
                let ui = ui.try_borrow().ok()?;
                let ui = ui.as_ref()?;
                if control == ui.values[ROW_STATUS] {
                    Some(match ui.health {
                        Health::Good => rgb(0x1f, 0x8a, 0x4c),
                        Health::Waiting => rgb(0xb7, 0x79, 0x1f),
                        Health::Failed => rgb(0xc2, 0x37, 0x2b),
                    })
                } else if control == ui.values[ROW_NOTE] {
                    // SAFETY: GetSysColor has no preconditions.
                    Some(unsafe { GetSysColor(COLOR_GRAYTEXT) })
                } else {
                    None
                }
            });
            // SAFETY: `hdc` is the device context Windows passed for this paint.
            unsafe {
                SetTextColor(hdc, color.unwrap_or_else(|| GetSysColor(COLOR_WINDOWTEXT)));
                SetBkColor(hdc, GetSysColor(COLOR_WINDOW));
                return GetSysColorBrush(COLOR_WINDOW) as isize;
            }
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
        }
        _ => {}
    }
    // SAFETY: default handling for every message, with the arguments Windows passed.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
