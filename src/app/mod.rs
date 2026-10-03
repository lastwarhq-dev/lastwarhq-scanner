//! Ties the parts together: live capture on a background thread, the status window in front,
//! and Load mail on request.

pub mod export;
pub mod state;

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::capture::Packet;
use crate::capture::adapters::{self, Adapter};
use crate::capture::npcap::{self, Capture};
use crate::capture::pipeline::{GAME_PORTS, Pipeline};
use crate::mail::database as mail;
use crate::ui::window;
use crate::util::time;
use state::State;

/// Packets waiting between the adapter readers and the decoder. When full, readers wait, and
/// anything Npcap drops meanwhile is handled as a stream gap. Packets are captured whole (up
/// to 64 KiB with segmentation offload), so the queue holds at most 64 MiB; ordinary packets
/// of up to 1,514 bytes make that about 1.5 MB.
const PACKET_QUEUE: usize = 1_024;

pub fn run() -> Result<(), String> {
    if window::already_running() {
        return Err("LastWarHQ Scanner is already running.".into());
    }
    let state = Arc::new(Mutex::new(State::default()));
    {
        let state = Arc::clone(&state);
        thread::spawn(move || {
            let result = panic::catch_unwind(AssertUnwindSafe(|| capture(&state)))
                .unwrap_or_else(|p| Err(format!("internal error: {}", panic_text(p.as_ref()))));
            let mut s = lock(&state);
            s.capture.running = false;
            s.capture.error = Some(match result {
                Ok(()) => "capture ended".into(),
                Err(err) => err,
            });
        });
    }
    window::run(state)
}

/// The message of a caught panic.
pub fn panic_text(panic: &(dyn Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown error".into())
}

/// How often the adapter list is checked for newly connected adapters.
const ADAPTER_CHECK: Duration = Duration::from_secs(10);

/// What the per-adapter reader threads send to the capture loop.
enum Event {
    Packet(Packet),
    /// The adapter's capture failed (e.g. the adapter was disabled); it may be reopened later.
    Closed {
        device: String,
        error: String,
    },
}

/// Captures on every usable adapter and feeds each message into the shared state. Runs until
/// the process ends; returns only if Npcap is missing.
fn capture(state: &Mutex<State>) -> Result<(), String> {
    npcap::available()?;
    let filter = format!("tcp portrange {}-{}", GAME_PORTS.start(), GAME_PORTS.end());
    let (tx, rx) = mpsc::sync_channel(PACKET_QUEUE);
    // Open adapters by device name â†’ display name.
    let mut open: HashMap<String, String> = HashMap::new();
    // Devices seen usable at the last check. Only devices new since then are opened, so an
    // adapter that failed to open (say, the admin prompt was declined) is not retried, and
    // re-prompted, every 10 s.
    let mut known: HashSet<String> = HashSet::new();
    let mut last_error: Option<String> = None;
    let mut next_check = Instant::now();

    let mut pipeline = Pipeline::new(GAME_PORTS);
    let mut messages = Vec::new();
    let mut last_link = None;
    loop {
        if Instant::now() >= next_check {
            next_check = Instant::now() + ADAPTER_CHECK;
            match adapters::usable_adapters() {
                Ok(list) => {
                    let new: Vec<&Adapter> = list
                        .iter()
                        .filter(|a| !known.contains(&a.device) && !open.contains_key(&a.device))
                        .collect();
                    for adapter in new {
                        match start_reader(adapter, &filter, tx.clone()) {
                            Ok(()) => {
                                open.insert(adapter.device.clone(), adapter.name.clone());
                            }
                            Err(err) => last_error = Some(err),
                        }
                    }
                    known = list.into_iter().map(|a| a.device).collect();
                }
                Err(err) => last_error = Some(err),
            }
            let mut s = lock(state);
            let mut names: Vec<String> = open.values().cloned().collect();
            names.sort();
            s.capture.adapters = names;
            s.capture.running = !open.is_empty();
            s.capture.error = if open.is_empty() {
                Some(
                    last_error
                        .clone()
                        .unwrap_or_else(|| "no connected network adapter to capture on".into()),
                )
            } else {
                None
            };
        }

        let packet = match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::Packet(packet)) => packet,
            Ok(Event::Closed { device, error }) => {
                open.remove(&device);
                known.remove(&device);
                last_error = Some(error);
                next_check = Instant::now();
                continue;
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => unreachable!("the loop holds a sender"),
        };
        pipeline.push(&packet, &mut messages);
        let link = pipeline.link();
        if messages.is_empty() && link == last_link {
            continue;
        }
        last_link = link;
        let mut s = lock(state);
        s.capture.server = link.map(|l| l.server.to_string());
        s.capture.last_heartbeat = link.and_then(|l| l.last_heartbeat);
        for m in messages.drain(..) {
            s.record(&m);
        }
    }
}

/// Opens an adapter and reads it on its own thread until it fails. The first open in the
/// process shows the Npcap admin prompt when Npcap is restricted to administrators.
fn start_reader(adapter: &Adapter, filter: &str, tx: SyncSender<Event>) -> Result<(), String> {
    let mut capture = Capture::open(&adapter.device, filter)?;
    let device = adapter.device.clone();
    thread::spawn(move || {
        loop {
            match capture.read_packet() {
                Ok(Some(packet)) => {
                    if tx.send(Event::Packet(packet)).is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = tx.send(Event::Closed { device, error });
                    return;
                }
            }
        }
    });
    Ok(())
}

/// Copies the game's mail database into memory, reads this week's Desert Storm results from
/// the copy, and drops the copy. The file itself is only read. The result is applied as of
/// when reading finished, and only if the account, alliance and week are still those it
/// started with. Always returns a short summary, even if reading fails unexpectedly, so the
/// window can re-enable its button.
pub fn load_mail(state: &Mutex<State>) -> String {
    let started = lock(state).start_mail_load(time::now());
    let read = || {
        let path = mail::db_path().ok_or_else(|| "USERPROFILE is not set".to_string())?;
        if !path.is_file() {
            return Err(format!("mail database not found at {}", path.display()));
        }
        let bytes = mail::read_snapshot(&path)?;
        mail::ds_battles(&bytes)
    };
    let result = panic::catch_unwind(read)
        .unwrap_or_else(|p| Err(format!("internal error: {}", panic_text(p.as_ref()))));
    lock(state).finish_mail_load(&started, result, time::now())
}

/// Locks the shared state, also after another thread panicked while holding it.
fn lock(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}
