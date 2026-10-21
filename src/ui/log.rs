//! The debug log window: the messages the game server sends while it is open, listed by time,
//! size and method, with the selected one's data as indented JSON. The filter narrows the list
//! to messages whose method or data contains its text. Recording starts when the window opens
//! and stops when it closes (see [`debug`]); what was recorded stays until cleared.
//!
//! Standard Win32 controls, on the status window's thread and through its message loop. A
//! quarter-second timer adds the messages recorded since.

#![allow(clippy::upper_case_acronyms)]

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use crate::app::debug;
use crate::ui::paint::{self, HANDLE, RECT};
use crate::ui::window::{error_box, set_clipboard, state, wide};

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
struct POINT {
    x: i32,
    y: i32,
}

#[repr(C)]
struct MINMAXINFO {
    reserved: POINT,
    max_size: POINT,
    max_position: POINT,
    min_track_size: POINT,
    max_track_size: POINT,
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
    fn SendMessageW(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn SetWindowTextW(hwnd: HWND, text: *const u16) -> i32;
    fn GetWindowTextW(hwnd: HWND, text: *mut u16, max: i32) -> i32;
    fn GetWindowTextLengthW(hwnd: HWND) -> i32;
    fn MoveWindow(hwnd: HWND, x: i32, y: i32, width: i32, height: i32, repaint: i32) -> i32;
    fn SetTimer(hwnd: HWND, id: usize, ms: u32, func: *const c_void) -> usize;
    fn KillTimer(hwnd: HWND, id: usize) -> i32;
    fn DestroyWindow(hwnd: HWND) -> i32;
    fn ShowWindow(hwnd: HWND, cmd: i32) -> i32;
    fn SetForegroundWindow(hwnd: HWND) -> i32;
    fn EnableWindow(hwnd: HWND, enable: i32) -> i32;
    fn InvalidateRect(hwnd: HWND, rect: *const RECT, erase: i32) -> i32;
    fn GetClientRect(hwnd: HWND, rect: *mut RECT) -> i32;
    fn LoadCursorW(instance: HANDLE, name: *const u16) -> HANDLE;
    fn GetDpiForSystem() -> u32;
    fn GetSystemMetrics(index: i32) -> i32;
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
    fn DeleteObject(object: HANDLE) -> i32;
    fn SetBkColor(hdc: HANDLE, color: u32) -> u32;
    fn GetStockObject(object: i32) -> HANDLE;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> HANDLE;
}

const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
const WS_VSCROLL: u32 = 0x0020_0000;
const WS_HSCROLL: u32 = 0x0010_0000;
const WS_EX_CLIENTEDGE: u32 = 0x0200;
const ES_MULTILINE: u32 = 0x0004;
const ES_AUTOVSCROLL: u32 = 0x0040;
const ES_AUTOHSCROLL: u32 = 0x0080;
const ES_NOHIDESEL: u32 = 0x0100;
const ES_READONLY: u32 = 0x0800;
const LBS_NOTIFY: u32 = 0x0001;
const LBS_NOINTEGRALHEIGHT: u32 = 0x0100;
const SS_CENTERIMAGE: u32 = 0x0200;
const SS_ENDELLIPSIS: u32 = 0x4000;

const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_SETREDRAW: u32 = 0x000B;
const WM_CLOSE: u32 = 0x0010;
const WM_GETMINMAXINFO: u32 = 0x0024;
const WM_SETFONT: u32 = 0x0030;
const WM_SETICON: u32 = 0x0080;
const WM_COMMAND: u32 = 0x0111;
const WM_TIMER: u32 = 0x0113;
const WM_CTLCOLORSTATIC: u32 = 0x0138;
const EM_SETLIMITTEXT: u32 = 0x00C5;
const LB_ADDSTRING: u32 = 0x0180;
const LB_DELETESTRING: u32 = 0x0182;
const LB_RESETCONTENT: u32 = 0x0184;
const LB_SETCURSEL: u32 = 0x0186;
const LB_GETCURSEL: u32 = 0x0188;
const LB_GETCOUNT: u32 = 0x018B;
const LB_GETTOPINDEX: u32 = 0x018E;
const LB_SETTOPINDEX: u32 = 0x0197;
const LB_GETITEMHEIGHT: u32 = 0x01A1;
const LBN_SELCHANGE: usize = 1;
const EN_CHANGE: usize = 0x0300;
const BN_CLICKED: usize = 0;

const SW_SHOW: i32 = 5;
const SW_RESTORE: i32 = 9;
const SM_CXICON: i32 = 11;
const SM_CXSMICON: i32 = 49;
const CW_USEDEFAULT: i32 = 0x8000_0000_u32 as i32;
const IDC_ARROW: usize = 32512;
/// The dialog background, as a class brush (`COLOR_BTNFACE + 1`).
const BTNFACE_BRUSH: usize = 16;
const WHITE_BRUSH: i32 = 0;
const WHITE: u32 = 0x00FF_FFFF;

const ID_FILTER: usize = 301;
const ID_PAUSE: usize = 302;
const ID_CLEAR: usize = 303;
const ID_COPY: usize = 304;
const ID_LIST: usize = 305;
const TIMER_ID: usize = 1;
const TIMER_MS: u32 = 250;
/// How long "Copied" shows in place of the count.
const NOTE_TIME: Duration = Duration::from_secs(2);

const INTRO: &str = "Every message the game server sends is recorded while this window is open, \
                     except its ping replies. Pick one on the left to see its data.\r\n\r\n\
                     The filter shows messages whose method or data contains its text.";

/// The window's controls.
#[derive(Clone, Copy)]
struct Controls {
    window: HWND,
    label: HWND,
    filter: HWND,
    pause: HWND,
    clear: HWND,
    copy: HWND,
    status: HWND,
    list: HWND,
    detail: HWND,
}

/// Window-thread data.
struct LogUi {
    controls: Controls,
    scale: f32,
    /// Segoe UI for the controls, Consolas for the list and the data.
    fonts: [HANDLE; 2],
    /// The entry ids of the list's lines, in order.
    shown: VecDeque<u64>,
    /// The newest entry id already looked at.
    last_id: u64,
    filter: String,
    /// The entry in the detail box.
    selected: Option<u64>,
    status: String,
    note_until: Option<Instant>,
}

thread_local! {
    static LOG: RefCell<Option<LogUi>> = const { RefCell::new(None) };
    static REGISTERED: Cell<bool> = const { Cell::new(false) };
}

fn controls() -> Option<Controls> {
    LOG.with(|l| l.borrow().as_ref().map(|u| u.controls))
}

/// Runs `f` on the window data, if the window is open. Windows must not be called inside: it
/// can call the window procedure straight back, which reads the data.
fn with_ui<T>(f: impl FnOnce(&mut LogUi) -> T) -> Option<T> {
    LOG.with(|l| l.borrow_mut().as_mut().map(f))
}

/// Opens the log window, or brings it to the front if it is open.
pub fn open() {
    if let Some(c) = controls() {
        // SAFETY: the window belongs to this thread.
        unsafe {
            ShowWindow(c.window, SW_RESTORE);
            SetForegroundWindow(c.window);
        }
        return;
    }
    if let Err(err) = create() {
        error_box(&format!("The debug log can't be opened: {err}"));
    }
}

fn create() -> Result<(), String> {
    // SAFETY: plain Win32 calls with valid, NUL-terminated strings; every handle used comes
    // from the call that created it, on this thread.
    unsafe {
        let scale = GetDpiForSystem() as f32 / 96.0;
        let px = |v: f32| (v * scale).round() as i32;
        let instance = GetModuleHandleW(std::ptr::null());
        let class_name = wide("LastWarHQScannerDebugLog");
        let (icon, small_icon) = (
            paint::app_icon(GetSystemMetrics(SM_CXICON)),
            paint::app_icon(GetSystemMetrics(SM_CXSMICON)),
        );
        if !REGISTERED.get() {
            let class = WNDCLASSEXW {
                size: size_of::<WNDCLASSEXW>() as u32,
                style: 0,
                wnd_proc: window_proc,
                cls_extra: 0,
                wnd_extra: 0,
                instance,
                icon,
                cursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16),
                background: BTNFACE_BRUSH as HANDLE,
                menu_name: std::ptr::null(),
                class_name: class_name.as_ptr(),
                icon_small: small_icon,
            };
            if RegisterClassExW(&class) == 0 {
                return Err("cannot register the window class".into());
            }
            REGISTERED.set(true);
        }
        let title = wide("LastWarHQ Scanner · Debug log");
        let window = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            px(1040.0),
            px(680.0),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null_mut(),
        );
        if window.is_null() {
            return Err("cannot create the window".into());
        }
        SendMessageW(window, WM_SETICON, 1, icon as isize);
        SendMessageW(window, WM_SETICON, 0, small_icon as isize);

        let child = |ex: u32, class: &str, text: &str, style: u32, id: usize| {
            let (class, text) = (wide(class), wide(text));
            CreateWindowExW(
                ex,
                class.as_ptr(),
                text.as_ptr(),
                WS_CHILD | WS_VISIBLE | style,
                0,
                0,
                0,
                0,
                window,
                id as HANDLE,
                instance,
                std::ptr::null_mut(),
            )
        };
        let controls = Controls {
            window,
            label: child(0, "STATIC", "Filter", SS_CENTERIMAGE, 0),
            filter: child(
                WS_EX_CLIENTEDGE,
                "EDIT",
                "",
                WS_TABSTOP | ES_AUTOHSCROLL,
                ID_FILTER,
            ),
            pause: child(0, "BUTTON", "Pause", WS_TABSTOP, ID_PAUSE),
            clear: child(0, "BUTTON", "Clear", WS_TABSTOP, ID_CLEAR),
            copy: child(0, "BUTTON", "Copy", WS_TABSTOP, ID_COPY),
            status: child(0, "STATIC", "", SS_CENTERIMAGE | SS_ENDELLIPSIS, 0),
            list: child(
                WS_EX_CLIENTEDGE,
                "LISTBOX",
                "",
                WS_TABSTOP | WS_VSCROLL | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT,
                ID_LIST,
            ),
            detail: child(
                WS_EX_CLIENTEDGE,
                "EDIT",
                "",
                WS_TABSTOP
                    | WS_VSCROLL
                    | WS_HSCROLL
                    | ES_MULTILINE
                    | ES_AUTOVSCROLL
                    | ES_AUTOHSCROLL
                    | ES_NOHIDESEL
                    | ES_READONLY,
                0,
            ),
        };
        let fonts = [font("Segoe UI", px(15.0)), font("Consolas", px(14.0))];
        let c = controls;
        for control in [c.label, c.filter, c.pause, c.clear, c.copy, c.status] {
            SendMessageW(control, WM_SETFONT, fonts[0] as usize, 0);
        }
        for control in [c.list, c.detail] {
            SendMessageW(control, WM_SETFONT, fonts[1] as usize, 0);
        }
        // The most a multiline edit control takes, rather than its default of 32K characters.
        SendMessageW(c.detail, EM_SETLIMITTEXT, 0, 0);
        set_text(c.detail, INTRO);
        EnableWindow(c.copy, 0);

        LOG.with(|l| {
            *l.borrow_mut() = Some(LogUi {
                controls,
                scale,
                fonts,
                shown: VecDeque::new(),
                last_id: 0,
                filter: String::new(),
                selected: None,
                status: String::new(),
                note_until: None,
            })
        });
        debug::set_recording(true);
        layout();
        poll();
        SetTimer(window, TIMER_ID, TIMER_MS, std::ptr::null());
        ShowWindow(window, SW_SHOW);
    }
    Ok(())
}

fn font(face: &str, height: i32) -> HANDLE {
    let face = wide(face);
    // SAFETY: `face` is NUL-terminated; the other arguments are plain values.
    unsafe { CreateFontW(-height, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr()) }
}

fn set_text(control: HWND, text: &str) {
    let text = wide(text);
    // SAFETY: `text` is NUL-terminated and outlives the call.
    unsafe { SetWindowTextW(control, text.as_ptr()) };
}

fn get_text(control: HWND) -> String {
    // SAFETY: the buffer holds the text and its NUL; the length written is within it.
    unsafe {
        let len = GetWindowTextLengthW(control).max(0) as usize;
        let mut buf = vec![0u16; len + 1];
        let n = GetWindowTextW(control, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
        String::from_utf16_lossy(&buf[..n.min(len)])
    }
}

/// Places the controls for the window's size: the bar along the top, the list on the left
/// and the data on the right.
fn layout() {
    let Some((c, scale)) = with_ui(|u| (u.controls, u.scale)) else {
        return;
    };
    let px = |v: f32| (v * scale).round() as i32;
    let mut rect = RECT::default();
    // SAFETY: `rect` is a valid out-parameter; every control belongs to this thread.
    unsafe {
        GetClientRect(c.window, &mut rect);
        let (w, h) = (rect.right, rect.bottom);
        let (m, bar) = (px(10.0), px(28.0));
        let mut x = m;
        let mut place = |control: HWND, width: i32, gap: f32| {
            MoveWindow(control, x, m, width, bar, 1);
            x += width + px(gap);
        };
        place(c.label, px(36.0), 4.0);
        place(c.filter, px(280.0), 10.0);
        place(c.pause, px(80.0), 6.0);
        place(c.clear, px(80.0), 6.0);
        place(c.copy, px(80.0), 14.0);
        MoveWindow(c.status, x, m, (w - m - x).max(0), bar, 1);
        let top = m + bar + px(8.0);
        let height = (h - top - m).max(0);
        let list_w = ((w - 3 * m) as f32 * 0.4) as i32;
        MoveWindow(c.list, m, top, list_w.max(0), height, 1);
        let detail_x = 2 * m + list_w;
        MoveWindow(
            c.detail,
            detail_x,
            top,
            (w - detail_x - m).max(0),
            height,
            1,
        );
    }
}

/// Whether the list shows its last line, so new lines should scroll into view.
fn at_bottom(list: HWND) -> bool {
    let mut rect = RECT::default();
    // SAFETY: plain queries on a control of this thread.
    unsafe {
        let count = SendMessageW(list, LB_GETCOUNT, 0, 0);
        let top = SendMessageW(list, LB_GETTOPINDEX, 0, 0);
        let item = SendMessageW(list, LB_GETITEMHEIGHT, 0, 0).max(1);
        GetClientRect(list, &mut rect);
        top + (rect.bottom as isize / item) >= count
    }
}

/// Adds the entries recorded since the last look, drops the lines of entries the log no
/// longer keeps, and updates the count.
fn poll() {
    let Some((c, last_id, filter)) = with_ui(|u| (u.controls, u.last_id, u.filter.clone())) else {
        return;
    };
    let (oldest, (lines, newest), count) = {
        let s = state();
        (
            s.debug.oldest(),
            s.debug.lines(last_id, &filter),
            s.debug.len(),
        )
    };
    let Some((dropped, shown)) = with_ui(|u| {
        let mut dropped = 0;
        while u
            .shown
            .front()
            .is_some_and(|id| oldest.is_none_or(|o| *id < o))
        {
            u.shown.pop_front();
            dropped += 1;
        }
        u.shown.extend(lines.iter().map(|l| l.id));
        u.last_id = newest;
        (dropped, u.shown.len())
    }) else {
        return;
    };
    if dropped > 0 || !lines.is_empty() {
        let follow = at_bottom(c.list);
        // SAFETY: the list belongs to this thread; each string outlives its call.
        unsafe {
            SendMessageW(c.list, WM_SETREDRAW, 0, 0);
            for _ in 0..dropped {
                SendMessageW(c.list, LB_DELETESTRING, 0, 0);
            }
            for line in &lines {
                let label = wide(&line.label);
                SendMessageW(c.list, LB_ADDSTRING, 0, label.as_ptr() as isize);
            }
            if follow && shown > 0 {
                SendMessageW(c.list, LB_SETTOPINDEX, shown - 1, 0);
            }
            SendMessageW(c.list, WM_SETREDRAW, 1, 0);
            InvalidateRect(c.list, std::ptr::null(), 1);
        }
    }
    let state = if debug::recording() {
        "recording"
    } else {
        "paused"
    };
    let status = if filter.is_empty() {
        format!("{count} messages · {state}")
    } else {
        format!("{shown} of {count} messages match · {state}")
    };
    show_status(status, false);
}

/// Shows `text` beside the buttons; a note stays for a moment before the count comes back.
fn show_status(text: String, note: bool) {
    let changed = with_ui(|u| {
        let now = Instant::now();
        if !note && u.note_until.is_some_and(|until| now < until) {
            return None;
        }
        u.note_until = note.then(|| now + NOTE_TIME);
        (u.status != text).then(|| {
            u.status = text.clone();
            u.controls.status
        })
    })
    .flatten();
    if let Some(status) = changed {
        set_text(status, &text);
    }
}

/// Lists only the entries matching the filter's new text, keeping the selection if it still
/// matches.
fn filter_changed() {
    let Some(c) = controls() else {
        return;
    };
    let filter = get_text(c.filter);
    with_ui(|u| {
        u.filter = filter;
        u.shown.clear();
        u.last_id = 0;
    });
    // SAFETY: the list belongs to this thread.
    unsafe { SendMessageW(c.list, LB_RESETCONTENT, 0, 0) };
    poll();
    let index = with_ui(|u| {
        u.selected
            .and_then(|id| u.shown.iter().position(|shown| *shown == id))
    })
    .flatten();
    if let Some(index) = index {
        // SAFETY: as above.
        unsafe {
            SendMessageW(c.list, LB_SETCURSEL, index, 0);
            SendMessageW(c.list, LB_SETTOPINDEX, index, 0);
        }
    }
}

/// Shows the selected entry's data.
fn selection_changed() {
    let Some(c) = controls() else {
        return;
    };
    // SAFETY: a plain query on the list.
    let index = unsafe { SendMessageW(c.list, LB_GETCURSEL, 0, 0) };
    let Some(id) = with_ui(|u| {
        usize::try_from(index)
            .ok()
            .and_then(|i| u.shown.get(i).copied())
    })
    .flatten() else {
        return;
    };
    with_ui(|u| u.selected = Some(id));
    let detail = state().debug.detail(id);
    let text = detail.map_or_else(
        || "This message is no longer in the log.".to_string(),
        |d| d.replace('\n', "\r\n"),
    );
    set_text(c.detail, &text);
    // SAFETY: the button belongs to this thread.
    unsafe { EnableWindow(c.copy, 1) };
}

fn pause_clicked() {
    let Some(c) = controls() else {
        return;
    };
    let recording = !debug::recording();
    debug::set_recording(recording);
    set_text(c.pause, if recording { "Pause" } else { "Resume" });
    poll();
}

fn clear_clicked() {
    let Some(c) = controls() else {
        return;
    };
    state().debug.clear();
    with_ui(|u| {
        u.shown.clear();
        u.selected = None;
    });
    // SAFETY: the controls belong to this thread.
    unsafe {
        SendMessageW(c.list, LB_RESETCONTENT, 0, 0);
        EnableWindow(c.copy, 0);
    }
    set_text(c.detail, INTRO);
    poll();
}

/// Copies the selected entry in full: method, time, size and data.
fn copy_clicked() {
    let Some(id) = with_ui(|u| u.selected).flatten() else {
        return;
    };
    let Some(detail) = state().debug.detail(id) else {
        return;
    };
    let note = match set_clipboard(&detail.replace('\n', "\r\n")) {
        Ok(()) => "Copied to the clipboard".to_string(),
        Err(err) => format!("Copy failed: {err}"),
    };
    show_status(note, true);
}

/// Closing stops the recording; what was recorded stays for the next time the window opens.
fn destroyed() {
    debug::set_recording(false);
    if let Some(ui) = LOG.with(|l| l.borrow_mut().take()) {
        // SAFETY: the timer and fonts were made for this window, which is going; the controls
        // using the fonts go with it.
        unsafe {
            KillTimer(ui.controls.window, TIMER_ID);
            for font in ui.fonts {
                DeleteObject(font);
            }
        }
    }
}

/// The window procedure. A panic must not unwind into Windows (that aborts the process), so
/// one is caught, reported, and the log window closed.
unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        handle_message(hwnd, msg, wparam, lparam)
    })) {
        Ok(result) => result,
        Err(panic) => {
            let what = crate::app::panic_text(panic.as_ref());
            error_box(&format!(
                "The debug log hit an internal error and will close:\n{what}"
            ));
            // SAFETY: `hwnd` is this window.
            unsafe { DestroyWindow(hwnd) };
            0
        }
    }
}

fn handle_message(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_TIMER => poll(),
        WM_SIZE => layout(),
        WM_GETMINMAXINFO => {
            // SAFETY: plain query; for WM_GETMINMAXINFO, `lparam` points at a MINMAXINFO
            // valid for the call.
            unsafe {
                let scale = GetDpiForSystem() as f32 / 96.0;
                let info = &mut *(lparam as *mut MINMAXINFO);
                info.min_track_size = POINT {
                    x: (720.0 * scale) as i32,
                    y: (400.0 * scale) as i32,
                };
            }
            return 0;
        }
        // The read-only data box on white, like the list, rather than the dialog grey.
        WM_CTLCOLORSTATIC if controls().is_some_and(|c| c.detail == lparam as HWND) => {
            // SAFETY: Windows passes the control's DC in `wparam` for this message; the brush
            // is a stock object, never deleted.
            unsafe {
                SetBkColor(wparam as HANDLE, WHITE);
                return GetStockObject(WHITE_BRUSH) as isize;
            }
        }
        WM_COMMAND => {
            let (id, code) = (wparam & 0xffff, (wparam >> 16) & 0xffff);
            match (id, code) {
                (ID_FILTER, EN_CHANGE) => filter_changed(),
                (ID_LIST, LBN_SELCHANGE) => selection_changed(),
                (ID_PAUSE, BN_CLICKED) => pause_clicked(),
                (ID_CLEAR, BN_CLICKED) => clear_clicked(),
                (ID_COPY, BN_CLICKED) => copy_clicked(),
                _ => {}
            }
        }
        WM_CLOSE => {
            // SAFETY: `hwnd` is this window.
            unsafe { DestroyWindow(hwnd) };
            return 0;
        }
        WM_DESTROY => destroyed(),
        _ => {}
    }
    // SAFETY: default handling for every message, with the arguments Windows passed.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
