//! Secret tasks on the world map: where they are, how rare, how many loots they have left and
//! when they can be looted, per server. Only that is kept; who a task belongs to is not.
//!
//! `world.get.block` lists the objects in the part of the map the game loads as it scrolls;
//! `world.get.detail.new` describes the object tapped. A task is replaced each time either
//! shows it, and dropped once it leaves the map or its point shows something else.

use std::borrow::Cow;
use std::collections::HashMap;
use std::time::Duration;

use crate::protocol::proto::{self, Field};
use crate::protocol::sfs::Value;

/// The map's side, in squares, when a message doesn't say. Point ids number the squares row by
/// row from 1: id − 1 = y × side + x.
const DEFAULT_SIZE: u32 = 1000;
/// The largest map side taken from a message.
const MAX_SIZE: u32 = 10_000;
/// The point type of a secret task.
const TASK_POINT: u64 = 17;
/// How many times a task can be looted.
pub const MAX_LOOTS: u32 = 3;
/// Tasks kept at most, across servers.
const MAX_TASKS: usize = 20_000;

/// A task's rarity, from its task id. In order of interest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rarity {
    /// Gold, shiny.
    Shiny,
    /// Gold.
    Ur,
    /// Purple.
    Ssr,
    /// Blue.
    Sr,
    /// A task id outside the families seen so far.
    Other,
}

/// The rarity of task `id`. Ids seen so far: SR 3xxxxx, SSR 4xxxxx, UR 5000xxxx and 600099xx,
/// shiny the other 6000xxxx (which stay on the map until a later reset).
pub fn rarity(id: u64) -> Rarity {
    match id {
        300_000..=399_999 => Rarity::Sr,
        400_000..=499_999 => Rarity::Ssr,
        50_000_000..=50_009_999 | 60_009_900..=60_009_999 => Rarity::Ur,
        60_000_000..=60_009_899 => Rarity::Shiny,
        _ => Rarity::Other,
    }
}

/// A secret task as last seen. Times are on the game server's clock.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub x: u32,
    pub y: u32,
    /// The task's id, which says its rarity.
    pub id: u64,
    /// How many times it has been looted.
    pub loots: u32,
    /// When the task finishes; it can't be looted before.
    pub ready: Duration,
    /// When it leaves the map.
    pub ends: Duration,
    /// When it was seen.
    pub seen: Duration,
}

impl Task {
    pub fn rarity(&self) -> Rarity {
        rarity(self.id)
    }

    pub fn full(&self) -> bool {
        self.loots >= MAX_LOOTS
    }

    /// Still on the map at `now`, and not looted in full when last seen.
    pub fn open(&self, now: Duration) -> bool {
        !self.full() && now < self.ends
    }

    /// Can be looted at `now`: finished, still on the map, with a loot left.
    pub fn lootable(&self, now: Duration) -> bool {
        self.open(now) && now >= self.ready
    }
}

/// The part of the map the game loaded last.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub server: u32,
    /// How far out the map is zoomed: 0 lists secret tasks, further out doesn't.
    pub view_level: i64,
    /// The map's side, in squares.
    pub size: u32,
    /// The corners loaded: left, bottom, right, top.
    pub area: (u32, u32, u32, u32),
    pub time: Duration,
}

impl Look {
    /// Whether the game lists secret tasks at this zoom.
    pub fn shows_tasks(&self) -> bool {
        self.view_level == 0
    }
}

/// Which parts of one server's map have been loaded at a zoom that lists secret tasks, in
/// cells [`CELL`] squares across.
#[derive(Debug, Clone, PartialEq)]
pub struct Scan {
    /// Cells across (and down) the map.
    pub across: u32,
    /// Row by row from the bottom: when each cell was last loaded, in seconds on the game
    /// server's clock; 0 if never.
    pub loaded: Vec<u32>,
}

impl Scan {
    fn new(size: u32) -> Scan {
        let across = size.div_ceil(CELL);
        Scan {
            across,
            loaded: vec![0; (across * across) as usize],
        }
    }

    /// Marks the cells under `area` (left, bottom, right, top squares) as loaded at `time`.
    fn mark(&mut self, area: (u32, u32, u32, u32), time: Duration) {
        let (left, bottom, right, top) = area;
        let last = self.across - 1;
        let secs = u32::try_from(time.as_secs()).unwrap_or(u32::MAX).max(1);
        for row in (bottom / CELL).min(last)..=(top / CELL).min(last) {
            for col in (left / CELL).min(last)..=(right / CELL).min(last) {
                self.loaded[(row * self.across + col) as usize] = secs;
            }
        }
    }
}

/// The map is tracked as scanned in cells this many squares across: the width of the strips
/// the game loads as the map scrolls.
pub const CELL: u32 = 10;

#[derive(Debug, Default)]
pub struct World {
    /// Server → point id → task.
    servers: HashMap<u32, HashMap<u64, Task>>,
    /// Server → what has been scanned.
    scans: HashMap<u32, Scan>,
    last: Option<Look>,
}

impl World {
    /// Takes in a message at `time` (on the game server's clock). Returns whether anything
    /// changed.
    pub fn apply(&mut self, command: &str, data: &Value, time: Duration) -> bool {
        let changed = match command {
            "world.get.block" => self.apply_block(data, time),
            "world.get.detail.new" => self.apply_detail(data, time),
            _ => return false,
        };
        self.prune(time);
        changed
    }

    /// The part of the map the game loaded last, on the server being viewed.
    pub fn last_look(&self) -> Option<&Look> {
        self.last.as_ref()
    }

    /// What has been scanned on `server`.
    pub fn scan(&self, server: u32) -> Option<&Scan> {
        self.scans.get(&server)
    }

    /// The tasks seen on `server`.
    pub fn tasks(&self, server: u32) -> impl Iterator<Item = &Task> {
        self.servers
            .get(&server)
            .into_iter()
            .flat_map(|t| t.values())
    }

    /// `world.get.block`: `serverPointArr` holds one entry per server, each with its `points`.
    fn apply_block(&mut self, data: &Value, time: Duration) -> bool {
        let mut changed = false;
        for block in data
            .get("serverPointArr")
            .and_then(Value::as_array)
            .unwrap_or_default()
        {
            let Some(server) = int(block, "serverId").and_then(|s| u32::try_from(s).ok()) else {
                continue;
            };
            let size = int(block, "maxAreaSize")
                .and_then(|s| u32::try_from(s).ok())
                .filter(|s| (1..=MAX_SIZE).contains(s))
                .unwrap_or(DEFAULT_SIZE);
            let corner = |key| int(block, key).and_then(|p| position(p as u64, size));
            if let (Some((left, bottom)), Some((right, top))) =
                (corner("leftBottom"), corner("rightTop"))
            {
                let look = Look {
                    server,
                    view_level: int(block, "viewLvl").unwrap_or(0),
                    size,
                    area: (left, bottom, right, top),
                    time,
                };
                changed |= self.last.as_ref().is_none_or(|last| {
                    Look {
                        time,
                        ..last.clone()
                    } != look
                });
                if look.shows_tasks() {
                    let scan = self.scans.entry(server).or_insert_with(|| Scan::new(size));
                    if scan.across != size.div_ceil(CELL) {
                        *scan = Scan::new(size);
                    }
                    scan.mark(look.area, time);
                    changed = true;
                }
                self.last = Some(look);
            }
            for point in block
                .get("points")
                .and_then(Value::as_array)
                .unwrap_or_default()
            {
                if let Some(bytes) = point_bytes(point) {
                    changed |= self.apply_point(server, size, &bytes, time);
                }
            }
        }
        changed
    }

    /// One map point: a task is kept, anything else clears a task the point held.
    fn apply_point(&mut self, server: u32, size: u32, bytes: &[u8], time: Duration) -> bool {
        let Ok(fields) = proto::fields(bytes) else {
            return false;
        };
        let Some(point) = proto::first(&fields, 1).and_then(|f| f.as_u64()) else {
            return false;
        };
        let Some((x, y)) = position(point, size) else {
            return false;
        };
        let kind = proto::first(&fields, 2).and_then(|f| f.as_u64());
        let task = kind
            .filter(|k| *k == TASK_POINT)
            .and_then(|_| proto::first(&fields, 10))
            .and_then(|f| f.as_bytes())
            .and_then(|b| task(b, x, y, time));
        match task {
            Some(task) => self.keep(server, point, task),
            None => self
                .servers
                .get_mut(&server)
                .and_then(|t| t.remove(&point))
                .is_some(),
        }
    }

    /// `world.get.detail.new` for a secret task: its point, server, id, loots and times. Other
    /// objects' details lack those, and change nothing.
    fn apply_detail(&mut self, data: &Value, time: Duration) -> bool {
        let fields = (
            int(data, "pointId"),
            int(data, "srcServer").and_then(|s| u32::try_from(s).ok()),
            int(data, "cfgId"),
            data.get("stealInfoList").and_then(Value::as_array),
            int(data, "actEndTime"),
        );
        let (Some(point), Some(server), Some(id), Some(loots), Some(ends)) = fields else {
            return false;
        };
        let size = self
            .last
            .as_ref()
            .filter(|l| l.server == server)
            .map_or(DEFAULT_SIZE, |l| l.size);
        let Some((x, y)) = position(point as u64, size) else {
            return false;
        };
        let task = Task {
            x,
            y,
            id: id as u64,
            loots: loots.len() as u32,
            ready: millis(int(data, "completionTime").unwrap_or(0)),
            ends: millis(ends),
            seen: time,
        };
        self.keep(server, point as u64, task)
    }

    /// Keeps `task` at `point`, unless the limit is reached. Returns whether anything changed.
    fn keep(&mut self, server: u32, point: u64, task: Task) -> bool {
        let known = self
            .servers
            .get(&server)
            .is_some_and(|t| t.contains_key(&point));
        if !known && self.servers.values().map(HashMap::len).sum::<usize>() >= MAX_TASKS {
            return false;
        }
        let tasks = self.servers.entry(server).or_default();
        let before = tasks.insert(point, task.clone());
        before.is_none_or(|b| {
            Task {
                seen: task.seen,
                ..b
            } != task
        })
    }

    /// Drops the tasks that have left the map.
    fn prune(&mut self, now: Duration) {
        for tasks in self.servers.values_mut() {
            tasks.retain(|_, t| now < t.ends);
        }
        self.servers.retain(|_, t| !t.is_empty());
    }
}

/// A task from a type-17 point's field 10: 2 the task id, 3 when it finishes, 4 once per loot
/// (the looter), 8 when it leaves the map (times in milliseconds).
fn task(bytes: &[u8], x: u32, y: u32, time: Duration) -> Option<Task> {
    let fields = proto::fields(bytes).ok()?;
    let get = |n| proto::first(&fields, n).and_then(|f: Field| f.as_u64());
    Some(Task {
        x,
        y,
        id: get(2)?,
        loots: proto::count(&fields, 4) as u32,
        ready: Duration::from_millis(get(3).unwrap_or(0)),
        ends: Duration::from_millis(get(8)?),
        seen: time,
    })
}

/// (x, y) of point id `point` on a map `size` squares across.
fn position(point: u64, size: u32) -> Option<(u32, u32)> {
    let size = u64::from(size);
    let index = point.checked_sub(1).filter(|i| *i < size * size)?;
    Some(((index % size) as u32, (index / size) as u32))
}

fn int(value: &Value, key: &str) -> Option<i64> {
    value.get(key)?.as_i64()
}

fn millis(ms: i64) -> Duration {
    Duration::from_millis(ms.max(0) as u64)
}

/// A map point's protobuf message: sent as a byte array, or as a hex string.
fn point_bytes(value: &Value) -> Option<Cow<'_, [u8]>> {
    match value {
        Value::ByteArray(bytes) => Some(Cow::Borrowed(bytes)),
        other => proto::from_hex(other.as_str()?).map(Cow::Owned),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::protocol::proto::tests::{bytes_field, hex, int_field};

    /// Monday 2026-10-19 00:00 UTC, in milliseconds.
    const DAY0: u64 = 1_792_368_000_000;
    const HOUR: u64 = 3_600_000;

    fn obj(entries: Vec<(&str, Value)>) -> Value {
        Value::Object(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// A type-17 point at (x, y) on a 1000-square map: task `id`, looted `loots` times, ready at
    /// `ready` and leaving at `ends` (milliseconds).
    pub(crate) fn task_point(
        x: u64,
        y: u64,
        id: u64,
        loots: usize,
        ready: u64,
        ends: u64,
    ) -> String {
        let mut task = bytes_field(1, b"1000000001000001");
        task.extend(int_field(2, id));
        task.extend(int_field(3, ready));
        for _ in 0..loots {
            task.extend(bytes_field(4, b"1000000002000001"));
        }
        task.extend(int_field(8, ends));
        task.extend(bytes_field(9, b"0123456789abcdef0123456789abcdef"));
        let mut point = int_field(1, y * 1000 + x + 1);
        point.extend(int_field(2, TASK_POINT));
        point.extend(bytes_field(10, &task));
        point.extend(int_field(100, 1_430_000_000_000_000_000));
        hex(&point)
    }

    /// Some other object at (x, y), of point type `kind`.
    pub(crate) fn other_point(x: u64, y: u64, kind: u64) -> String {
        let mut point = int_field(1, y * 1000 + x + 1);
        point.extend(int_field(2, kind));
        point.extend(bytes_field(6, &int_field(1, 101)));
        hex(&point)
    }

    /// A `world.get.block` reply for `server` at `view_level`, around (400..500, 900..999), with
    /// the points as byte arrays, as the game sends them (anything not hex as a string).
    pub(crate) fn block(server: i32, view_level: i32, points: Vec<String>) -> Value {
        let points = points
            .into_iter()
            .map(|p| proto::from_hex(&p).map_or(Value::Str(p), Value::ByteArray))
            .collect();
        block_of(server, view_level, points)
    }

    fn block_of(server: i32, view_level: i32, points: Vec<Value>) -> Value {
        let entry = obj(vec![
            ("maxAreaSize", Value::Int(1000)),
            ("leftBottom", Value::Int(900_401)),
            ("rightTop", Value::Int(1_000_000)),
            ("serverId", Value::Int(server)),
            ("viewLvl", Value::Int(view_level)),
            ("points", Value::Array(points)),
        ]);
        obj(vec![("serverPointArr", Value::Array(vec![entry]))])
    }

    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn rarity_follows_the_task_id() {
        assert_eq!(rarity(300_402), Rarity::Sr);
        assert_eq!(rarity(400_502), Rarity::Ssr);
        assert_eq!(rarity(50_000_604), Rarity::Ur);
        assert_eq!(rarity(60_009_901), Rarity::Ur);
        assert_eq!(rarity(60_000_501), Rarity::Shiny);
        assert_eq!(rarity(60_000_601), Rarity::Shiny);
        assert_eq!(rarity(70_000_001), Rarity::Other);
    }

    #[test]
    fn blocks_give_tasks_and_where_the_map_is_looked_at() {
        let mut world = World::default();
        let now = DAY0 + 10 * HOUR;
        let points = vec![
            task_point(250, 750, 60_000_601, 0, now + HOUR, now + 50 * HOUR),
            task_point(700, 800, 400_502, 1, now - HOUR, now + 16 * HOUR),
            other_point(260, 760, 7),
            "zz".into(),
        ];
        assert!(world.apply("world.get.block", &block(901, 0, points), at(now)));
        let look = world.last_look().unwrap();
        assert_eq!((look.server, look.size), (901, 1000));
        assert_eq!(look.area, (400, 900, 999, 999));
        assert!(look.shows_tasks());

        let mut tasks: Vec<&Task> = world.tasks(901).collect();
        tasks.sort_by_key(|t| t.x);
        assert_eq!(tasks.len(), 2);
        let shiny = tasks[0];
        assert_eq!(
            (shiny.x, shiny.y, shiny.id, shiny.loots),
            (250, 750, 60_000_601, 0)
        );
        assert_eq!(shiny.rarity(), Rarity::Shiny);
        assert!(
            shiny.open(at(now)) && !shiny.lootable(at(now)),
            "still in progress"
        );
        assert!(shiny.lootable(at(now + HOUR)));
        assert_eq!((tasks[1].loots, tasks[1].rarity()), (1, Rarity::Ssr));
        assert!(tasks[1].lootable(at(now)));
        assert_eq!(world.tasks(902).count(), 0);
    }

    #[test]
    fn a_task_is_replaced_when_seen_again_and_dropped_when_it_goes() {
        let mut world = World::default();
        let now = DAY0;
        let seen = |loots| {
            block(
                901,
                0,
                vec![task_point(1, 2, 400_502, loots, now, now + HOUR)],
            )
        };
        world.apply("world.get.block", &seen(1), at(now));
        world.apply("world.get.block", &seen(1), at(now + 1));
        let tasks: Vec<&Task> = world.tasks(901).collect();
        assert_eq!(
            (tasks.len(), tasks[0].loots),
            (1, 1),
            "seen again, not twice"
        );
        world.apply("world.get.block", &seen(3), at(now + 2));
        let task = world.tasks(901).next().unwrap();
        assert!(task.full() && !task.open(at(now + 2)));

        // The point now shows something else.
        world.apply(
            "world.get.block",
            &block(901, 0, vec![other_point(1, 2, 7)]),
            at(now + 3),
        );
        assert_eq!(world.tasks(901).count(), 0);

        // Gone from the map once its end time passes.
        world.apply("world.get.block", &seen(0), at(now));
        world.apply("world.get.block", &block(901, 0, vec![]), at(now + HOUR));
        assert_eq!(world.tasks(901).count(), 0);
    }

    #[test]
    fn points_sent_as_hex_text_are_read_too() {
        let mut world = World::default();
        let point = task_point(1, 2, 400_502, 0, DAY0, DAY0 + HOUR);
        let data = block_of(901, 0, vec![Value::Str(point)]);
        world.apply("world.get.block", &data, at(DAY0));
        assert_eq!(world.tasks(901).count(), 1);
    }

    #[test]
    fn zoomed_out_loads_say_so() {
        let mut world = World::default();
        world.apply("world.get.block", &block(901, 1, vec![]), at(DAY0));
        assert!(!world.last_look().unwrap().shows_tasks());
        assert_eq!(world.scan(901), None, "nothing is scanned that far out");
    }

    #[test]
    fn loads_mark_the_cells_they_cover_as_scanned() {
        let mut world = World::default();
        world.apply("world.get.block", &block(901, 0, vec![]), at(DAY0));
        let scan = world.scan(901).unwrap();
        assert_eq!(scan.across, 100);
        // Squares 400..=999 across and 900..=999 up: cells 40..=99 by 90..=99.
        let secs = (DAY0 / 1000) as u32;
        let loaded = |col: u32, row: u32| scan.loaded[(row * 100 + col) as usize];
        assert_eq!(scan.loaded.iter().filter(|t| **t == secs).count(), 60 * 10);
        assert_eq!((loaded(40, 90), loaded(99, 99)), (secs, secs));
        assert_eq!((loaded(39, 90), loaded(40, 89)), (0, 0));
        assert_eq!(world.scan(902), None);
    }

    #[test]
    fn tapping_a_task_updates_its_loots() {
        let mut world = World::default();
        let now = DAY0;
        let points = vec![task_point(700, 800, 400_502, 0, now, now + HOUR)];
        world.apply("world.get.block", &block(901, 0, points), at(now));
        let looter = || obj(vec![("uid", Value::Str("1".into()))]);
        let detail = obj(vec![
            ("pointId", Value::Int(800_701)),
            ("srcServer", Value::Int(901)),
            ("cfgId", Value::Int(400_502)),
            ("stealInfoList", Value::Array(vec![looter(), looter()])),
            ("completionTime", Value::Long(now as i64)),
            ("actEndTime", Value::Long((now + HOUR) as i64)),
        ]);
        assert!(world.apply("world.get.detail.new", &detail, at(now + 5)));
        let task = world.tasks(901).next().unwrap();
        assert_eq!((task.x, task.y, task.loots), (700, 800, 2));

        // Another object's details change nothing.
        let player = obj(vec![
            ("pointId", Value::Int(5)),
            ("srcServer", Value::Int(901)),
        ]);
        assert!(!world.apply("world.get.detail.new", &player, at(now + 6)));
    }

    #[test]
    fn points_off_the_map_are_ignored() {
        assert_eq!(position(1, 1000), Some((0, 0)));
        assert_eq!(position(1_000_000, 1000), Some((999, 999)));
        assert_eq!(position(0, 1000), None);
        assert_eq!(position(1_000_001, 1000), None);
    }
}
