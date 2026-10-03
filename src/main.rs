// A window app: no console.
#![windows_subsystem = "windows"]

fn main() {
    if let Err(err) = lastwarhq_scanner::app::run() {
        lastwarhq_scanner::ui::window::error_box(&err);
    }
}
