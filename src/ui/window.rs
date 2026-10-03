//! The status window: plain Win32 through hand-written declarations, no crates.
//!
//! One window, painted by [`paint`], with two child controls: the update and Copy JSON
//! buttons. A one-second timer rereads the shared state, runs the weekly reset from the clock,
//! and repaints if anything changed. All window work happens on the thread that calls [`run`].

#![allow(clippy::upper_case_acronyms)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::app::export::payload_json;
use crate::app::state::{Install, State};
use crate::ui::paint::{self, ButtonState, HANDLE, Painter, RECT};
use crate::ui::status::{Mark, Screen, screen};
use crate::update::{Version, install};
use crate::util::time::now;

type HWND = HANDLE;
type WndProc = unsafe extern "system" fn(HWND, u32, usize, isize) -> isize;

#[repr(C)]
struct POINT {
    x: i32,
    y: i32,
}

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
struct PAINTSTRUCT {
    hdc: HANDLE,
    erase: i32,
    paint: RECT,
    restore: i32,
    inc_update: i32,
    reserved: [u8; 32],
}

#[repr(C)]
struct DRAWITEMSTRUCT {
    ctl_type: u32,
    ctl_id: u32,
    item_id: u32,
    item_action: u32,
    item_state: u32,
    hwnd_item: HWND,
    hdc: HANDLE,
    rect: RECT,
    item_data: usize,
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
    fn SetTimer(hwnd: HWND, id: usize, ms: u32, func: *const c_void) -> usize;
    fn ShowWindow(hwnd: HWND, cmd: i32) -> i32;
    fn UpdateWindow(hwnd: HWND) -> i32;
    fn EnableWindow(hwnd: HWND, enable: i32) -> i32;
    fn InvalidateRect(hwnd: HWND, rect: *const RECT, erase: i32) -> i32;
    fn BeginPaint(hwnd: HWND, paint: *mut PAINTSTRUCT) -> HANDLE;
    fn EndPaint(hwnd: HWND, paint: *const PAINTSTRUCT) -> i32;
    fn LoadCursorW(instance: HANDLE, name: *const u16) -> HANDLE;
    fn SetCursor(cursor: HANDLE) -> HANDLE;
    fn GetCursorPos(point: *mut POINT) -> i32;
    fn ScreenToClient(hwnd: HWND, point: *mut POINT) -> i32;
    fn AdjustWindowRect(rect: *mut RECT, style: u32, menu: i32) -> i32;
    fn SetProcessDPIAware() -> i32;
    fn GetDpiForSystem() -> u32;
    fn GetSystemMetrics(index: i32) -> i32;
    fn MessageBoxW(hwnd: HWND, text: *const u16, caption: *const u16, kind: u32) -> i32;
    fn MoveWindow(hwnd: HWND, x: i32, y: i32, width: i32, height: i32, repaint: i32) -> i32;
    fn OpenClipboard(owner: HWND) -> i32;
    fn EmptyClipboard() -> i32;
    fn SetClipboardData(format: u32, data: HANDLE) -> HANDLE;
    fn CloseClipboard() -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateCompatibleDC(hdc: HANDLE) -> HANDLE;
    fn CreateCompatibleBitmap(hdc: HANDLE, width: i32, height: i32) -> HANDLE;
    fn SelectObject(hdc: HANDLE, object: HANDLE) -> HANDLE;
    fn DeleteObject(object: HANDLE) -> i32;
    fn DeleteDC(hdc: HANDLE) -> i32;
    fn BitBlt(
        dest: HANDLE,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        src: HANDLE,
        src_x: i32,
        src_y: i32,
        rop: u32,
    ) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> HANDLE;
    fn GlobalAlloc(flags: u32, bytes: usize) -> HANDLE;
    fn GlobalLock(mem: HANDLE) -> *mut c_void;
    fn GlobalUnlock(mem: HANDLE) -> i32;
    fn GlobalFree(mem: HANDLE) -> HANDLE;
    fn CreateMutexW(attributes: *const c_void, owner: i32, name: *const u16) -> HANDLE;
    fn CloseHandle(handle: HANDLE) -> i32;
    fn GetLastError() -> u32;
}

const WS_CAPTION: u32 = 0x00C0_0000;
const WS_SYSMENU: u32 = 0x0008_0000;
const WS_MINIMIZEBOX: u32 = 0x0002_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_TABSTOP: u32 = 0x0001_0000;
const BS_OWNERDRAW: u32 = 0xB;
const WINDOW_STYLE: u32 = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;

const WM_DESTROY: u32 = 0x0002;
const WM_PAINT: u32 = 0x000F;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_DRAWITEM: u32 = 0x002B;
const WM_SETICON: u32 = 0x0080;
const WM_COMMAND: u32 = 0x0111;
const WM_TIMER: u32 = 0x0113;
const WM_SETCURSOR: u32 = 0x0020;
const WM_LBUTTONUP: u32 = 0x0202;
const HTCLIENT: usize = 1;
/// Posted by the update thread when the install finishes.
const WM_UPDATE_DONE: u32 = 0x8001;

const ODS_SELECTED: u32 = 0x1;
const ODS_DISABLED: u32 = 0x4;
const ODS_FOCUS: u32 = 0x10;
const ODS_NOFOCUSRECT: u32 = 0x200;
const SW_HIDE: i32 = 0;
const SW_SHOW: i32 = 5;
const SM_CXICON: i32 = 11;
const SM_CXSMICON: i32 = 49;
const SRCCOPY: u32 = 0x00CC_0020;
const ERROR_ALREADY_EXISTS: u32 = 183;
const MB_OKCANCEL: u32 = 0x1;
const MB_ICONERROR: u32 = 0x10;
const MB_ICONQUESTION: u32 = 0x20;
const IDOK: i32 = 1;
const CW_USEDEFAULT: i32 = 0x8000_0000_u32 as i32;
const IDC_ARROW: usize = 32512;
const IDC_HAND: usize = 32649;

/// The card row that copies the alliance id when clicked.
const ALLIANCE_ROW: usize = 0;
const CF_UNICODETEXT: u32 = 13;
const GMEM_MOVEABLE: u32 = 0x0002;

const ID_UPDATE: usize = 200;
const ID_COPY_JSON: usize = 201;
const TIMER_ID: usize = 1;

/// How long a note such as "Copied 97 players" stays in the footer.
const NOTE_TIME: Duration = Duration::from_secs(5);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Shared with the window procedure, which has no other way to reach it.
static STATE: OnceLock<Arc<Mutex<State>>> = OnceLock::new();

/// Window-thread data.
struct Ui {
    window: HWND,
    /// The update button.
    button: HWND,
    copy_button: HWND,
    scale: f32,
    /// What is painted now.
    shown: Screen,
    /// A short note shown in the footer in place of its text, and until when.
    note: Option<(String, Mark, Instant)>,
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

/// Shows a message box, for failures before or outside the window. Returns the button pressed.
fn message_box(owner: HWND, text: &str, kind: u32) -> i32 {
    let (text, caption) = (wide(text), wide("LastWarHQ Scanner"));
    // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call.
    unsafe { MessageBoxW(owner, text.as_ptr(), caption.as_ptr(), kind) }
}

/// Shows an error in a message box, for failures before or outside the window.
pub fn error_box(text: &str) {
    message_box(std::ptr::null_mut(), text, MB_ICONERROR);
}

/// True if another copy of the tool is still running after waiting up to `wait` for it to
/// close. The named mutex lives as long as the process.
pub fn already_running(wait: Duration) -> bool {
    let name = wide("Local\\lastwarhq-scanner-single-instance");
    let deadline = Instant::now() + wait;
    loop {
        // SAFETY: `name` is a NUL-terminated UTF-16 string; no security attributes. A handle
        // to another process's mutex is closed before trying again.
        unsafe {
            let mutex = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            if mutex.is_null() || GetLastError() != ERROR_ALREADY_EXISTS {
                return false;
            }
            CloseHandle(mutex);
        }
        if Instant::now() >= deadline {
            return true;
        }
        thread::sleep(Duration::from_millis(250));
    }
}

/// Opens the window and runs its message loop until it is closed.
pub fn run(shared: Arc<Mutex<State>>) -> Result<(), String> {
    STATE.set(shared).map_err(|_| "window already running")?;
    paint::start()?;
    // SAFETY: plain Win32 calls with valid, NUL-terminated strings and zeroed out-parameters;
    // every handle used comes from the call that created it.
    unsafe {
        SetProcessDPIAware();
        let scale = GetDpiForSystem() as f32 / 96.0;
        let px = |v: f32| (v * scale).round() as i32;
        let instance = GetModuleHandleW(std::ptr::null());
        let (icon, small_icon) = (
            paint::app_icon(GetSystemMetrics(SM_CXICON)),
            paint::app_icon(GetSystemMetrics(SM_CXSMICON)),
        );
        let class_name = wide("LastWarHQScannerWindow");
        let class = WNDCLASSEXW {
            size: size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            wnd_proc: window_proc,
            cls_extra: 0,
            wnd_extra: 0,
            instance,
            icon,
            cursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16),
            background: std::ptr::null_mut(),
            menu_name: std::ptr::null(),
            class_name: class_name.as_ptr(),
            icon_small: small_icon,
        };
        if RegisterClassExW(&class) == 0 {
            return Err("cannot register the window class".into());
        }

        let mut rect = RECT {
            left: 0,
            top: 0,
            right: px(paint::WIDTH),
            bottom: px(paint::HEIGHT),
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
        SendMessageW(window, WM_SETICON, 1, icon as isize);
        SendMessageW(window, WM_SETICON, 0, small_icon as isize);

        // Hidden until there is an update.
        let (bx, by, bw, bh) = paint::BUTTON;
        let (button_class, no_text) = (wide("BUTTON"), wide(""));
        let button = CreateWindowExW(
            0,
            button_class.as_ptr(),
            no_text.as_ptr(),
            WS_CHILD | WS_TABSTOP | BS_OWNERDRAW,
            px(bx),
            px(by),
            px(bw),
            px(bh),
            window,
            ID_UPDATE as HANDLE,
            instance,
            std::ptr::null_mut(),
        );
        // Placed by `place_buttons`.
        let copy_button = CreateWindowExW(
            0,
            button_class.as_ptr(),
            no_text.as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW,
            0,
            0,
            0,
            0,
            window,
            ID_COPY_JSON as HANDLE,
            instance,
            std::ptr::null_mut(),
        );

        let shown = current_screen();
        UI.with(|ui| {
            *ui.borrow_mut() = Some(Ui {
                window,
                button,
                copy_button,
                scale,
                shown: shown.clone(),
                note: None,
            })
        });
        place_buttons(button, copy_button, scale, &shown);
        SetTimer(window, TIMER_ID, 1000, std::ptr::null());
        ShowWindow(window, SW_SHOW);
        UpdateWindow(window);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

/// The screen as of now, after the weekly reset if one is due, with the footer note if one
/// is showing.
fn current_screen() -> Screen {
    let now = now();
    let mut screen = {
        let mut s = state();
        s.tick(now);
        screen(&s, now)
    };
    let note = UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        let ui = ui.as_mut()?;
        if ui
            .note
            .as_ref()
            .is_some_and(|(_, _, until)| Instant::now() >= *until)
        {
            ui.note = None;
        }
        ui.note.clone()
    });
    if let Some((text, mark, _)) = note {
        screen.footer.text = text;
        screen.footer.mark = Some(mark);
    }
    screen
}

/// Shows, hides and enables the update button for `screen`, and puts the Copy JSON button
/// beside it.
fn place_buttons(button: HWND, copy_button: HWND, scale: f32, screen: &Screen) {
    let px = |v: f32| (v * scale).round() as i32;
    let (x, y, w, h) = paint::copy_button(screen.footer.button.is_some());
    // SAFETY: both buttons belong to this thread's window.
    unsafe {
        match screen.footer.button {
            Some((_, enabled)) => {
                EnableWindow(button, i32::from(enabled));
                ShowWindow(button, SW_SHOW);
                InvalidateRect(button, std::ptr::null(), 0);
            }
            None => {
                ShowWindow(button, SW_HIDE);
            }
        }
        MoveWindow(copy_button, px(x), px(y), px(w), px(h), 1);
    }
}

/// Rereads the shared state and repaints if the screen changed. The window data is not
/// borrowed while Windows is called, because Windows may call the window procedure straight
/// back, which reads it.
fn refresh() {
    let next = current_screen();
    let changed = UI.with(|ui| {
        let mut ui = ui.borrow_mut();
        let ui = ui.as_mut()?;
        if ui.shown == next {
            return None;
        }
        ui.shown = next.clone();
        Some((ui.window, ui.button, ui.copy_button, ui.scale))
    });
    if let Some((window, button, copy_button, scale)) = changed {
        // SAFETY: the window belongs to this thread.
        unsafe { InvalidateRect(window, std::ptr::null(), 0) };
        place_buttons(button, copy_button, scale, &next);
    }
}

/// Paints the whole client area through an off-screen bitmap, so it never flickers.
fn paint_window(hwnd: HWND) {
    let Some((scale, screen)) =
        UI.with(|ui| ui.borrow().as_ref().map(|u| (u.scale, u.shown.clone())))
    else {
        return;
    };
    let (w, h) = (
        (paint::WIDTH * scale).round() as i32,
        (paint::HEIGHT * scale).round() as i32,
    );
    // SAFETY: BeginPaint/EndPaint pair on this window; the memory DC and bitmap are made,
    // used and deleted here, with the DC's original bitmap selected back first.
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mem = CreateCompatibleDC(hdc);
        let bitmap = CreateCompatibleBitmap(hdc, w, h);
        let old = SelectObject(mem, bitmap);
        Painter::new(mem, scale).screen(&screen);
        BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old);
        DeleteObject(bitmap);
        DeleteDC(mem);
        EndPaint(hwnd, &ps);
    }
}

fn draw_button(item: &DRAWITEMSTRUCT) {
    let Some((scale, label)) = UI.with(|ui| {
        ui.borrow()
            .as_ref()
            .map(|u| (u.scale, u.shown.footer.button.map(|(l, _)| l)))
    }) else {
        return;
    };
    let (label, primary) = match item.ctl_id as usize {
        ID_COPY_JSON => ("Copy JSON", false),
        _ => (label.unwrap_or(""), true),
    };
    let s = item.item_state;
    let state = ButtonState {
        pressed: s & ODS_SELECTED != 0,
        enabled: s & ODS_DISABLED == 0,
        focus: s & ODS_FOCUS != 0 && s & ODS_NOFOCUSRECT == 0,
    };
    // SAFETY: Windows passes a DC that is valid for the whole WM_DRAWITEM call.
    let mut painter = unsafe { Painter::new(item.hdc, scale) };
    painter.button(item.rect, label, primary, state);
}

/// Shows `text` in the footer for a few seconds.
fn note(text: String, mark: Mark) {
    UI.with(|ui| {
        if let Some(ui) = ui.borrow_mut().as_mut() {
            ui.note = Some((text, mark, Instant::now() + NOTE_TIME));
        }
    });
    refresh();
}

/// Whether the client-area point (device pixels) is on the Alliance row while there is an
/// alliance id to copy.
fn on_alliance_id(x: i32, y: i32) -> bool {
    let Some(scale) = UI.with(|ui| ui.borrow().as_ref().map(|u| u.scale)) else {
        return false;
    };
    paint::row_at(x as f32 / scale, y as f32 / scale) == Some(ALLIANCE_ROW)
        && state().view.alliance_id().is_some()
}

/// Copies the alliance id, the one id the sync is keyed on.
fn copy_alliance_id() {
    let Some(id) = state().view.alliance_id().map(str::to_string) else {
        return;
    };
    match set_clipboard(&id) {
        Ok(()) => note("Copied alliance ID".to_string(), Mark::Done),
        Err(err) => note(format!("Copy failed: {err}"), Mark::Failed),
    }
}

/// Copies the payload the sync will send, for checking it by hand.
fn copy_json_clicked() {
    let (payload, members) = {
        let mut s = state();
        let payload = payload_json(&mut s, now());
        (
            payload,
            s.view.roster.as_ref().map_or(0, |r| r.entries.len()),
        )
    };
    match set_clipboard(&payload) {
        Ok(()) => note(format!("Copied JSON · {members} members"), Mark::Done),
        Err(err) => note(format!("Copy failed: {err}"), Mark::Failed),
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

/// Asks first, since restarting clears what the tool has loaded, then installs on a
/// background thread, which posts WM_UPDATE_DONE with the result.
fn update_clicked() {
    let Some(window) = UI.with(|ui| ui.borrow().as_ref().map(|u| u.window)) else {
        return;
    };
    let current = Version::current();
    let Some(version) = state().update.latest.filter(|v| *v > current) else {
        return;
    };
    let question = format!(
        "Update LastWarHQ Scanner from version {current} to {version}?\n\n\
         The tool restarts to finish. Restarting clears the data it has loaded so far, so \
         you'll need to open the game panels again.\n\n\
         Windows may ask for admin approval again when it starts."
    );
    if message_box(window, &question, MB_OKCANCEL | MB_ICONQUESTION) != IDOK {
        return;
    }
    state().update.install = Install::Downloading;
    refresh();
    let window = window as usize;
    thread::spawn(move || {
        let result = std::panic::catch_unwind(|| install::install(version)).unwrap_or_else(|p| {
            Err(format!(
                "internal error: {}",
                crate::app::panic_text(p.as_ref())
            ))
        });
        let result = Box::into_raw(Box::new(result)) as isize;
        // SAFETY: the window outlives the update thread (closing it ends the process); the
        // window procedure takes back ownership of the boxed result.
        unsafe { PostMessageW(window as HWND, WM_UPDATE_DONE, 0, result) };
    });
}

/// Starts the new version, which waits for this one to close, and closes this one.
fn update_done(result: Result<PathBuf, String>) {
    match result {
        Ok(exe) => {
            if let Err(err) = std::process::Command::new(&exe)
                .arg(install::UPDATED_ARG)
                .spawn()
            {
                error_box(&format!(
                    "The update is installed, but the new version could not be started: {err}\n\nStart LastWarHQ Scanner again to use it."
                ));
            }
            // SAFETY: ends this thread's message loop, and with it the process.
            unsafe { PostQuitMessage(0) };
        }
        Err(err) => {
            state().update.install = Install::Failed(err);
            refresh();
        }
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
        WM_PAINT => {
            paint_window(hwnd);
            return 0;
        }
        // Painting covers the whole client area.
        WM_ERASEBKGND => return 1,
        WM_LBUTTONUP => {
            // The low and high words of `lparam` are the signed client x and y.
            let (x, y) = (lparam as i16 as i32, (lparam >> 16) as i16 as i32);
            if on_alliance_id(x, y) {
                copy_alliance_id();
            }
        }
        // A hand over the Alliance row, which copies the id when clicked.
        WM_SETCURSOR if hwnd == wparam as HWND && lparam as usize & 0xffff == HTCLIENT => {
            let mut point = POINT { x: 0, y: 0 };
            // SAFETY: `point` is a valid out-parameter; `hwnd` is this window.
            let over =
                unsafe { GetCursorPos(&mut point) != 0 && ScreenToClient(hwnd, &mut point) != 0 }
                    && on_alliance_id(point.x, point.y);
            if over {
                // SAFETY: a system cursor, loaded and set on this thread.
                unsafe { SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_HAND as *const u16)) };
                return 1;
            }
        }
        WM_DRAWITEM => {
            // SAFETY: for WM_DRAWITEM, `lparam` points at a DRAWITEMSTRUCT valid for the call.
            draw_button(unsafe { &*(lparam as *const DRAWITEMSTRUCT) });
            return 1;
        }
        WM_COMMAND if wparam & 0xffff == ID_UPDATE => update_clicked(),
        WM_COMMAND if wparam & 0xffff == ID_COPY_JSON => copy_json_clicked(),
        WM_UPDATE_DONE => {
            // SAFETY: `lparam` is the boxed result leaked by the update thread for this message.
            let result = unsafe { *Box::from_raw(lparam as *mut Result<PathBuf, String>) };
            update_done(result);
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
