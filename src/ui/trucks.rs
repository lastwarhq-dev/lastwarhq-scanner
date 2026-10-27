//! What the trucks window shows, built from the shared state. Kept apart from the window so it
//! can be tested without one.

use std::time::Duration;

use crate::game::tasks::{DEFAULT_SIZE, World};
use crate::game::trucks::{Kind, Truck, Trucks};
use crate::ui::tasks::{ScannedRun, scanned_runs, wait};

/// The power slider's range and step. At its top it lets every truck through.
pub const POWER_MAX: i64 = 100_000_000;
pub const POWER_STEP: i64 = 1_000_000;
/// Where the power slider starts.
pub const DEFAULT_POWER: i64 = 40_000_000;

/// Which trucks the window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Filter {
    /// Only reindeer trucks.
    pub reindeer_only: bool,
    /// Only trucks defended by this much power at most; [`POWER_MAX`] lets every one through.
    pub max_power: i64,
}

impl Default for Filter {
    fn default() -> Filter {
        Filter {
            reindeer_only: true,
            max_power: DEFAULT_POWER,
        }
    }
}

impl Filter {
    fn lets_through(&self, truck: &Truck) -> bool {
        (!self.reindeer_only || truck.kind == Kind::Reindeer)
            && (self.max_power >= POWER_MAX || truck.power <= self.max_power)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrucksScreen {
    /// The server being viewed and how many of its cities are placed, or what to do to see one.
    pub title: String,
    pub filter: Filter,
    /// The map's side, in squares.
    pub size: u32,
    /// The part of the map loaded last: left, bottom, right, top.
    pub area: Option<(u32, u32, u32, u32)>,
    /// What has been scanned since the daily reset, as on the secret tasks map.
    pub scanned: Vec<ScannedRun>,
    /// The cities placed so far, which trucks drive between.
    pub cities: Vec<(u32, u32)>,
    pub markers: Vec<TruckMarker>,
    /// The selected truck's way ahead: where it is now, then each city placed so far.
    pub route: Vec<(f32, f32)>,
    pub summary: String,
    pub lines: Vec<TruckLine>,
}

/// A truck on the map, where it is now, in squares.
#[derive(Debug, Clone, PartialEq)]
pub struct TruckMarker {
    pub x: f32,
    pub y: f32,
    pub kind: Kind,
    pub selected: bool,
    /// The city it is driving to, or sets off for next from the one it is at, once placed.
    pub heading: Option<(f32, f32)>,
}

/// A truck in the list under the map.
#[derive(Debug, Clone, PartialEq)]
pub struct TruckLine {
    pub id: u64,
    pub kind: Kind,
    pub power: i64,
    /// Where it is now, to the square; `None` until the cities it is between are placed.
    pub at: Option<(u32, u32)>,
    /// Raids taken, and where it is heading.
    pub text: String,
    pub selected: bool,
}

pub fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Reindeer => "Reindeer",
        Kind::Ur => "UR",
    }
}

/// A power in millions: "38.2M".
pub fn power_text(power: i64) -> String {
    format!("{:.1}M", power as f64 / 1_000_000.0)
}

/// The slider's label: "Power ≤ 40M", or "Power: any" at the top.
pub fn power_label(max_power: i64) -> String {
    if max_power >= POWER_MAX {
        "Power: any".to_string()
    } else {
        format!("Power ≤ {}M", max_power / 1_000_000)
    }
}

/// The window's content at `now` on the game server's clock: the trucks on the server the
/// world map was last loaded for that `filter` lets through, leaving out our alliance's
/// (`ours`), reindeer first and then the weakest defended. `selected` is the truck whose route
/// is drawn. Like the secret tasks map, it shows what has been scanned and the part loaded last.
pub fn trucks_screen(
    trucks: &Trucks,
    world: &World,
    ours: Option<&str>,
    now: Duration,
    filter: Filter,
    selected: Option<u64>,
) -> TrucksScreen {
    let Some(look) = world.last_look() else {
        return TrucksScreen {
            title: "Trucks · open the world map in the game".to_string(),
            filter,
            size: DEFAULT_SIZE,
            area: None,
            scanned: Vec::new(),
            cities: Vec::new(),
            markers: Vec::new(),
            route: Vec::new(),
            summary: "Trucks load as the game sends them".to_string(),
            lines: Vec::new(),
        };
    };
    let server = look.server;
    let (scanned, percent) = world
        .scan(server)
        .map(|scan| scanned_runs(scan, now))
        .unwrap_or_default();
    // A truck whose route has ended goes at once, not at the next truck message.
    let theirs: Vec<&Truck> = trucks
        .trucks(server)
        .filter(|t| now < t.ends && (ours.is_none() || t.alliance.as_deref() != ours))
        .collect();
    let mut shown: Vec<&Truck> = theirs
        .iter()
        .copied()
        .filter(|t| filter.lets_through(t))
        .collect();
    shown.sort_by_key(|t| (t.kind, t.power, t.id));
    let chosen = shown.iter().find(|t| Some(t.id) == selected).copied();

    let lines: Vec<TruckLine> = shown
        .iter()
        .map(|t| {
            let here = trucks.position(t, now);
            TruckLine {
                id: t.id,
                kind: t.kind,
                power: t.power,
                at: here.map(|(x, y)| (x.round() as u32, y.round() as u32)),
                text: line_text(trucks, t, here.is_some(), now),
                selected: Some(t.id) == selected,
            }
        })
        .collect();
    let markers = shown
        .iter()
        .filter_map(|t| {
            let (x, y) = trucks.position(t, now)?;
            let heading = trucks
                .stops(t)
                .into_iter()
                .find(|s| s.arrives > now)
                .and_then(|s| s.city)
                .map(|(x, y)| (x as f32, y as f32));
            Some(TruckMarker {
                x,
                y,
                kind: t.kind,
                selected: Some(t.id) == selected,
                heading,
            })
        })
        .collect();
    let route = chosen.map_or_else(Vec::new, |t| route(trucks, t, now));
    let summary = match chosen {
        Some(t) => selected_text(trucks, t, now),
        None if theirs.is_empty() => format!("{percent}% scanned · no UR or reindeer seen yet"),
        None => format!(
            "{percent}% scanned · {} shown · {} seen · click one for its route",
            shown.len(),
            theirs.len()
        ),
    };
    let cities = trucks.city_places(server);
    TrucksScreen {
        title: format!("Server {server} · {} cities placed", cities.len()),
        filter,
        size: look.size,
        area: Some(look.area),
        scanned,
        cities,
        markers,
        route,
        summary,
        lines,
    }
}

/// Where `truck` is now, then each city ahead whose place is known.
fn route(trucks: &Trucks, truck: &Truck, now: Duration) -> Vec<(f32, f32)> {
    let Some(here) = trucks.position(truck, now) else {
        return Vec::new();
    };
    let ahead = trucks
        .stops(truck)
        .into_iter()
        .filter(|s| s.arrives > now)
        .map_while(|s| s.city)
        .map(|(x, y)| (x as f32, y as f32));
    std::iter::once(here).chain(ahead).collect()
}

/// The raids taken, and where the truck is heading: "0/1 raids · city in 2m".
fn line_text(trucks: &Trucks, truck: &Truck, placed: bool, now: Duration) -> String {
    let raids = format!("{}/{} raids", truck.raids, truck.kind.max_raids());
    let stops = trucks.stops(truck);
    let heading = if !placed {
        "not placed".to_string()
    } else if stops.iter().any(|s| s.arrives <= now && now < s.leaves) {
        "at a city".to_string()
    } else {
        match stops.iter().find(|s| s.arrives > now) {
            Some(next) => format!("city in {}", wait(next.arrives - now)),
            None => "at its last city".to_string(),
        }
    };
    format!("{raids} · {heading}")
}

/// The selected truck's next city and when its route ends: "Next city X 130 Y 240 in 3m ·
/// ends in 1h 05m".
fn selected_text(trucks: &Trucks, truck: &Truck, now: Duration) -> String {
    let ends = format!("ends in {}", wait(truck.ends.saturating_sub(now)));
    let next = trucks.stops(truck).into_iter().find(|s| s.arrives > now);
    match next {
        Some(stop) => match stop.city {
            Some((x, y)) => format!(
                "Next city X {x} Y {y} in {} · {ends}",
                wait(stop.arrives - now)
            ),
            None => format!("Next city not placed yet · {ends}"),
        },
        None => format!("At its last city · {ends}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::trucks::STOP;
    use crate::game::trucks::tests::{OTHERS, Run, SERVER, T0, mapped, march};

    const OURS: &str = "fedcba9876543210fedcba9876543210";
    const S: u32 = SERVER as u32;

    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    /// A UR at 20M, a reindeer at 30M, a reindeer at 50M and our own reindeer, all at the
    /// start of the test route's first leg.
    fn trucks() -> Trucks {
        let mut trucks = mapped();
        let runs = [
            Run {
                cfg_id: 5033,
                id: 1,
                power: 20_000_000,
                ..Run::default()
            },
            Run {
                id: 2,
                power: 30_000_000,
                ..Run::default()
            },
            Run {
                id: 3,
                power: 50_000_000,
                ..Run::default()
            },
            Run {
                id: 4,
                power: 10_000_000,
                alliance: OURS,
                ..Run::default()
            },
        ];
        for run in &runs {
            trucks.apply("push.world.march.new", &march(run), at(T0));
        }
        trucks
    }

    fn ids(screen: &TrucksScreen) -> Vec<u64> {
        screen.lines.iter().map(|l| l.id).collect()
    }

    /// The world map loaded for the test server at the closest zoom, around (400..999,
    /// 900..999): 6% of the map.
    fn viewing() -> World {
        let mut world = World::default();
        let block = crate::game::tasks::tests::block(SERVER, 0, Vec::new());
        world.apply("world.get.block", &block, at(T0));
        world
    }

    #[test]
    fn the_map_shows_the_scan_the_frame_and_the_cities() {
        let screen = trucks_screen(&trucks(), &viewing(), None, at(T0), Filter::default(), None);
        assert_eq!(screen.area, Some((400, 900, 999, 999)));
        assert_eq!(screen.scanned.len(), 10);
        assert_eq!(
            screen.cities,
            [(100, 100), (130, 140), (130, 240), (230, 240)]
        );
    }

    #[test]
    fn reindeer_up_to_40m_show_at_first() {
        let screen = trucks_screen(
            &trucks(),
            &viewing(),
            Some(OURS),
            at(T0),
            Filter::default(),
            None,
        );
        assert_eq!(ids(&screen), [2]);
        assert_eq!(screen.title, format!("Server {S} · 4 cities placed"));
        assert_eq!(
            screen.summary,
            "6% scanned · 1 shown · 3 seen · click one for its route"
        );
        let line = &screen.lines[0];
        assert_eq!((line.kind, line.at), (Kind::Reindeer, Some((100, 100))));
        assert_eq!(line.text, "0/1 raids · city in 1m");
        assert_eq!(screen.markers.len(), 1);
        // On its way to the first city of its route.
        assert_eq!(screen.markers[0].heading, Some((130.0, 140.0)));
    }

    #[test]
    fn the_filters_widen_and_our_own_trucks_never_show() {
        let all = Filter {
            reindeer_only: false,
            max_power: POWER_MAX,
        };
        let screen = trucks_screen(&trucks(), &viewing(), Some(OURS), at(T0), all, None);
        // Reindeer first, then the weakest.
        assert_eq!(ids(&screen), [2, 3, 1]);
        // Until our alliance is known, nothing is left out.
        let screen = trucks_screen(&trucks(), &viewing(), None, at(T0), all, None);
        assert_eq!(ids(&screen), [4, 2, 3, 1]);
        let ur_too = Filter {
            reindeer_only: false,
            max_power: 25_000_000,
        };
        let screen = trucks_screen(&trucks(), &viewing(), Some(OURS), at(T0), ur_too, None);
        assert_eq!(ids(&screen), [1]);
    }

    #[test]
    fn the_selected_truck_shows_its_route_and_timer() {
        let trucks = trucks();
        let screen = trucks_screen(
            &trucks,
            &viewing(),
            Some(OURS),
            at(T0),
            Filter::default(),
            Some(2),
        );
        // From where it is through the three cities ahead.
        assert_eq!(
            screen.route,
            [
                (100.0, 100.0),
                (130.0, 140.0),
                (130.0, 240.0),
                (230.0, 240.0)
            ]
        );
        assert_eq!(
            screen.summary,
            "Next city X 130 Y 140 in 1m · ends in 1h 00m"
        );
        assert!(screen.lines[0].selected && screen.markers[0].selected);
        // Stopped at the first city: the route goes on from there.
        let stopped = at(T0 + 102_000);
        let screen = trucks_screen(
            &trucks,
            &viewing(),
            Some(OURS),
            stopped,
            Filter::default(),
            Some(2),
        );
        assert_eq!(screen.route[0], (130.0, 140.0));
        assert_eq!(screen.route.len(), 3);
        assert_eq!(screen.lines[0].text, "0/1 raids · at a city");
        let leaving = T0 + 100_000 + STOP.as_millis() as u64;
        let screen = trucks_screen(
            &trucks,
            &viewing(),
            Some(OURS),
            at(leaving),
            Filter::default(),
            Some(2),
        );
        assert_eq!(screen.lines[0].text, "0/1 raids · city in 3m");
    }

    #[test]
    fn without_a_map_or_trucks_it_says_what_to_do() {
        let screen = trucks_screen(
            &Trucks::default(),
            &World::default(),
            None,
            at(T0),
            Filter::default(),
            None,
        );
        assert_eq!(screen.title, "Trucks · open the world map in the game");
        let screen = trucks_screen(
            &Trucks::default(),
            &viewing(),
            None,
            at(T0),
            Filter::default(),
            None,
        );
        assert_eq!(screen.summary, "6% scanned · no UR or reindeer seen yet");
        assert!(screen.lines.is_empty());
    }

    #[test]
    fn a_truck_between_unplaced_cities_is_listed_without_a_place() {
        let mut trucks = Trucks::default();
        let run = Run {
            alliance: OTHERS,
            ..Run::default()
        };
        // Its tap reply alone: no protobuf, so no cities.
        let refresh = crate::protocol::sfs::Value::Object(vec![(
            "train".into(),
            crate::game::trucks::tests::train(&run),
        )]);
        trucks.apply("train.refresh", &refresh, at(T0));
        let screen = trucks_screen(&trucks, &viewing(), None, at(T0), Filter::default(), None);
        assert_eq!(screen.lines[0].at, None);
        assert_eq!(screen.lines[0].text, "0/1 raids · not placed");
        assert!(screen.markers.is_empty());
    }

    #[test]
    fn power_reads_in_millions() {
        assert_eq!(power_text(38_179_728), "38.2M");
        assert_eq!(power_label(DEFAULT_POWER), "Power ≤ 40M");
        assert_eq!(power_label(POWER_MAX), "Power: any");
    }
}
