//! The trucks window: the UR and reindeer trucks on the server being viewed, on a map like the
//! secret tasks window's, moving as they drive, with a power slider and a "Reindeer only"
//! switch. Clicking a truck in the list selects it, which draws its way ahead on the map, and
//! copies where it is now. A one-second timer rereads the shared state and repaints. It runs on
//! the status window's thread, through its message loop.

#![allow(clippy::upper_case_acronyms)]

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::time::{Duration, Instant};

use crate::ui::dock;
use crate::ui::paint::{self, HANDLE, Painter, RECT};
use crate::ui::trucks::{Filter, TrucksScreen, trucks_screen};
use crate::ui::window::{error_box, set_clipboard, state, wide};
use crate::util::time::now;

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
struct PAINTSTRUCT {
    hdc: HANDLE,
    erase: i32,
    paint: RECT,
    restore: i32,
    inc_update: i32,
    reserved: [u8; 32],
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
    fn SetTimer(hwnd: HWND, id: usize, ms: u32, func: *const c_void) -> usize;
    fn KillTimer(hwnd: HWND, id: usize) -> i32;
    fn DestroyWindow(hwnd: HWND) -> i32;
    fn ShowWindow(hwnd: HWND, cmd: i32) -> i32;
    fn SetForegroundWindow(hwnd: HWND) -> i32;
    fn InvalidateRect(hwnd: HWND, rect: *const RECT, erase: i32) -> i32;
    fn BeginPaint(hwnd: HWND, paint: *mut PAINTSTRUCT) -> HANDLE;
    fn EndPaint(hwnd: HWND, paint: *const PAINTSTRUCT) -> i32;
    fn LoadCursorW(instance: HANDLE, name: *const u16) -> HANDLE;
    fn SetCursor(cursor: HANDLE) -> HANDLE;
    fn GetCursorPos(point: *mut POINT) -> i32;
    fn ScreenToClient(hwnd: HWND, point: *mut POINT) -> i32;
    fn SetCapture(hwnd: HWND) -> HWND;
    fn ReleaseCapture() -> i32;
    fn AdjustWindowRect(rect: *mut RECT, style: u32, menu: i32) -> i32;
    fn GetDpiForSystem() -> u32;
    fn GetSystemMetrics(index: i32) -> i32;
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
}

const WS_CAPTION: u32 = 0x00C0_0000;
const WS_SYSMENU: u32 = 0x0008_0000;
const WS_MINIMIZEBOX: u32 = 0x0002_0000;
const WINDOW_STYLE: u32 = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;

const WM_DESTROY: u32 = 0x0002;
const WM_PAINT: u32 = 0x000F;
const WM_CLOSE: u32 = 0x0010;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_SETCURSOR: u32 = 0x0020;
const WM_SETICON: u32 = 0x0080;
const WM_TIMER: u32 = 0x0113;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_CAPTURECHANGED: u32 = 0x0215;
/// One notch of the mouse wheel.
const WHEEL_DELTA: i32 = 120;
const HTCLIENT: usize = 1;

const SW_SHOW: i32 = 5;
const SW_RESTORE: i32 = 9;
const SM_CXICON: i32 = 11;
const SM_CXSMICON: i32 = 49;
const SRCCOPY: u32 = 0x00CC_0020;
const CW_USEDEFAULT: i32 = 0x8000_0000_u32 as i32;
const IDC_ARROW: usize = 32512;
const IDC_HAND: usize = 32649;
const TIMER_ID: usize = 1;

/// How long "Copied …" shows in place of the summary.
const NOTE_TIME: Duration = Duration::from_secs(3);

/// Window-thread data.
struct TrucksUi {
    window: HWND,
    scale: f32,
    /// What is painted now.
    shown: TrucksScreen,
    /// A note shown in place of the summary, and until when.
    note: Option<(String, Instant)>,
    /// How many truck lines the list is scrolled down.
    scroll: usize,
    /// The power slider's knob is being dragged.
    dragging: bool,
}

thread_local! {
    static TRUCKS: RefCell<Option<TrucksUi>> = const { RefCell::new(None) };
    static REGISTERED: Cell<bool> = const { Cell::new(false) };
    /// Which trucks are shown, and the one whose route is drawn; kept while the tool runs.
    static FILTER: Cell<Option<Filter>> = const { Cell::new(None) };
    static SELECTED: Cell<Option<u64>> = const { Cell::new(None) };
}

fn filter() -> Filter {
    FILTER.get().unwrap_or_default()
}

/// Opens the window, or brings it to the front if it is open.
pub fn open() {
    if let Some(window) = handle() {
        // SAFETY: the window belongs to this thread.
        unsafe {
            ShowWindow(window, SW_RESTORE);
            SetForegroundWindow(window);
        }
        return;
    }
    if let Err(err) = create() {
        error_box(&format!("The trucks map can't be opened: {err}"));
    }
}

/// The window, while it is open.
pub(super) fn handle() -> Option<HWND> {
    TRUCKS.with(|t| t.borrow().as_ref().map(|u| u.window))
}

fn create() -> Result<(), String> {
    // SAFETY: plain Win32 calls with valid, NUL-terminated strings and zeroed out-parameters;
    // every handle used comes from the call that created it, on this thread.
    unsafe {
        let scale = GetDpiForSystem() as f32 / 96.0;
        let px = |v: f32| (v * scale).round() as i32;
        let instance = GetModuleHandleW(std::ptr::null());
        let class_name = wide("LastWarHQScannerTrucks");
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
                background: std::ptr::null_mut(),
                menu_name: std::ptr::null(),
                class_name: class_name.as_ptr(),
                icon_small: small_icon,
            };
            if RegisterClassExW(&class) == 0 {
                return Err("cannot register the window class".into());
            }
            REGISTERED.set(true);
        }
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: px(paint::TASKS_WIDTH),
            bottom: px(paint::TASKS_HEIGHT),
        };
        AdjustWindowRect(&mut rect, WINDOW_STYLE, 0);
        let title = wide("LastWarHQ Scanner · Trucks");
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
        let shown = current_screen();
        TRUCKS.with(|t| {
            *t.borrow_mut() = Some(TrucksUi {
                window,
                scale,
                shown,
                note: None,
                scroll: 0,
                dragging: false,
            })
        });
        SetTimer(window, TIMER_ID, 1000, std::ptr::null());
        ShowWindow(window, SW_SHOW);
    }
    dock::follow();
    Ok(())
}

/// The screen as of now, on the game server's clock.
fn current_screen() -> TrucksScreen {
    let s = state();
    let game_now = s.game_clock(now());
    trucks_screen(
        &s.trucks,
        &s.world,
        s.view.alliance_id(),
        game_now,
        filter(),
        SELECTED.get(),
    )
}

/// Rereads the shared state and repaints if the screen changed or a note ran out. The window
/// data is not borrowed while Windows is called, which may call the window procedure back.
fn refresh() {
    let next = current_screen();
    let window = TRUCKS.with(|t| {
        let mut t = t.borrow_mut();
        let ui = t.as_mut()?;
        let note_over = ui
            .note
            .as_ref()
            .is_some_and(|(_, until)| Instant::now() >= *until);
        if note_over {
            ui.note = None;
        }
        if ui.shown == next && !note_over {
            return None;
        }
        ui.shown = next;
        Some(ui.window)
    });
    if let Some(window) = window {
        // SAFETY: the window belongs to this thread.
        unsafe { InvalidateRect(window, std::ptr::null(), 0) };
    }
}

/// Paints the whole client area through an off-screen bitmap, so it never flickers.
fn paint_window(hwnd: HWND) {
    let Some((scale, screen, note, scroll)) = TRUCKS.with(|t| {
        t.borrow().as_ref().map(|u| {
            (
                u.scale,
                u.shown.clone(),
                u.note.as_ref().map(|n| n.0.clone()),
                u.scroll,
            )
        })
    }) else {
        return;
    };
    let (w, h) = (
        (paint::TASKS_WIDTH * scale).round() as i32,
        (paint::TASKS_HEIGHT * scale).round() as i32,
    );
    // SAFETY: BeginPaint/EndPaint pair on this window; the memory DC and bitmap are made,
    // used and deleted here, with the DC's original bitmap selected back first.
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mem = CreateCompatibleDC(hdc);
        let bitmap = CreateCompatibleBitmap(hdc, w, h);
        let old = SelectObject(mem, bitmap);
        Painter::new(mem, scale).trucks(&screen, note.as_deref(), scroll);
        BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old);
        DeleteObject(bitmap);
        DeleteDC(mem);
        EndPaint(hwnd, &ps);
    }
}

/// What a click at the client-area point (device pixels) would do.
enum Target {
    /// Show only reindeer trucks, or every kind.
    ReindeerOnly,
    /// Set the power with the slider.
    Slider,
    /// Select this truck, at this place if known.
    Line(u64, Option<(u32, u32)>),
}

/// The client-area point (device pixels) in 96-DPI pixels.
fn unscaled(x: i32, y: i32) -> Option<(f32, f32)> {
    let scale = TRUCKS.with(|t| t.borrow().as_ref().map(|u| u.scale))?;
    Some((x as f32 / scale, y as f32 / scale))
}

fn target(x: i32, y: i32) -> Option<Target> {
    let (x, y) = unscaled(x, y)?;
    if paint::on_reindeer_toggle(x, y) {
        return Some(Target::ReindeerOnly);
    }
    if paint::on_slider(x, y) {
        return Some(Target::Slider);
    }
    TRUCKS.with(|t| {
        let t = t.borrow();
        let ui = t.as_ref()?;
        let line = paint::task_line_at(x, y, ui.shown.lines.len(), ui.scroll)?;
        let line = &ui.shown.lines[line];
        Some(Target::Line(line.id, line.at))
    })
}

/// Sets the power from the slider's knob at the client-area x (device pixels).
fn slide(x: i32) {
    let Some((x, _)) = unscaled(x, 0) else {
        return;
    };
    let max_power = paint::slider_power(x);
    let f = filter();
    if f.max_power != max_power {
        FILTER.set(Some(Filter { max_power, ..f }));
        refresh();
    }
}

fn pressed(x: i32, y: i32) {
    if !matches!(target(x, y), Some(Target::Slider)) {
        return;
    }
    let window = TRUCKS.with(|t| {
        let mut t = t.borrow_mut();
        let ui = t.as_mut()?;
        ui.dragging = true;
        Some(ui.window)
    });
    if let Some(window) = window {
        // SAFETY: the window belongs to this thread; the capture is released on button up.
        unsafe { SetCapture(window) };
        slide(x);
    }
}

fn dragging() -> bool {
    TRUCKS.with(|t| t.borrow().as_ref().is_some_and(|u| u.dragging))
}

fn stop_dragging() {
    TRUCKS.with(|t| {
        if let Some(ui) = t.borrow_mut().as_mut() {
            ui.dragging = false;
        }
    });
}

fn released(x: i32, y: i32) {
    if dragging() {
        stop_dragging();
        // SAFETY: releases this thread's capture, taken when the drag began.
        unsafe { ReleaseCapture() };
        return;
    }
    match target(x, y) {
        Some(Target::ReindeerOnly) => {
            let f = filter();
            FILTER.set(Some(Filter {
                reindeer_only: !f.reindeer_only,
                ..f
            }));
            refresh();
        }
        Some(Target::Line(id, at)) => {
            SELECTED.set(Some(id));
            let note = match at {
                Some((x, y)) => match set_clipboard(&format!("X:{x} Y:{y}")) {
                    Ok(()) => format!("Copied X {x} Y {y} · route drawn"),
                    Err(err) => format!("Copy failed: {err}"),
                },
                None => "Route drawn once its cities are placed".to_string(),
            };
            let window = TRUCKS.with(|t| {
                let mut t = t.borrow_mut();
                let ui = t.as_mut()?;
                ui.note = Some((note, Instant::now() + NOTE_TIME));
                Some(ui.window)
            });
            refresh();
            if let Some(window) = window {
                // SAFETY: the window belongs to this thread.
                unsafe { InvalidateRect(window, std::ptr::null(), 0) };
            }
        }
        Some(Target::Slider) | None => {}
    }
}

/// Scrolls the truck list by one line per notch of the wheel, `delta` up (positive) or down.
fn wheel(delta: i32) {
    let window = TRUCKS.with(|t| {
        let mut t = t.borrow_mut();
        let ui = t.as_mut()?;
        let max = paint::max_scroll(ui.shown.lines.len());
        let lines = delta / WHEEL_DELTA;
        let scroll = (ui.scroll.min(max) as i32 - lines).clamp(0, max as i32) as usize;
        (scroll != ui.scroll).then(|| {
            ui.scroll = scroll;
            ui.window
        })
    });
    if let Some(window) = window {
        // SAFETY: the window belongs to this thread.
        unsafe { InvalidateRect(window, std::ptr::null(), 0) };
    }
}

fn destroyed() {
    if let Some(ui) = TRUCKS.with(|t| t.borrow_mut().take()) {
        // SAFETY: the timer was made for this window, which is going.
        unsafe { KillTimer(ui.window, TIMER_ID) };
    }
}

/// The window procedure. A panic must not unwind into Windows (that aborts the process), so
/// one is caught, reported, and this window closed.
unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        handle_message(hwnd, msg, wparam, lparam)
    })) {
        Ok(result) => result,
        Err(panic) => {
            let what = crate::app::panic_text(panic.as_ref());
            error_box(&format!(
                "The trucks map hit an internal error and will close:\n{what}"
            ));
            // SAFETY: `hwnd` is this window.
            unsafe { DestroyWindow(hwnd) };
            0
        }
    }
}

fn handle_message(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    // The low and high words of `lparam` are the signed client x and y, for mouse messages.
    let (x, y) = (lparam as i16 as i32, (lparam >> 16) as i16 as i32);
    match msg {
        WM_TIMER => refresh(),
        WM_PAINT => {
            paint_window(hwnd);
            return 0;
        }
        // Painting covers the whole client area.
        WM_ERASEBKGND => return 1,
        // The high word of `wparam` is the signed distance the wheel turned.
        WM_MOUSEWHEEL => {
            wheel((wparam >> 16) as i16 as i32);
            return 0;
        }
        WM_LBUTTONDOWN => pressed(x, y),
        WM_MOUSEMOVE if dragging() => slide(x),
        WM_LBUTTONUP => released(x, y),
        WM_CAPTURECHANGED => stop_dragging(),
        // A hand over what does something when clicked.
        WM_SETCURSOR if hwnd == wparam as HWND && lparam as usize & 0xffff == HTCLIENT => {
            let mut point = POINT { x: 0, y: 0 };
            // SAFETY: `point` is a valid out-parameter; `hwnd` is this window.
            let over =
                unsafe { GetCursorPos(&mut point) != 0 && ScreenToClient(hwnd, &mut point) != 0 }
                    && target(point.x, point.y).is_some();
            if over {
                // SAFETY: a system cursor, loaded and set on this thread.
                unsafe { SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_HAND as *const u16)) };
                return 1;
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
