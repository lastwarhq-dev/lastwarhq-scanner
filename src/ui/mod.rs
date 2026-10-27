//! The status window: `window` is the Win32 window itself, `status` turns the shared state
//! into what it shows, and `paint` draws that. `log` is the debug log window, and
//! `tasks_window` the secret tasks map, with its content from `tasks`, and `trucks_window` the
//! trucks map, with its content from `trucks`; all three open from it. `dock` keeps the status
//! window and the maps docked beside the game window.

pub mod dock;
pub mod log;
pub mod paint;
pub mod status;
pub mod tasks;
pub mod tasks_window;
pub mod trucks;
pub mod trucks_window;
pub mod window;
