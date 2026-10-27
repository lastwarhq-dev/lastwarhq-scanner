//! Keeps the tool's windows docked beside the game window: the status window against its left
//! edge, and the secret tasks map and the trucks map against its right edge, tops aligned, the
//! trucks map below the tasks map while that is open. They move when the game window moves or
//! resizes, minimise and restore with it, and come forward with it.
//!
//! Only the game window is looked at, never the game process: the window is found by its class
//! and title, its frame read from the window manager, and its moves come as out-of-context
//! window events (`SetWinEventHook`), which load nothing into the game. A window moved by hand
//! stays put until the game window next moves. Placement uses the visible frames (without the
//! invisible resize borders), so the windows touch the game's.
//!
//! Runs on the status window's thread, which pumps the window events through its message loop.

#![allow(clippy::upper_case_acronyms)]

use std::cell::RefCell;
use std::ffi::c_void;

use crate::ui::paint::{HANDLE, RECT};
use crate::ui::window::wide;
use crate::ui::{tasks_window, trucks_window, window};

type HWND = HANDLE;
type WinEventProc = unsafe extern "system" fn(HANDLE, u32, HWND, i32, i32, u32, u32);
type MonitorEnumProc = unsafe extern "system" fn(HANDLE, HANDLE, *mut RECT, isize) -> i32;

#[repr(C)]
struct POINT {
    x: i32,
    y: i32,
}

#[repr(C)]
struct MONITORINFO {
    size: u32,
    monitor: RECT,
    work: RECT,
    flags: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> HWND;
    fn IsWindow(hwnd: HWND) -> i32;
    fn IsWindowVisible(hwnd: HWND) -> i32;
    fn IsIconic(hwnd: HWND) -> i32;
    fn GetWindowRect(hwnd: HWND, rect: *mut RECT) -> i32;
    fn ClientToScreen(hwnd: HWND, point: *mut POINT) -> i32;
    fn GetWindowThreadProcessId(hwnd: HWND, pid: *mut u32) -> u32;
    fn SetWindowPos(hwnd: HWND, after: HWND, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32;
    fn ShowWindow(hwnd: HWND, cmd: i32) -> i32;
    fn SetWinEventHook(
        min: u32,
        max: u32,
        module: HANDLE,
        proc: WinEventProc,
        pid: u32,
        thread: u32,
        flags: u32,
    ) -> HANDLE;
    fn UnhookWinEvent(hook: HANDLE) -> i32;
    fn EnumDisplayMonitors(
        hdc: HANDLE,
        clip: *const RECT,
        proc: MonitorEnumProc,
        data: isize,
    ) -> i32;
    fn GetMonitorInfoW(monitor: HANDLE, info: *mut MONITORINFO) -> i32;
}

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmGetWindowAttribute(hwnd: HWND, attribute: u32, value: *mut c_void, size: u32) -> i32;
}

/// The game window's class and title.
const GAME_CLASS: &str = "UnityWndClass";
const GAME_TITLE: &str = "Last War-Survival Game";

/// How much of a docked window's width stays on a screen, with its title bar, when the docked
/// spot would put it off every screen.
pub const MIN_VISIBLE: i32 = 100;

const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_SYSTEM_MINIMIZESTART: u32 = 0x0016;
const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
const EVENT_OBJECT_LOCATIONCHANGE: u32 = 0x800B;
const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;
const WINEVENT_SKIPOWNPROCESS: u32 = 0x0002;
const OBJID_WINDOW: i32 = 0;
const CHILDID_SELF: i32 = 0;
const DWMWA_EXTENDED_FRAME_BOUNDS: u32 = 9;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
const HWND_TOP: HWND = std::ptr::null_mut();
const SW_SHOWNOACTIVATE: i32 = 4;
const SW_SHOWMINNOACTIVE: i32 = 7;

/// Which edge of the game window a window docks against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Side {
    Left,
    Right,
}

/// Where a window whose visible frame is `width` wide goes beside `game` (its visible frame),
/// as the top-left corner of its frame. `grip` is the height of its title bar, and `screens`
/// the screens' work areas.
///
/// The docked spot is kept as long as the title bar and [`MIN_VISIBLE`] of the width are on
/// one screen, even if the rest is off every screen. Otherwise the window moves the least it
/// takes to bring that much onto the nearest screen, over the game's edge if need be. With no
/// screen it could fit, it stays at the docked spot.
pub fn place(game: RECT, side: Side, width: i32, grip: i32, screens: &[RECT]) -> (i32, i32) {
    let x = match side {
        Side::Left => game.left - width,
        Side::Right => game.right,
    };
    let y = game.top;
    let keep = MIN_VISIBLE.min(width);
    screens
        .iter()
        .filter(|s| s.right - s.left >= keep && s.bottom - s.top >= grip)
        .map(|s| {
            (
                x.clamp(s.left + keep - width, s.right - keep),
                y.clamp(s.top, s.bottom - grip),
            )
        })
        .min_by_key(|(nx, ny)| (nx - x).abs() + (ny - y).abs())
        .unwrap_or((x, y))
}

/// The game window being followed, and the window events asked for on its process.
struct Game {
    window: HWND,
    hooks: Vec<HANDLE>,
}

/// One of the tool's windows as last docked: for which game frame and screens.
#[derive(PartialEq)]
struct Placed {
    window: HWND,
    game: RECT,
    screens: Vec<RECT>,
}

#[derive(Default)]
struct Dock {
    game: Option<Game>,
    /// Per window (see [`windows`]), as last docked; it is docked again once its place beside
    /// the game frame, the screens or the window itself differ.
    placed: [Option<Placed>; 3],
    /// The tool's windows minimised because the game window was, to restore with it.
    minimised: Vec<HWND>,
}

thread_local! {
    static DOCK: RefCell<Dock> = RefCell::new(Dock::default());
}

/// Finds the game window if it isn't known, and docks the tool's windows that aren't where
/// the game window's frame puts them. Called every second, when a window opens, and on the
/// game window's events. A call made while one is running (Windows can call back into the
/// thread) does nothing; the next catches up.
pub fn follow() {
    DOCK.with(|dock| {
        if let Ok(mut dock) = dock.try_borrow_mut() {
            dock.follow();
        }
    });
}

/// The tool's windows, open or not, with the side each docks on, in stacking order.
fn windows() -> [(Side, Option<HWND>); 3] {
    [
        (Side::Left, window::handle()),
        (Side::Right, tasks_window::handle()),
        (Side::Right, trucks_window::handle()),
    ]
}

impl Dock {
    fn follow(&mut self) {
        let Some(game) = self.game_window() else {
            return;
        };
        let windows = windows();
        // SAFETY: `game` is a window handle checked by `game_window`; the tool's windows belong
        // to this thread; `frame` and the window calls take valid handles and out-parameters.
        unsafe {
            if IsIconic(game) != 0 {
                for window in windows.iter().filter_map(|(_, w)| *w) {
                    if IsIconic(window) == 0 {
                        ShowWindow(window, SW_SHOWMINNOACTIVE);
                        self.minimised.push(window);
                    }
                }
                return;
            }
            self.restore();
            let Some(game_frame) = frame(game) else {
                return;
            };
            let screens = screens();
            // Windows on the same side stack down from the game window's top, in order.
            let mut below = [0; 2];
            for (slot, (side, window)) in windows.into_iter().enumerate() {
                let Some(window) = window else {
                    continue;
                };
                if IsIconic(window) != 0 {
                    continue;
                }
                let beside = RECT {
                    top: game_frame.top + below[side as usize],
                    ..game_frame
                };
                let placed = Placed {
                    window,
                    game: beside,
                    screens: screens.clone(),
                };
                if self.placed[slot].as_ref() != Some(&placed) {
                    dock(window, side, beside, &screens);
                    self.placed[slot] = Some(placed);
                }
                if let Some(f) = frame(window) {
                    below[side as usize] += f.bottom - f.top;
                }
            }
        }
    }

    /// The game window, looked for again (and its events asked for) when the one known has
    /// gone or is hidden.
    fn game_window(&mut self) -> Option<HWND> {
        // SAFETY: window checks and lookups with a valid handle or NUL-terminated strings; the
        // hooks are removed on the thread that set them.
        unsafe {
            if let Some(game) = &self.game {
                if IsWindow(game.window) != 0 && IsWindowVisible(game.window) != 0 {
                    return Some(game.window);
                }
                for hook in &game.hooks {
                    UnhookWinEvent(*hook);
                }
                self.game = None;
                self.placed = Default::default();
                self.restore();
            }
            let (class, title) = (wide(GAME_CLASS), wide(GAME_TITLE));
            let window = FindWindowW(class.as_ptr(), title.as_ptr());
            if window.is_null() || IsWindowVisible(window) == 0 {
                return None;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(window, &mut pid);
            let hooks = [
                (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
                (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
                (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
            ]
            .into_iter()
            .map(|(min, max)| {
                SetWinEventHook(
                    min,
                    max,
                    std::ptr::null_mut(),
                    game_event,
                    pid,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            })
            .filter(|hook| !hook.is_null())
            .collect();
            self.game = Some(Game { window, hooks });
            Some(window)
        }
    }

    /// Restores the tool's windows minimised with the game window.
    fn restore(&mut self) {
        for window in self.minimised.drain(..) {
            // SAFETY: a window of this thread's; one closed meanwhile is no longer a window.
            unsafe {
                if IsWindow(window) != 0 && IsIconic(window) != 0 {
                    ShowWindow(window, SW_SHOWNOACTIVATE);
                }
            }
        }
    }
}

/// Moves `window` to its docked spot on `side` of `game` (a visible frame).
///
/// # Safety
/// `window` must be a window of this thread's.
unsafe fn dock(window: HWND, side: Side, game: RECT, screens: &[RECT]) {
    // SAFETY: the caller passes a window of this thread's; the out-parameters are valid.
    unsafe {
        let mut outer = RECT::default();
        if GetWindowRect(window, &mut outer) == 0 {
            return;
        }
        let visible = frame(window).unwrap_or(outer);
        let mut client = POINT { x: 0, y: 0 };
        ClientToScreen(window, &mut client);
        let width = visible.right - visible.left;
        let grip = (client.y - visible.top).max(0);
        let (x, y) = place(game, side, width, grip, screens);
        // The window's position includes its invisible borders.
        SetWindowPos(
            window,
            std::ptr::null_mut(),
            x - (visible.left - outer.left),
            y - (visible.top - outer.top),
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// A window's visible frame, without the invisible resize borders, from the window manager.
///
/// # Safety
/// `window` must be a window handle.
unsafe fn frame(window: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    // SAFETY: `rect` is a RECT-sized out-parameter for this attribute.
    let result = unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut RECT as *mut c_void,
            size_of::<RECT>() as u32,
        )
    };
    (result == 0).then_some(rect)
}

/// The screens' work areas: each monitor without its taskbar.
fn screens() -> Vec<RECT> {
    unsafe extern "system" fn add(monitor: HANDLE, _: HANDLE, _: *mut RECT, data: isize) -> i32 {
        let mut info = MONITORINFO {
            size: size_of::<MONITORINFO>() as u32,
            monitor: RECT::default(),
            work: RECT::default(),
            flags: 0,
        };
        // SAFETY: `data` is the Vec passed to EnumDisplayMonitors below, alive for the whole
        // enumeration; `info` is sized as Windows expects.
        unsafe {
            if GetMonitorInfoW(monitor, &mut info) != 0 {
                (*(data as *mut Vec<RECT>)).push(info.work);
            }
        }
        1
    }
    let mut screens: Vec<RECT> = Vec::new();
    // SAFETY: the callback only pushes onto `screens`, which outlives the call.
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            add,
            &mut screens as *mut Vec<RECT> as isize,
        );
    }
    screens
}

/// Brings the tool's windows forward, without taking the focus from the game.
fn raise() {
    for window in windows().into_iter().filter_map(|(_, w)| w) {
        // SAFETY: a window of this thread's.
        unsafe {
            SetWindowPos(
                window,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        };
    }
}

/// The game process's window events, delivered through this thread's message loop. Only the
/// game window's own events count. A panic must not unwind into Windows, so one is dropped.
unsafe extern "system" fn game_event(
    _hook: HANDLE,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    if object != OBJID_WINDOW || child != CHILDID_SELF {
        return;
    }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let ours = DOCK.with(|dock| {
            dock.try_borrow()
                .is_ok_and(|d| d.game.as_ref().is_some_and(|g| g.window == hwnd))
        });
        if !ours {
            return;
        }
        if event == EVENT_SYSTEM_FOREGROUND {
            raise();
        }
        follow();
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    /// Three screens side by side, each with a 48-pixel taskbar.
    fn screens() -> Vec<RECT> {
        vec![
            rect(-1920, 0, 0, 1032),
            rect(0, 0, 2560, 1392),
            rect(2560, 0, 4480, 1032),
        ]
    }

    const GRIP: i32 = 30;

    #[test]
    fn docks_outside_each_edge_with_tops_aligned() {
        let game = rect(600, 40, 1900, 1000);
        assert_eq!(place(game, Side::Left, 400, GRIP, &screens()), (200, 40));
        assert_eq!(place(game, Side::Right, 500, GRIP, &screens()), (1900, 40));
    }

    #[test]
    fn a_neighbouring_screen_counts() {
        let game = rect(0, 40, 1300, 1000);
        assert_eq!(place(game, Side::Left, 400, GRIP, &screens()), (-400, 40));
    }

    #[test]
    fn may_hang_off_until_only_the_minimum_is_left() {
        let screens = [rect(0, 0, 2560, 1392)];
        // 50 of 400 pixels would show: moved in until 100 do, over the game's edge.
        let game = rect(50, 40, 1300, 1000);
        assert_eq!(place(game, Side::Left, 400, GRIP, &screens), (-300, 40));
        // 200 of 400 pixels show: left as it is.
        let game = rect(200, 40, 1300, 1000);
        assert_eq!(place(game, Side::Left, 400, GRIP, &screens), (-200, 40));
        let game = rect(1300, 40, 2500, 1000);
        assert_eq!(place(game, Side::Right, 500, GRIP, &screens), (2460, 40));
    }

    #[test]
    fn the_title_bar_stays_on_a_screen() {
        let screens = [rect(0, 0, 2560, 1392)];
        let game = rect(600, -100, 1900, 900);
        assert_eq!(place(game, Side::Left, 400, GRIP, &screens), (200, 0));
        let game = rect(600, 1380, 1900, 2000);
        assert_eq!(place(game, Side::Right, 400, GRIP, &screens), (1900, 1362));
    }

    #[test]
    fn the_nearest_screen_is_chosen() {
        // Below the shorter left screen: moving up onto it (by 98) is nearer than moving
        // across onto the middle screen (by 100).
        let game = rect(0, 1100, 1300, 1300);
        assert_eq!(place(game, Side::Left, 400, GRIP, &screens()), (-400, 1002));
        let game = rect(0, 1150, 1300, 1300);
        assert_eq!(place(game, Side::Left, 400, GRIP, &screens()), (-300, 1150));
    }

    #[test]
    fn with_no_screen_the_docked_spot_stays() {
        let game = rect(600, 40, 1900, 1000);
        assert_eq!(place(game, Side::Left, 400, GRIP, &[]), (200, 40));
    }
}
