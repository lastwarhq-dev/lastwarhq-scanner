//! Desert Storm and Canyon Storm results from the game's local mail database, `config.db`
//! (SQLite).
//!
//! The file is only ever read: opened read-only with full sharing so the game is never
//! blocked, copied into memory in one pass, and decoded from that copy. No SQLite library is
//! used, because opening a database with one can take locks or roll back a half-finished save,
//! which writes to the file.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::game::battle::{Battle, Event};
use crate::mail::sqlite::{Database, SqlValue};
use crate::util::json::{self, Json};

/// Mail `type` of a Desert Storm battle result.
const DS_RESULT_TYPE: i64 = 109;
/// Mail `type` of a Canyon Storm battle result.
const CS_RESULT_TYPE: i64 = 181;

/// `%USERPROFILE%\AppData\LocalLow\FunFly\Last War-Survival Game\config.db`
pub fn db_path() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE")?;
    Some(PathBuf::from(home).join(r"AppData\LocalLow\FunFly\Last War-Survival Game\config.db"))
}

/// A consistent in-memory copy of the database. A save in progress (a `-journal` file beside
/// the database, or the header's change counter moving during the read) means retrying.
///
/// Only rollback-journal databases are read. In WAL mode the newest data can live in the
/// `-wal` file, so reading the main file alone could give stale results; that is refused.
pub fn read_snapshot(path: &Path) -> Result<Vec<u8>, String> {
    let beside = |suffix: &str| {
        path.with_file_name(format!(
            "{}{suffix}",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("config.db")
        ))
    };
    let (journal, wal) = (beside("-journal"), beside("-wal"));
    if std::fs::metadata(&wal).is_ok_and(|m| m.len() > 0) {
        return Err("the mail database is in WAL mode, which is not supported".into());
    }
    for attempt in 0..5 {
        if attempt > 0 {
            thread::sleep(Duration::from_millis(400));
        }
        if journal.exists() {
            continue;
        }
        let bytes = read_shared(path, None)?;
        let header = read_shared(path, Some(100))?;
        if bytes.len() < 100 || header.len() < 100 {
            continue;
        }
        // Header bytes 18 and 19 are the write and read versions: 2 means WAL mode.
        if bytes[18] == 2 || bytes[19] == 2 {
            return Err("the mail database is in WAL mode, which is not supported".into());
        }
        // Offset 24 is the change counter, raised by every save.
        if bytes[24..28] == header[24..28] && !journal.exists() {
            return Ok(bytes);
        }
    }
    Err("the game is saving its mail; the next read tries again".into())
}

/// Reads the whole file, or its first `limit` bytes, through a read-only handle. Rust opens
/// files on Windows with read, write and delete sharing, so the game can keep using it.
fn read_shared(path: &Path, limit: Option<u64>) -> Result<Vec<u8>, String> {
    let file = File::options()
        .read(true)
        .open(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    let result = match limit {
        Some(n) => file.take(n).read_to_end(&mut bytes),
        None => (&file).read_to_end(&mut bytes),
    };
    result.map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Ok(bytes)
}

/// Mails for the same battle sent to different accounts arrive seconds apart; battles of the
/// same team are a week apart.
const SAME_BATTLE_WINDOW: Duration = Duration::from_secs(600);

/// Every Desert Storm and Canyon Storm result in the database, one per battle and alliance.
/// Several accounts on one PC can each hold a copy of the same battle's mail: copies share the
/// event, alliance, opponent and both scores, and arrive within [`SAME_BATTLE_WINDOW`] of each
/// other.
pub fn battles(db_bytes: &[u8]) -> Result<Vec<Battle>, String> {
    let db = Database::open(db_bytes)?;
    let table = db
        .tables()?
        .into_iter()
        .find(|t| t.name == "MailData")
        .ok_or("no MailData table")?;
    let columns = column_names(&table.sql);
    let col = |name: &str| {
        columns
            .iter()
            .position(|c| c == name)
            .ok_or_else(|| format!("MailData has no {name} column"))
    };
    let (type_col, time_col, contents_col) = (col("type")?, col("createTime")?, col("contents")?);

    // Rows are read one at a time; only the battles found are kept.
    let mut battles: Vec<Battle> = Vec::new();
    db.for_each_row(table.root_page, |row| {
        let parse: fn(&str, i64) -> Vec<Battle> = match row.get(type_col) {
            Some(SqlValue::Int(DS_RESULT_TYPE)) => |c, t| parse_result(c, t).into_iter().collect(),
            Some(SqlValue::Int(CS_RESULT_TYPE)) => {
                |c, t| parse_canyon_result(c, t).unwrap_or_default()
            }
            _ => return Ok(()),
        };
        let Some(&SqlValue::Int(created)) = row.get(time_col) else {
            return Ok(());
        };
        let contents = match row.get(contents_col) {
            Some(SqlValue::Text(s)) => s.clone(),
            Some(SqlValue::Blob(b)) => String::from_utf8_lossy(b).into_owned(),
            _ => return Ok(()),
        };
        for battle in parse(&contents, created) {
            if !battles.iter().any(|b| same_battle(b, &battle)) {
                battles.push(battle);
            }
        }
        Ok(())
    })?;
    battles.sort_by_key(|b| b.time);
    Ok(battles)
}

fn same_battle(a: &Battle, b: &Battle) -> bool {
    a.event == b.event
        && a.alliance_id == b.alliance_id
        && a.enemy_abbr == b.enemy_abbr
        && a.score == b.score
        && a.enemy_score == b.enemy_score
        && a.time.abs_diff(b.time) <= SAME_BATTLE_WINDOW
}

/// Column names, in order, from a `CREATE TABLE` statement.
fn column_names(sql: &str) -> Vec<String> {
    let Some(open) = sql.find('(') else {
        return Vec::new();
    };
    let body = sql[open + 1..].trim_end().trim_end_matches(')');
    body.split(',')
        .filter_map(|def| def.split_whitespace().next())
        .map(|name| {
            name.trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']')
                .to_string()
        })
        .collect()
}

/// A whole number at `key`. JSON numbers are parsed as f64; only whole numbers it holds exactly
/// (up to 2^53) are used.
fn num(v: &Json, key: &str) -> Option<i64> {
    match v.get(key) {
        Some(Json::Number(n)) if n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_992.0 => {
            Some(*n as i64)
        }
        _ => None,
    }
}

/// The string at `key`, or empty.
fn text(v: &Json, key: &str) -> String {
    v.get(key)
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `contents` of a Desert Storm result mail:
/// `{"b":{...},"obj":{"result","alliances":[ours, theirs],"ranks":[...]}}`.
fn parse_result(contents: &str, created_ms: i64) -> Option<Battle> {
    let doc = json::parse(contents).ok()?;
    let obj = doc.get("obj")?;
    let alliances = obj.get("alliances")?.as_array()?;
    let (ours, theirs) = (alliances.first()?, alliances.get(1)?);
    // An entry without a readable uid and score is skipped, and the list is marked incomplete.
    let ranks = obj.get("ranks")?.as_array()?;
    let players: Vec<(String, i64)> = ranks
        .iter()
        .filter_map(|r| Some((r.get("uid")?.as_str()?.to_string(), num(r, "score")?)))
        .collect();
    Some(Battle {
        event: Event::DesertStorm,
        players_complete: players.len() == ranks.len(),
        time: Duration::from_millis(u64::try_from(created_ms).ok()?),
        won: num(obj, "result")? == 1,
        alliance_id: text(ours, "alId"),
        score: num(ours, "score")?,
        enemy_abbr: text(theirs, "abbr"),
        enemy_name: text(theirs, "name"),
        enemy_score: num(theirs, "score")?,
        players,
    })
}

/// `contents` of a Canyon Storm result mail: `{"b":{...},"obj":{"role","isWin","alliances",
/// "landlord","farmer","ranks"}}`. Side 1 (`landlord`) is one alliance and side 2 (`farmer`)
/// two; `role` is the side the mail's account fought on, and `ranks` lists every player on that
/// side, each with their `allianceId`. One battle is made for each alliance on the side, with
/// its own players: which of the two is ours is known only from the account.
fn parse_canyon_result(contents: &str, created_ms: i64) -> Option<Vec<Battle>> {
    let doc = json::parse(contents).ok()?;
    let obj = doc.get("obj")?;
    let role = num(obj, "role")?;
    let (ours, theirs) = match role {
        1 => ("landlord", "farmer"),
        2 => ("farmer", "landlord"),
        _ => return None,
    };
    let Some(&Json::Bool(won)) = obj.get("isWin") else {
        return None;
    };
    let (score, enemy_score) = (
        num(obj.get(ours)?, "score")?,
        num(obj.get(theirs)?, "score")?,
    );
    let alliances = obj.get("alliances")?.as_array()?;
    let (side, enemies): (Vec<&Json>, Vec<&Json>) =
        alliances.iter().partition(|a| num(a, "role") == Some(role));
    let joined = |key, separator| {
        let names: Vec<String> = enemies.iter().map(|a| text(a, key)).collect();
        names.join(separator)
    };
    let time = Duration::from_millis(u64::try_from(created_ms).ok()?);
    let mut battles: Vec<Battle> = side
        .iter()
        .map(|a| Battle {
            event: Event::CanyonStorm,
            time,
            won,
            alliance_id: text(a, "alId"),
            score,
            enemy_abbr: joined("abbr", "+"),
            enemy_name: joined("name", " + "),
            enemy_score,
            players: Vec::new(),
            players_complete: true,
        })
        .collect();
    // An entry without a readable uid, score and alliance on our side can't be placed: every
    // battle of the mail is then incomplete, as it could be any alliance's player.
    let ranks = obj.get("ranks")?.as_array()?;
    let mut placed = 0;
    for r in ranks {
        let alliance = r.get("allianceId").and_then(Json::as_str);
        let battle = battles
            .iter_mut()
            .find(|b| Some(b.alliance_id.as_str()) == alliance);
        let uid = r.get("uid").and_then(Json::as_str);
        if let (Some(battle), Some(uid), Some(score)) = (battle, uid, num(r, "score")) {
            battle.players.push((uid.to_string(), score));
            placed += 1;
        }
    }
    for battle in &mut battles {
        battle.players_complete = placed == ranks.len();
    }
    Some(battles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_names_from_create_table() {
        let sql = "CREATE TABLE \"MailData\" ( \n\"uid\" varchar primary key not null,\n\"type\" integer,\n\"createTime\" bigint )";
        assert_eq!(column_names(sql), ["uid", "type", "createTime"]);
    }

    #[test]
    fn parses_result_contents() {
        let contents = r#"{"b":{"mailId":23003},"obj":{"result":1,"ranks":[{"uid":"7","score":120,"name":"A","rank":1},{"uid":"8","score":0,"name":"B","rank":2}],"alliances":[{"score":300000,"alId":"ours","abbr":"EXA","name":"Example Alliance"},{"score":250000,"alId":"x","abbr":"OPP","name":"Opponents"}]}}"#;
        let b = parse_result(contents, 1_790_940_621_000).unwrap();
        assert!(b.won);
        assert_eq!((b.score, b.enemy_score), (300_000, 250_000));
        assert_eq!(b.enemy_abbr, "OPP");
        assert_eq!(b.players, [("7".to_string(), 120), ("8".to_string(), 0)]);
        assert!(b.players_complete);
        assert_eq!(b.time, Duration::from_millis(1_790_940_621_000));
    }

    #[test]
    fn an_unreadable_player_entry_marks_the_list_incomplete() {
        let contents = r#"{"obj":{"result":0,"ranks":[{"uid":"7","score":120},{"uid":"8","score":"500"}],"alliances":[{"score":1,"alId":"ours"},{"score":2,"alId":"x"}]}}"#;
        let b = parse_result(contents, 1_790_940_621_000).unwrap();
        assert_eq!(b.players, [("7".to_string(), 120)]);
        assert!(!b.players_complete);
    }

    /// A Canyon Storm result: alliances "ours" and "ally" on side 2 beat "opp" on side 1.
    /// `ranks` lists both alliances' players.
    const CANYON: &str = r#"{"b":{"mailId":61207},"obj":{"role":2,"isWin":true,"landlord":{"score":230000,"isWin":false,"member":20},"farmer":{"score":290000,"isWin":true,"member":3},"alliances":[{"role":1,"alId":"opp","abbr":"OPP","name":"Opponents"},{"role":2,"alId":"ally","abbr":"ALY","name":"Allies"},{"role":2,"alId":"ours","abbr":"EXA","name":"Example Alliance"}],"ranks":[{"allianceId":"ours","uid":"7","score":3891331,"battleScore":3667631,"tacticsScore":195500,"cooperationScore":28200},{"allianceId":"ally","uid":"8","score":900},{"allianceId":"ours","uid":"9","score":0}]}}"#;

    #[test]
    fn a_canyon_result_gives_each_alliance_on_our_side_its_players() {
        let battles = parse_canyon_result(CANYON, 1_793_889_047_566).unwrap();
        let by_alliance: Vec<_> = battles
            .iter()
            .map(|b| (b.alliance_id.as_str(), b.players.clone()))
            .collect();
        assert_eq!(
            by_alliance,
            [
                ("ally", vec![("8".to_string(), 900)]),
                (
                    "ours",
                    vec![("7".to_string(), 3_891_331), ("9".to_string(), 0)]
                ),
            ]
        );
        let ours = &battles[1];
        assert_eq!(ours.event, Event::CanyonStorm);
        assert!(ours.won && ours.players_complete);
        assert_eq!((ours.score, ours.enemy_score), (290_000, 230_000));
        assert_eq!(ours.enemy_abbr, "OPP");
        assert_eq!(ours.time, Duration::from_millis(1_793_889_047_566));
    }

    #[test]
    fn a_canyon_result_on_side_one_is_one_alliance() {
        let contents = r#"{"obj":{"role":1,"isWin":false,"landlord":{"score":1},"farmer":{"score":2},"alliances":[{"role":1,"alId":"ours"},{"role":2,"alId":"a","abbr":"A"},{"role":2,"alId":"b","abbr":"B"}],"ranks":[{"allianceId":"ours","uid":"7","score":5}]}}"#;
        let battles = parse_canyon_result(contents, 1).unwrap();
        assert_eq!(battles.len(), 1);
        let b = &battles[0];
        assert!(!b.won);
        assert_eq!((b.score, b.enemy_score), (1, 2));
        assert_eq!(b.enemy_abbr, "A+B");
    }

    #[test]
    fn a_canyon_player_who_cant_be_placed_makes_every_battle_incomplete() {
        // One entry has no alliance, so it could be either alliance's player.
        let contents = CANYON.replace(r#""allianceId":"ally","#, "");
        let battles = parse_canyon_result(&contents, 1).unwrap();
        assert!(battles.iter().all(|b| !b.players_complete));
        assert_eq!(battles[1].players.len(), 2, "ours are all still there");
        // A side the mail doesn't know is no result.
        let contents = CANYON.replace(r#""role":2,"isWin""#, r#""role":3,"isWin""#);
        assert_eq!(parse_canyon_result(&contents, 1), None);
    }

    fn battle(secs: u64, score: i64) -> Battle {
        Battle {
            event: Event::DesertStorm,
            time: Duration::from_secs(secs),
            won: true,
            alliance_id: "ours".into(),
            score,
            enemy_abbr: "OPP".into(),
            enemy_name: "Opponents".into(),
            enemy_score: 250_000,
            players: Vec::new(),
            players_complete: true,
        }
    }

    #[test]
    fn copies_of_one_battle_match_but_other_weeks_do_not() {
        let friday = 1_790_940_621;
        assert!(
            same_battle(&battle(friday, 300_000), &battle(friday + 7, 300_000)),
            "another account's copy"
        );
        assert!(
            !same_battle(
                &battle(friday, 300_000),
                &battle(friday + 7 * 86_400, 300_000)
            ),
            "same scores a week later"
        );
        assert!(
            !same_battle(&battle(friday, 300_000), &battle(friday + 7, 300_001)),
            "different score"
        );
    }
}
