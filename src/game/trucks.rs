//! Trucks on the world map: the UR and reindeer trucks that drive from city to city, where they
//! are, where they go next and when, per server. Only what finding and judging a truck needs is
//! kept: its kind, defending power, raids taken, alliance (to leave out our own), route and
//! times. Who owns it is not kept.
//!
//! A truck is a march (`mt` 14) carrying a `train`: `push.world.march.new` when it reaches a city
//! or sets off from one, `push.world.march.world.get.new` for the trucks in the part of the map
//! the game loads, and `train.refresh` when one is tapped. `stationList` is its route, city by
//! city, and `lastPosIndex` the city it is driving to or stopped at. It drives in a straight
//! line at a steady speed and stops at each city for a few seconds.
//!
//! A march's protobuf names the two cities of its leg, which is how cities' places are learnt:
//! a station id is the same place for every truck. Every truck's march teaches them, whatever
//! its kind.

use std::collections::HashMap;
use std::time::Duration;

use crate::game::tasks::{DEFAULT_SIZE, millis, point_bytes, position};
use crate::protocol::proto::{self, Field};
use crate::protocol::sfs::Value;

/// The march type of a truck.
const TRUCK_MARCH: i64 = 14;

/// How long a truck stops at a city before setting off again, as seen (5.9 to 7.4 seconds).
pub const STOP: Duration = Duration::from_millis(5_920);

/// A truck's speed in squares a second without buffs, and the buff that raises it.
const BASE_SPEED: f64 = 0.25;
const SPEED_BUFF: &str = "90109";

/// Trucks kept at most, across servers.
const MAX_TRUCKS: usize = 5_000;
/// Cities kept at most, across servers.
const MAX_CITIES: usize = 20_000;

/// The kinds of truck worth raiding, from `cfgId`. In order of interest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Reindeer,
    Ur,
}

/// The kind of truck `cfg_id`: 234 a reindeer; 140 to 150 UR for HQ 20 to 30, and 5031 to 5035 for
/// HQ 31 to 35. Other trucks aren't tracked.
pub fn kind(cfg_id: i64) -> Option<Kind> {
    match cfg_id {
        234 => Some(Kind::Reindeer),
        140..=150 | 5031..=5035 => Some(Kind::Ur),
        _ => None,
    }
}

impl Kind {
    /// How many times a truck of this kind can be raided.
    pub fn max_raids(self) -> u32 {
        match self {
            Kind::Reindeer => 1,
            Kind::Ur => 2,
        }
    }
}

/// A truck as last seen. Times are on the game server's clock.
#[derive(Debug, Clone, PartialEq)]
pub struct Truck {
    /// The truck's `uuid` (its march has another).
    pub id: u64,
    pub server: u32,
    pub kind: Kind,
    /// The defending squad's power, buffs included.
    pub power: i64,
    /// How many times it has been raided.
    pub raids: u32,
    /// The owner's alliance id; `None` without one.
    pub alliance: Option<String>,
    /// The route: station ids, in order.
    stations: Vec<u32>,
    /// Index in `stations` of the city it is driving to or stopped at.
    index: usize,
    /// The point it set off from, before the first city.
    start: u64,
    /// When it set off on the leg to `stations[index]`.
    left: Duration,
    /// When it reaches `stations[index]`, and when it leaves it again.
    arrives: Duration,
    leaves: Duration,
    /// Squares a second.
    speed: f64,
    /// When its route ends.
    pub ends: Duration,
}

/// A city on a truck's route ahead, with when the truck gets there and sets off again.
#[derive(Debug, Clone, PartialEq)]
pub struct Stop {
    /// Where the city is: `None` until its place has been learnt.
    pub city: Option<(u32, u32)>,
    pub arrives: Duration,
    pub leaves: Duration,
}

impl Truck {
    /// Whether it can still be raided.
    pub fn raidable(&self) -> bool {
        self.raids < self.kind.max_raids()
    }

    /// How far along its route the message was: a later leg, or a later start of the same one.
    fn progress(&self) -> (usize, Duration) {
        (self.index, self.left)
    }
}

/// Trucks per server, and where the cities are.
#[derive(Debug, Default)]
pub struct Trucks {
    trucks: HashMap<u64, Truck>,
    /// Server → station id → point id.
    cities: HashMap<u32, HashMap<u32, u64>>,
}

impl Trucks {
    /// Takes in a message at `time` (on the game server's clock). Returns whether anything
    /// changed.
    pub fn apply(&mut self, command: &str, data: &Value, time: Duration) -> bool {
        let changed = match command {
            "push.world.march.new" => self.apply_march(data),
            "push.world.march.world.get.new" => {
                let mut changed = false;
                for server in array(data, "serverMarchArr") {
                    for march in array(server, "marchInfos") {
                        changed |= self.apply_march(march);
                    }
                }
                changed
            }
            // The reply to tapping a truck: the whole truck, or just its uuid and version when
            // the game has it already.
            "train.refresh" => data
                .get("train")
                .is_some_and(|train| self.apply_train(train, None)),
            _ => return false,
        };
        self.prune(time);
        changed
    }

    /// The trucks seen on `server`.
    pub fn trucks(&self, server: u32) -> impl Iterator<Item = &Truck> {
        self.trucks.values().filter(move |t| t.server == server)
    }

    /// How many cities' places are known on `server`.
    pub fn cities(&self, server: u32) -> usize {
        self.cities.get(&server).map_or(0, HashMap::len)
    }

    /// Where the cities placed so far on `server` are, in order along the map's rows.
    pub fn city_places(&self, server: u32) -> Vec<(u32, u32)> {
        let mut places: Vec<(u32, u32)> = self
            .cities
            .get(&server)
            .into_iter()
            .flat_map(|cities| cities.values())
            .filter_map(|&point| position(point, DEFAULT_SIZE))
            .collect();
        places.sort_by_key(|&(x, y)| (y, x));
        places
    }

    /// The cities on `truck`'s route from the one it is driving to or stopped at, as far as
    /// their places are known: each next arrival needs the city before.
    pub fn stops(&self, truck: &Truck) -> Vec<Stop> {
        let place = |station: u32| self.city(truck.server, station);
        let mut stops = vec![Stop {
            city: place(truck.stations[truck.index]),
            arrives: truck.arrives,
            leaves: truck.leaves,
        }];
        for &station in &truck.stations[truck.index + 1..] {
            let last = stops.last().expect("one stop at least");
            let (Some(from), Some(to)) = (last.city, place(station)) else {
                break;
            };
            let arrives = last.leaves + Duration::from_secs_f64(distance(from, to) / truck.speed);
            stops.push(Stop {
                city: Some(to),
                arrives,
                leaves: arrives + STOP,
            });
        }
        stops
    }

    /// Where `truck` is at `now`, in squares: on its way between two cities, or at one. `None`
    /// when a city it is between hasn't been placed yet.
    pub fn position(&self, truck: &Truck, now: Duration) -> Option<(f32, f32)> {
        if now < truck.arrives {
            let from = match truck.index {
                0 => position(truck.start, DEFAULT_SIZE),
                i => self.city(truck.server, truck.stations[i - 1]),
            };
            let to = self.city(truck.server, truck.stations[truck.index]);
            return Some(between(from?, to?, truck.left, truck.arrives, now));
        }
        let stops = self.stops(truck);
        for (i, stop) in stops.iter().enumerate() {
            if now < stop.leaves {
                return stop.city.map(squares);
            }
            match stops.get(i + 1) {
                Some(next) if now < next.arrives => {
                    return Some(between(
                        stop.city?,
                        next.city?,
                        stop.leaves,
                        next.arrives,
                        now,
                    ));
                }
                Some(_) => {}
                // At the last city, it stays there until its route ends.
                None if truck.index + stops.len() == truck.stations.len() => {
                    return stop.city.map(squares);
                }
                None => return None,
            }
        }
        None
    }

    fn city(&self, server: u32, station: u32) -> Option<(u32, u32)> {
        let point = *self.cities.get(&server)?.get(&station)?;
        position(point, DEFAULT_SIZE)
    }

    /// A march: a truck's, with its leg's two cities, which are learnt whatever the truck.
    fn apply_march(&mut self, march: &Value) -> bool {
        if int(march, "mt") != Some(TRUCK_MARCH) {
            return false;
        }
        let Some(train) = march.get("train") else {
            return false;
        };
        let leg = march
            .get("_proto")
            .and_then(point_bytes)
            .and_then(|bytes| Leg::read(&bytes));
        let learnt = leg.as_ref().is_some_and(|leg| self.learn(train, leg));
        self.apply_train(train, leg.and_then(|l| l.speed)) || learnt
    }

    /// Places the cities at both ends of a march's leg: it ends at the city at `lastPosIndex`
    /// and starts at the one before.
    fn learn(&mut self, train: &Value, leg: &Leg) -> bool {
        let (Some(server), Some(stations), Some(index)) = (
            server(train),
            stations(train),
            train
                .get("marchInfo")
                .and_then(|i| int(i, "lastPosIndex"))
                .and_then(|i| usize::try_from(i).ok()),
        ) else {
            return false;
        };
        let ends = [
            (index.checked_sub(1).and_then(|i| stations.get(i)), leg.from),
            (stations.get(index), leg.to),
        ];
        let mut changed = false;
        for (station, point) in ends {
            let (Some(&station), Some(point)) = (station, point) else {
                continue;
            };
            if position(point, DEFAULT_SIZE).is_none() {
                continue;
            }
            let known = self.cities.values().map(HashMap::len).sum::<usize>();
            let cities = self.cities.entry(server).or_default();
            if !cities.contains_key(&station) && known >= MAX_CITIES {
                continue;
            }
            changed |= cities.insert(station, point) != Some(point);
        }
        changed
    }

    /// A `train`, kept if it is a kind worth raiding. `speed` is from the march's protobuf, when
    /// there is one; otherwise it is worked out from the speed buff.
    fn apply_train(&mut self, train: &Value, speed: Option<f64>) -> bool {
        let Some(kind) = int(train, "cfgId").and_then(kind) else {
            return false;
        };
        let Some(id) = int(train, "uuid").map(|u| u as u64) else {
            return false;
        };
        let info = train.get("marchInfo");
        if info.and_then(|i| i.get("isEnd")) == Some(&Value::Bool(true)) {
            return self.trucks.remove(&id).is_some();
        }
        let old = self.trucks.get(&id);
        let Some(truck) = read_truck(id, kind, train, speed, old.map(|t| t.speed)) else {
            return false;
        };
        // Marches can arrive out of order: one about an earlier leg changes nothing.
        if old.is_some_and(|old| old.progress() > truck.progress()) {
            return false;
        }
        if old.is_none() && self.trucks.len() >= MAX_TRUCKS {
            return false;
        }
        self.trucks.insert(id, truck.clone()) != Some(truck)
    }

    /// Drops the trucks whose route has ended, and those raided as often as they can be.
    fn prune(&mut self, now: Duration) {
        self.trucks.retain(|_, t| now < t.ends && t.raidable());
    }
}

/// A truck from its `train`, or `None` if something it needs is missing. A truck stopped at a
/// city has arrived there after setting off (`lastArriveTime` is later than `lastSendTime`),
/// and leaves at `nextEndTime`; one on its way arrives at `nextEndTime`.
fn read_truck(
    id: u64,
    kind: Kind,
    train: &Value,
    speed: Option<f64>,
    old_speed: Option<f64>,
) -> Option<Truck> {
    let info = train.get("marchInfo")?;
    let stations = stations(train)?;
    let index = usize::try_from(int(info, "lastPosIndex")?).ok()?;
    if index >= stations.len() {
        return None;
    }
    let left = millis(int(info, "lastSendTime")?);
    let arrived = millis(int(info, "lastArriveTime").unwrap_or(0));
    let next = millis(int(info, "nextEndTime")?);
    let (arrives, leaves) = if arrived > left {
        (arrived, next)
    } else {
        (next, next + STOP)
    };
    let buffed = || {
        let buff = info
            .get("effectInfo")
            .and_then(|e| number(e.get(SPEED_BUFF)?));
        BASE_SPEED * (1.0 + buff.unwrap_or(0.0))
    };
    let speed = speed
        .or(old_speed)
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or_else(buffed);
    Some(Truck {
        id,
        server: server(train)?,
        kind,
        power: int(info, "power").unwrap_or(0),
        raids: int(info, "robTimes").map_or(0, |r| r.clamp(0, i64::from(u32::MAX)) as u32),
        alliance: train
            .get("allianceId")
            .and_then(Value::as_str)
            .filter(|a| !a.is_empty())
            .map(str::to_string),
        stations,
        index,
        start: int(train, "startPos").map_or(0, |p| p.max(0) as u64),
        left,
        arrives,
        leaves,
        speed: if speed.is_finite() && speed > 0.0 {
            speed
        } else {
            BASE_SPEED
        },
        ends: millis(int(train, "arriveTime")?),
    })
}

/// A march's leg from its protobuf: 9 the point it set off from, 10 the point it is going to,
/// and 17 its speed in squares a second.
struct Leg {
    from: Option<u64>,
    to: Option<u64>,
    speed: Option<f64>,
}

impl Leg {
    fn read(bytes: &[u8]) -> Option<Leg> {
        let fields = proto::fields(bytes).ok()?;
        let point = |n| proto::first(&fields, n).and_then(|f: Field| f.as_u64());
        let speed = match proto::first(&fields, 17) {
            Some(Field::Fixed32(bits)) => Some(f64::from(f32::from_bits(bits))),
            _ => None,
        };
        Some(Leg {
            from: point(9),
            to: point(10),
            speed,
        })
    }
}

/// The point `now` of the way from `from` (left at `left`) to `to` (reached at `arrives`).
fn between(
    from: (u32, u32),
    to: (u32, u32),
    left: Duration,
    arrives: Duration,
    now: Duration,
) -> (f32, f32) {
    let total = arrives.saturating_sub(left).as_secs_f64();
    let done = if total > 0.0 {
        (now.saturating_sub(left).as_secs_f64() / total).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let along = |a: u32, b: u32| (f64::from(a) + (f64::from(b) - f64::from(a)) * done) as f32;
    (along(from.0, to.0), along(from.1, to.1))
}

fn squares((x, y): (u32, u32)) -> (f32, f32) {
    (x as f32, y as f32)
}

/// Straight-line distance in squares.
fn distance(a: (u32, u32), b: (u32, u32)) -> f64 {
    let dx = f64::from(a.0) - f64::from(b.0);
    let dy = f64::from(a.1) - f64::from(b.1);
    (dx * dx + dy * dy).sqrt()
}

fn server(train: &Value) -> Option<u32> {
    int(train, "serverId").and_then(|s| u32::try_from(s).ok())
}

/// The route's station ids, sent as a typed integer array or as an array of integers.
fn stations(train: &Value) -> Option<Vec<u32>> {
    let ids: Vec<i64> = match train.get("stationList")? {
        Value::ShortArray(items) => items.iter().map(|&v| v.into()).collect(),
        Value::IntArray(items) => items.iter().map(|&v| v.into()).collect(),
        Value::LongArray(items) => items.clone(),
        Value::Array(items) => items.iter().map(Value::as_i64).collect::<Option<_>>()?,
        _ => return None,
    };
    let ids = ids
        .into_iter()
        .map(|id| u32::try_from(id).ok())
        .collect::<Option<Vec<u32>>>()?;
    (!ids.is_empty()).then_some(ids)
}

fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value.get(key).and_then(Value::as_array).unwrap_or_default()
}

fn int(value: &Value, key: &str) -> Option<i64> {
    value.get(key)?.as_i64()
}

/// Any number, widened.
fn number(value: &Value) -> Option<f64> {
    match *value {
        Value::Float(v) => Some(f64::from(v)),
        Value::Double(v) => Some(v),
        ref other => other.as_i64().map(|v| v as f64),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::protocol::proto::tests::{bytes_field, hex, int_field, varint_bytes};

    /// 2026-10-26 23:00 UTC, in milliseconds.
    pub(crate) const T0: u64 = 1_793_055_600_000;
    pub(crate) const SERVER: i32 = 901;
    /// A made-up alliance id.
    pub(crate) const OTHERS: &str = "0123456789abcdef0123456789abcdef";

    fn obj(entries: Vec<(&str, Value)>) -> Value {
        Value::Object(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// Point id of (x, y) on a 1000-square map.
    fn point(x: u64, y: u64) -> u64 {
        y * 1000 + x + 1
    }

    /// Cities of the test routes: station 10 at (100, 100), 20 at (130, 140) (50 squares on),
    /// 30 at (130, 240) (100 squares on), 40 at (230, 240).
    pub(crate) const CITIES: [(u32, (u64, u64)); 4] = [
        (10, (100, 100)),
        (20, (130, 140)),
        (30, (130, 240)),
        (40, (230, 240)),
    ];

    fn city(station: u32) -> (u64, u64) {
        CITIES.iter().find(|(s, _)| *s == station).unwrap().1
    }

    /// What a truck is doing in a test march.
    pub(crate) struct Run {
        pub cfg_id: i64,
        pub id: i64,
        pub power: i64,
        pub raids: i64,
        pub alliance: &'static str,
        /// Index of the city it is driving to (or at, if `stopped`).
        pub index: i64,
        pub stopped: bool,
        /// When it set off on this leg, in milliseconds.
        pub left: u64,
    }

    impl Default for Run {
        fn default() -> Run {
            Run {
                cfg_id: 234,
                id: 1_430_000_000_000_000_001,
                power: 30_000_000,
                raids: 0,
                alliance: OTHERS,
                index: 1,
                stopped: false,
                left: T0,
            }
        }
    }

    /// The route used: 10, 20, 30, 40, at 0.5 squares a second (100 s for the first leg).
    pub(crate) const ROUTE: [u32; 4] = [10, 20, 30, 40];
    const SPEED: f32 = 0.5;

    /// The truck's `train`, as `train.refresh` gives it.
    pub(crate) fn train(run: &Run) -> Value {
        let index = run.index as usize;
        let from = city(ROUTE[index - 1]);
        let to = city(ROUTE[index]);
        let dx = to.0 as f64 - from.0 as f64;
        let dy = to.1 as f64 - from.1 as f64;
        let drive = ((dx * dx + dy * dy).sqrt() / f64::from(SPEED) * 1000.0) as u64;
        let (arrived, next) = if run.stopped {
            (run.left + drive, run.left + drive + 6_000)
        } else {
            (run.left - 6_000, run.left + drive)
        };
        let info = obj(vec![
            ("robTimes", Value::Int(run.raids as i32)),
            ("lastSendTime", Value::Long(run.left as i64)),
            ("lastArriveTime", Value::Long(arrived as i64)),
            ("nextEndTime", Value::Long(next as i64)),
            ("lastPosIndex", Value::Int(run.index as i32)),
            ("isEnd", Value::Bool(false)),
            ("power", Value::Long(run.power)),
            (
                "effectInfo",
                obj(vec![(
                    SPEED_BUFF,
                    Value::Double(f64::from(SPEED) / 0.25 - 1.0),
                )]),
            ),
        ]);
        obj(vec![
            ("serverId", Value::Int(SERVER)),
            ("uuid", Value::Long(run.id)),
            ("allianceId", Value::Str(run.alliance.into())),
            ("arriveTime", Value::Long((T0 + 3_600_000) as i64)),
            ("marchInfo", info),
            ("startPos", Value::Int(point(90, 90) as i32)),
            (
                "stationList",
                Value::Array(ROUTE.iter().map(|&s| Value::Int(s as i32)).collect()),
            ),
            ("cfgId", Value::Int(run.cfg_id as i32)),
        ])
    }

    /// The truck's march, as `push.world.march.new` gives it, with its leg in the protobuf.
    pub(crate) fn march(run: &Run) -> Value {
        let index = run.index as usize;
        let (from, to) = (city(ROUTE[index - 1]), city(ROUTE[index]));
        let mut leg = bytes_field(1, b"Example");
        leg.extend(int_field(9, point(from.0, from.1)));
        leg.extend(int_field(10, point(to.0, to.1)));
        leg.extend(varint_bytes((17 << 3) | 5));
        leg.extend(SPEED.to_le_bytes());
        obj(vec![
            ("_proto", Value::Str(hex(&leg))),
            ("mt", Value::Int(14)),
            ("uuid", Value::Long(run.id + 1)),
            ("train", train(run)),
        ])
    }

    /// A `push.world.march.world.get.new` carrying these marches.
    pub(crate) fn area(marches: Vec<Value>) -> Value {
        let entry = obj(vec![
            ("marchInfos", Value::Array(marches)),
            ("serverId", Value::Int(SERVER)),
        ]);
        obj(vec![("serverMarchArr", Value::Array(vec![entry]))])
    }

    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    /// Trucks that know every city of the test route, from marches of a truck not tracked.
    pub(crate) fn mapped() -> Trucks {
        let mut trucks = Trucks::default();
        for index in 1..ROUTE.len() as i64 {
            let run = Run {
                cfg_id: 100,
                index,
                ..Run::default()
            };
            trucks.apply("push.world.march.new", &march(&run), at(T0));
        }
        trucks
    }

    #[test]
    fn kinds_follow_the_config_id() {
        assert_eq!(kind(234), Some(Kind::Reindeer));
        assert_eq!(kind(5031), Some(Kind::Ur));
        assert_eq!(kind(5035), Some(Kind::Ur));
        assert_eq!((kind(140), kind(150)), (Some(Kind::Ur), Some(Kind::Ur)));
        assert_eq!((kind(139), kind(151), kind(88)), (None, None, None));
        assert_eq!(kind(5036), None);
        assert_eq!((Kind::Reindeer.max_raids(), Kind::Ur.max_raids()), (1, 2));
    }

    #[test]
    fn any_truck_s_march_places_its_leg_s_cities() {
        let trucks = mapped();
        assert_eq!(trucks.cities(SERVER as u32), 4);
        assert_eq!(trucks.city(SERVER as u32, 30), Some((130, 240)));
        assert_eq!(trucks.city(SERVER as u32 + 1, 30), None);
        // The truck itself isn't of a kind worth tracking.
        assert_eq!(trucks.trucks(SERVER as u32).count(), 0);
    }

    #[test]
    fn a_truck_on_its_way_is_between_its_two_cities() {
        let mut trucks = Trucks::default();
        assert!(trucks.apply("push.world.march.new", &march(&Run::default()), at(T0)));
        let truck = trucks.trucks(SERVER as u32).next().unwrap().clone();
        assert_eq!(
            (truck.kind, truck.power, truck.raids),
            (Kind::Reindeer, 30_000_000, 0)
        );
        // 50 squares from (100, 100) to (130, 140) at 0.5 a second: halfway after 50 s.
        assert_eq!(trucks.position(&truck, at(T0)), Some((100.0, 100.0)));
        assert_eq!(
            trucks.position(&truck, at(T0 + 50_000)),
            Some((115.0, 120.0))
        );
        // Arrived at 100 s, and stopped there until it sets off again.
        assert_eq!(
            trucks.position(&truck, at(T0 + 100_000)),
            Some((130.0, 140.0))
        );
        // The next city isn't known from this one march: nowhere after it sets off.
        assert_eq!(trucks.position(&truck, at(T0 + 110_000)), None);
    }

    #[test]
    fn known_cities_carry_the_route_on() {
        let mut trucks = mapped();
        trucks.apply("push.world.march.new", &march(&Run::default()), at(T0));
        let truck = trucks.trucks(SERVER as u32).next().unwrap().clone();
        let stops = trucks.stops(&truck);
        let arrivals: Vec<u64> = stops.iter().map(|s| s.arrives.as_millis() as u64).collect();
        // 100 s to the first, a stop, 200 s on, a stop, 200 s on.
        let stop = STOP.as_millis() as u64;
        assert_eq!(
            arrivals,
            [T0 + 100_000, T0 + 300_000 + stop, T0 + 500_000 + 2 * stop]
        );
        // Halfway along the second leg, (130, 140) to (130, 240).
        let halfway = T0 + 200_000 + stop;
        assert_eq!(trucks.position(&truck, at(halfway)), Some((130.0, 190.0)));
        // At the last city, until the route ends.
        let parked = T0 + 600_000 + 2 * stop;
        assert_eq!(trucks.position(&truck, at(parked)), Some((230.0, 240.0)));
    }

    #[test]
    fn a_tap_updates_the_truck_but_an_older_march_does_not() {
        let mut trucks = mapped();
        let later = Run {
            index: 2,
            left: T0 + 106_000,
            ..Run::default()
        };
        trucks.apply("push.world.march.new", &march(&later), at(T0));
        // An out-of-order march about the leg before changes nothing.
        assert!(!trucks.apply("push.world.march.new", &march(&Run::default()), at(T0)));
        // A tap brings a new power; it has no protobuf, so the truck keeps its speed.
        let tapped = Run {
            power: 31_000_000,
            ..later
        };
        let refresh = obj(vec![("train", train(&tapped))]);
        assert!(trucks.apply("train.refresh", &refresh, at(T0)));
        let truck = trucks.trucks(SERVER as u32).next().unwrap();
        assert_eq!(truck.power, 31_000_000);
        assert!((truck.speed - 0.5).abs() < 1e-9);
        // The short reply, when the game has the truck already, carries nothing.
        let short = obj(vec![
            ("uuid", Value::Long(1)),
            ("lastUpdateTime", Value::Long(1)),
        ]);
        assert!(!trucks.apply("train.refresh", &short, at(T0)));
    }

    #[test]
    fn a_stopped_truck_waits_at_its_city() {
        let mut trucks = mapped();
        let stopped = Run {
            stopped: true,
            ..Run::default()
        };
        trucks.apply("push.world.march.new", &march(&stopped), at(T0));
        let truck = trucks.trucks(SERVER as u32).next().unwrap().clone();
        assert_eq!(
            trucks.position(&truck, at(T0 + 103_000)),
            Some((130.0, 140.0))
        );
        assert_eq!(trucks.stops(&truck)[0].leaves, at(T0 + 106_000));
    }

    #[test]
    fn trucks_go_once_raided_out_or_their_route_ends() {
        let mut trucks = Trucks::default();
        let ur = Run {
            cfg_id: 5034,
            id: 7,
            raids: 1,
            ..Run::default()
        };
        let reindeer = Run {
            id: 8,
            raids: 1,
            ..Run::default()
        };
        trucks.apply(
            "push.world.march.world.get.new",
            &area(vec![march(&ur), march(&reindeer)]),
            at(T0),
        );
        // A UR truck takes two raids, a reindeer one.
        let ids: Vec<u64> = trucks.trucks(SERVER as u32).map(|t| t.id).collect();
        assert_eq!(ids, [7]);
        trucks.apply(
            "push.world.march.new",
            &march(&Run::default()),
            at(T0 + 3_600_000),
        );
        assert_eq!(trucks.trucks(SERVER as u32).count(), 0);
    }

    #[test]
    fn other_marches_and_damaged_ones_change_nothing() {
        let mut trucks = Trucks::default();
        let mut other = march(&Run::default());
        if let Value::Object(entries) = &mut other {
            entries.retain(|(k, _)| k != "mt");
            entries.push(("mt".into(), Value::Int(1)));
        }
        assert!(!trucks.apply("push.world.march.new", &other, at(T0)));
        let mut damaged = march(&Run::default());
        if let Value::Object(entries) = &mut damaged {
            entries.retain(|(k, _)| k != "_proto");
            entries.push(("_proto".into(), Value::Str("zz".into())));
        }
        // Still a truck, placed nowhere until its cities are known.
        assert!(trucks.apply("push.world.march.new", &damaged, at(T0)));
        assert_eq!(trucks.cities(SERVER as u32), 0);
    }
}
