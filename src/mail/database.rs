//! Desert Storm results from the game's local mail database, `config.db` (SQLite).
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

use crate::game::battle::DsBattle;
use crate::mail::sqlite::{Database, SqlValue};
use crate::util::json::{self, Json};

/// Mail `type` of a Desert Storm battle result.
const DS_RESULT_TYPE: i64 = 109;

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
    Err("the game is saving its mail; try again in a moment".into())
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

/// Every Desert Storm result mail in the database, one per battle. Several accounts on one PC
/// can each hold a copy of the same battle's mail: copies share the alliance, opponent and both
/// scores, and arrive within [`SAME_BATTLE_WINDOW`] of each other.
pub fn ds_battles(db_bytes: &[u8]) -> Result<Vec<DsBattle>, String> {
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
    let mut battles: Vec<DsBattle> = Vec::new();
    db.for_each_row(table.root_page, |row| {
        if !matches!(row.get(type_col), Some(SqlValue::Int(DS_RESULT_TYPE))) {
            return Ok(());
        }
        let Some(&SqlValue::Int(created)) = row.get(time_col) else {
            return Ok(());
        };
        let contents = match row.get(contents_col) {
            Some(SqlValue::Text(s)) => s.clone(),
            Some(SqlValue::Blob(b)) => String::from_utf8_lossy(b).into_owned(),
            _ => return Ok(()),
        };
        if let Some(battle) = parse_result(&contents, created)
            && !battles.iter().any(|b| same_battle(b, &battle))
        {
            battles.push(battle);
        }
        Ok(())
    })?;
    battles.sort_by_key(|b| b.time);
    Ok(battles)
}

fn same_battle(a: &DsBattle, b: &DsBattle) -> bool {
    a.alliance_id == b.alliance_id
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

/// `contents` of a result mail: `{"b":{...},"obj":{"result","alliances":[ours, theirs],"ranks":[...]}}`.
fn parse_result(contents: &str, created_ms: i64) -> Option<DsBattle> {
    let doc = json::parse(contents).ok()?;
    let obj = doc.get("obj")?;
    let alliances = obj.get("alliances")?.as_array()?;
    let (ours, theirs) = (alliances.first()?, alliances.get(1)?);
    // JSON numbers are parsed as f64; only whole numbers it holds exactly (up to 2^53) are used.
    let num = |v: &Json, key: &str| match v.get(key) {
        Some(Json::Number(n)) if n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_992.0 => {
            Some(*n as i64)
        }
        _ => None,
    };
    let text = |v: &Json, key: &str| {
        v.get(key)
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string()
    };
    // An entry without a readable uid and score is skipped, and the list is marked incomplete.
    let ranks = obj.get("ranks")?.as_array()?;
    let players: Vec<(String, i64)> = ranks
        .iter()
        .filter_map(|r| Some((r.get("uid")?.as_str()?.to_string(), num(r, "score")?)))
        .collect();
    Some(DsBattle {
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

    fn battle(secs: u64, score: i64) -> DsBattle {
        DsBattle {
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
