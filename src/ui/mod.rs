//! The status window: `window` is the Win32 window itself, `status` turns the shared state
//! into what it shows, and `paint` draws that. `log` is the debug log window, and
//! `tasks_window` the secret tasks map, with its content from `tasks`; both open from it.

pub mod log;
pub mod paint;
pub mod status;
pub mod tasks;
pub mod tasks_window;
pub mod window;
