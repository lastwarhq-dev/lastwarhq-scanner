//! What the secret tasks window shows, built from the shared state. Kept apart from the
//! window so it can be tested without one.

use std::time::Duration;

use crate::game::tasks::{MAX_LOOTS, Rarity, Scan, ShinyDay, Task, World};
use crate::ui::status::Mark;

/// Task lines shown under the map at once; the list scrolls to the rest.
pub const MAX_LINES: usize = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct TasksScreen {
    /// The server being viewed, or what to do to see one.
    pub title: String,
    /// The server being viewed has its shiny day today, so the title stands out.
    pub shiny_today: bool,
    /// Whether the game's zoom lets secret tasks through: green, red if zoomed out too far.
    pub zoom: Mark,
    pub zoom_text: String,
    /// The map's side, in squares.
    pub size: u32,
    /// The part of the map loaded last: left, bottom, right, top.
    pub area: Option<(u32, u32, u32, u32)>,
    /// What has been scanned since the daily reset, as runs of cells along each row.
    pub scanned: Vec<ScannedRun>,
    pub markers: Vec<Marker>,
    pub shiny_only: bool,
    pub summary: String,
    pub lines: Vec<TaskLine>,
}

/// Cells next to each other in a row of the map, scanned since the daily reset. Cells are
/// [`CELL`](crate::game::tasks::CELL) squares across; rows count up from the bottom.
#[derive(Debug, Clone, PartialEq)]
pub struct ScannedRun {
    pub row: u32,
    /// The first cell, and the one after the last.
    pub from: u32,
    pub to: u32,
    /// Scanned within [`FRESH`]; new tasks appear all the time, so older scans fade.
    pub fresh: bool,
}

/// How long a scan counts as fresh.
pub const FRESH: Duration = Duration::from_secs(30 * 60);

/// A task on the map.
#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    pub x: u32,
    pub y: u32,
    pub rarity: Rarity,
    /// Finished, so it can be looted; otherwise still in progress.
    pub ready: bool,
}

/// A task in the list under the map.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskLine {
    pub rarity: Rarity,
    pub x: u32,
    pub y: u32,
    /// Loots taken, and whether it can be looted yet.
    pub text: String,
}

/// "shiny today", "shiny tomorrow" or "shiny in 2 days".
pub fn shiny_text(day: ShinyDay) -> &'static str {
    match day {
        ShinyDay::Today => "shiny today",
        ShinyDay::Tomorrow => "shiny tomorrow",
        ShinyDay::InTwoDays => "shiny in 2 days",
    }
}

pub fn rarity_name(rarity: Rarity) -> &'static str {
    match rarity {
        Rarity::Shiny => "Shiny",
        Rarity::Ur => "UR",
        Rarity::Ssr => "SSR",
        Rarity::Sr => "SR",
        Rarity::Other => "Other",
    }
}

/// The window's content at `now` on the game server's clock: the tasks with loots left on
/// the server looked at last, only shiny ones if `shiny_only`.
pub fn tasks_screen(world: &World, now: Duration, shiny_only: bool) -> TasksScreen {
    let look = world.last_look();
    let (zoom, zoom_text) = match look {
        None => (Mark::Pending, "Open the world map in the game".to_string()),
        Some(l) if l.shows_tasks() => (
            Mark::Done,
            "Zoom level OK · secret tasks come through as you scroll".to_string(),
        ),
        Some(_) => (
            Mark::Failed,
            "Zoomed out too far · zoom in until secret tasks come through".to_string(),
        ),
    };
    let mut tasks: Vec<&Task> = look
        .map(|l| world.tasks(l.server).collect())
        .unwrap_or_default();
    let seen = tasks.len();
    tasks.retain(|t| t.open(now) && (!shiny_only || t.rarity() == Rarity::Shiny));
    tasks.sort_by_key(|t| (t.rarity(), !t.lootable(now), t.ready, t.y, t.x));

    let (scanned, percent) = look
        .and_then(|l| world.scan(l.server))
        .map(|scan| scanned_runs(scan, now))
        .unwrap_or_default();
    let ready = tasks.iter().filter(|t| t.lootable(now)).count();
    let waiting = tasks.len() - ready;
    let counts = match (ready, waiting) {
        (0, 0) if shiny_only => "no shiny with loots left".to_string(),
        (0, 0) => "none with loots left".to_string(),
        _ => format!("{ready} to loot · {waiting} in progress"),
    };
    let shiny = look.and_then(|l| world.shiny_day(l.server, now));
    TasksScreen {
        title: match (look, shiny) {
            (None, _) => "Secret tasks".to_string(),
            (Some(l), None) => format!("Server {}", l.server),
            (Some(l), Some(day)) => format!("Server {} · {}", l.server, shiny_text(day)),
        },
        shiny_today: shiny == Some(ShinyDay::Today),
        zoom,
        zoom_text,
        size: look.map_or(1000, |l| l.size),
        area: look.map(|l| l.area),
        scanned,
        summary: format!("{percent}% scanned · {seen} seen · {counts}"),
        markers: tasks
            .iter()
            .map(|t| Marker {
                x: t.x,
                y: t.y,
                rarity: t.rarity(),
                ready: t.lootable(now),
            })
            .collect(),
        shiny_only,
        lines: tasks
            .iter()
            .map(|t| TaskLine {
                rarity: t.rarity(),
                x: t.x,
                y: t.y,
                text: line_text(t, now),
            })
            .collect(),
    }
}

/// The cells of `scan` loaded since the last daily reset (02:00 UTC) before `now`, as runs
/// of fresh or older cells, and how much of the map they cover, in whole percent.
fn scanned_runs(scan: &Scan, now: Duration) -> (Vec<ScannedRun>, u32) {
    let secs = now.as_secs();
    let reset = secs - (secs + 86_400 - 7_200) % 86_400;
    let state = |loaded: u32| {
        let loaded = u64::from(loaded);
        (loaded != 0 && loaded >= reset).then(|| secs.saturating_sub(loaded) <= FRESH.as_secs())
    };
    let mut runs: Vec<ScannedRun> = Vec::new();
    let mut covered = 0;
    for row in 0..scan.across {
        for col in 0..scan.across {
            let Some(fresh) = state(scan.loaded[(row * scan.across + col) as usize]) else {
                continue;
            };
            covered += 1;
            match runs.last_mut() {
                Some(run) if run.row == row && run.to == col && run.fresh == fresh => run.to += 1,
                _ => runs.push(ScannedRun {
                    row,
                    from: col,
                    to: col + 1,
                    fresh,
                }),
            }
        }
    }
    let total = u64::from(scan.across) * u64::from(scan.across);
    (runs, (covered * 100 / total.max(1)) as u32)
}

/// "1/3 looted · ready" or "0/3 looted · ready in 1h 05m".
fn line_text(task: &Task, now: Duration) -> String {
    let state = if task.lootable(now) {
        "ready".to_string()
    } else {
        format!("ready in {}", wait(task.ready.saturating_sub(now)))
    };
    format!("{}/{MAX_LOOTS} looted · {state}", task.loots)
}

/// A wait as "45s", "12m" or "1h 05m".
fn wait(time: Duration) -> String {
    let secs = time.as_secs();
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h {:02}m", secs / 3600, secs / 60 % 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::tasks::tests::{block, task_point};
    use crate::protocol::sfs::Value;

    /// Wednesday 2026-10-21 23:00 UTC, in milliseconds.
    const NOW: u64 = 1_792_623_600_000;
    const HOUR: u64 = 3_600_000;

    fn world(view_level: i32) -> World {
        let points = vec![
            // Shiny, still in progress for an hour.
            task_point(250, 750, 60_000_601, 0, NOW + HOUR, NOW + 51 * HOUR),
            // Shiny, ready, one loot taken.
            task_point(120, 300, 60_000_501, 1, NOW - HOUR, NOW + 51 * HOUR),
            // Shiny, looted in full.
            task_point(130, 300, 60_000_501, 3, NOW - HOUR, NOW + 51 * HOUR),
            // A purple, ready.
            task_point(700, 800, 400_502, 0, NOW - HOUR, NOW + 3 * HOUR),
        ];
        let mut world = World::default();
        world.apply(
            "world.get.block",
            &block(901, view_level, points),
            Duration::from_millis(NOW),
        );
        world
    }

    fn now() -> Duration {
        Duration::from_millis(NOW)
    }

    #[test]
    fn shiny_tasks_with_loots_left_are_shown_ready_first() {
        let screen = tasks_screen(&world(0), now(), true);
        assert_eq!(screen.title, "Server 901");
        assert_eq!(screen.zoom, Mark::Done);
        assert_eq!(screen.area, Some((400, 900, 999, 999)));
        assert_eq!(
            screen.summary,
            "6% scanned · 4 seen · 1 to loot · 1 in progress"
        );
        let lines: Vec<(u32, &str)> = screen
            .lines
            .iter()
            .map(|l| (l.x, l.text.as_str()))
            .collect();
        assert_eq!(
            lines,
            [
                (120, "1/3 looted · ready"),
                (250, "0/3 looted · ready in 1h 00m"),
            ]
        );
        assert_eq!(
            screen.markers.iter().map(|m| m.ready).collect::<Vec<_>>(),
            [true, false]
        );
    }

    #[test]
    fn every_rarity_shows_when_asked() {
        let screen = tasks_screen(&world(0), now(), false);
        assert_eq!(
            screen.summary,
            "6% scanned · 4 seen · 2 to loot · 1 in progress"
        );
        assert_eq!(screen.lines.last().map(|l| l.rarity), Some(Rarity::Ssr));
        assert_eq!(
            screen.lines.last().map(|l| l.text.as_str()),
            Some("0/3 looted · ready")
        );
    }

    #[test]
    fn zooming_out_too_far_shows_red() {
        let screen = tasks_screen(&world(1), now(), true);
        assert_eq!(screen.zoom, Mark::Failed);
        assert!(screen.zoom_text.starts_with("Zoomed out too far"));

        let nothing = tasks_screen(&World::default(), now(), true);
        assert_eq!(nothing.zoom, Mark::Pending);
        assert_eq!(nothing.title, "Secret tasks");
        assert_eq!(
            nothing.summary,
            "0% scanned · 0 seen · no shiny with loots left"
        );
    }

    #[test]
    fn the_title_says_where_the_server_is_in_its_shiny_cycle() {
        let mut world = world(0);
        assert_eq!(tasks_screen(&world, now(), false).title, "Server 901");
        // Opened 2 days before today in server time: shiny tomorrow.
        let info = |opened: u64| {
            Value::Object(vec![
                ("server".into(), Value::Int(901)),
                ("openTime".into(), Value::Long(opened as i64)),
            ])
        };
        world.apply("get.other.server.info", &info(NOW - 50 * HOUR), now());
        let screen = tasks_screen(&world, now(), false);
        assert_eq!(screen.title, "Server 901 · shiny tomorrow");
        assert!(!screen.shiny_today);
        world.apply("get.other.server.info", &info(NOW - 3 * 24 * HOUR), now());
        let screen = tasks_screen(&world, now(), false);
        assert_eq!(screen.title, "Server 901 · shiny today");
        assert!(screen.shiny_today);
    }

    #[test]
    fn waits_are_short() {
        let w = |secs| wait(Duration::from_secs(secs));
        assert_eq!(
            (w(45), w(12 * 60 + 30), w(3900)),
            ("45s".into(), "12m".into(), "1h 05m".into())
        );
    }

    #[test]
    fn scans_fade_and_start_again_at_the_reset() {
        // One row of 4 cells: loaded just now, 31 minutes ago, never, and before the 02:00
        // reset (NOW is 23:00, so the reset was 21 hours ago).
        let secs = (NOW / 1000) as u32;
        let scan = Scan {
            across: 2,
            loaded: vec![secs, secs - 31 * 60, 0, secs - 22 * 3600],
        };
        let (runs, percent) = scanned_runs(&scan, now());
        assert_eq!(
            runs,
            [
                ScannedRun {
                    row: 0,
                    from: 0,
                    to: 1,
                    fresh: true
                },
                ScannedRun {
                    row: 0,
                    from: 1,
                    to: 2,
                    fresh: false
                },
            ]
        );
        assert_eq!(percent, 50);

        // A whole loaded row is one run.
        let screen = tasks_screen(&world(0), now(), false);
        assert_eq!(screen.scanned.len(), 10);
        assert_eq!(
            screen.scanned[0],
            ScannedRun {
                row: 90,
                from: 40,
                to: 100,
                fresh: true
            }
        );
    }
}
