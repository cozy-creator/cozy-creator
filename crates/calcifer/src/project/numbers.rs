//! A project's runs are numbered 1, 2, 3… in the order they were made (cl-1075): `KEY-N` is run N of its project. A run's id is its
//! identity and its number its name there, which project.db also keeps beside what the run made: each such number moves with it.
use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use serde_json::Value;

/// A run number project.db keeps outside `runs`: the number, and the run's id where it keeps that too.
struct Label {
    number: i64,
    id: Option<String>,
}

/// What becomes of a label.
enum Fate {
    Keep,
    Number(i64),
    /// It names no run here and would name another: its version keeps its run's id alone.
    Clear,
    Refuse(String),
}

/// Why the numbers can't move, never a project's names or words: the table and what didn't hold.
pub(crate) type Refused = String;

fn sql(error: rusqlite::Error) -> Refused {
    format!("sqlite: {error}")
}

/// The project's runs are 1..n.
pub(crate) fn numbered(conn: &Connection) -> rusqlite::Result<bool> {
    let (count, low, high): (i64, i64, i64) = conn.query_row("SELECT COUNT(*), COALESCE(MIN(number), 1), COALESCE(MAX(number), 0) FROM runs", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    Ok(count == 0 || (low == 1 && high == count))
}

/// The project's runs renumbered 1..n in the order made, each number project.db keeps of them moved with them, and the answers it
/// kept for retried requests dropped, as they name runs by their numbers. Checked before the caller commits: every moved number
/// names the run it named, as many as before, and the runs are 1..n. Ok(false): they were 1..n already. Err: the caller rolls back.
pub(crate) fn renumber(tx: &Transaction<'_>) -> Result<bool, Refused> {
    if numbered(tx).map_err(sql)? { return Ok(false); }
    let mut statement = tx.prepare("SELECT id, number FROM runs ORDER BY created_at, number").map_err(sql)?;
    let runs: Vec<(String, i64)> = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).map_err(sql)?.collect::<rusqlite::Result<_>>().map_err(sql)?;
    drop(statement);
    let count = runs.len() as i64;
    let by_id: HashMap<&str, (i64, i64)> = runs.iter().zip(1..).map(|((id, old), new)| (id.as_str(), (*old, new))).collect();
    let by_old: HashMap<i64, i64> = by_id.values().copied().collect();
    let dangling = |site: &str, number: i64, optional: bool| match (1..=count).contains(&number) {
        false => Fate::Keep,
        true if optional => Fate::Clear,
        true => Fate::Refuse(format!("{site}: run {number} isn't here, and would name run {number} of 1..{count}")),
    };
    let mut moved = 0usize;
    visit(tx, |site, label, optional| {
        let fate = match &label.id {
            Some(id) => match by_id.get(id.as_str()) {
                Some((old, new)) if *old == label.number => Fate::Number(*new),
                Some((old, _)) => Fate::Refuse(format!("{site}: names run {} by its number and run {old} by its id", label.number)),
                None => dangling(site, label.number, optional),
            },
            None => by_old.get(&label.number).map_or_else(|| dangling(site, label.number, optional), |new| Fate::Number(*new)),
        };
        if matches!(fate, Fate::Number(_)) { moved += 1; }
        fate
    })?;
    tx.execute("UPDATE runs SET number = -number", []).map_err(sql)?;
    let mut set = tx.prepare("UPDATE runs SET number = ?2 WHERE id = ?1").map_err(sql)?;
    for (id, (_, new)) in &by_id { set.execute(params![id, new]).map_err(sql)?; }
    drop(set);
    tx.execute("DELETE FROM request_keys", []).map_err(sql)?;
    check(tx, &runs, moved)?;
    Ok(true)
}

/// What `renumber` left, read again before it commits.
fn check(tx: &Transaction<'_>, runs: &[(String, i64)], moved: usize) -> Result<(), Refused> {
    let count = runs.len() as i64;
    if !numbered(tx).map_err(sql)? || tx.query_row("SELECT COUNT(*) FROM runs", [], |row| row.get::<_, i64>(0)).map_err(sql)? != count {
        return Err(format!("runs: not 1..{count} once moved"));
    }
    let mut number = tx.prepare("SELECT number FROM runs WHERE id = ?1").map_err(sql)?;
    let mut now: HashMap<String, i64> = HashMap::new();
    for ((id, _), new) in runs.iter().zip(1..) {
        let held: Option<i64> = number.query_row([id], |row| row.get(0)).optional().map_err(sql)?;
        if held != Some(new) { return Err(format!("runs: a run is {held:?} once moved, not {new}")); }
        now.insert(id.clone(), new);
    }
    drop(number);
    let mut named = 0usize;
    visit(tx, |site, label, _| match &label.id {
        Some(id) => match now.get(id) {
            Some(new) if *new == label.number => { named += 1; Fate::Keep }
            Some(new) => Fate::Refuse(format!("{site}: names run {} once moved, and its run is {new}", label.number)),
            None if (1..=count).contains(&label.number) => Fate::Refuse(format!("{site}: run {} isn't here once moved", label.number)),
            None => Fate::Keep,
        },
        None if (1..=count).contains(&label.number) => { named += 1; Fate::Keep }
        None => Fate::Keep,
    })?;
    if named != moved { return Err(format!("labels: {moved} moved, {named} name a run once moved")); }
    Ok(())
}

/// Run `id`, numbered `old` in Calcifer's old store, imported as run `new` here (cl-1075): each number project.db keeps of it moves
/// with it. One without the id moves only when `old` names no other run here.
pub(crate) fn imported(tx: &Transaction<'_>, id: &str, old: i64, new: i64) -> Result<(), Refused> {
    if old == new { return Ok(()); }
    let taken = tx.query_row("SELECT EXISTS(SELECT 1 FROM runs WHERE number = ?1 AND id <> ?2)", params![old, id], |row| row.get::<_, bool>(0)).map_err(sql)?;
    visit(tx, |_, label, _| match &label.id {
        Some(held) if held == id => Fate::Number(new),
        None if label.number == old && !taken => Fate::Number(new),
        _ => Fate::Keep,
    })
}

/// Each run number project.db keeps outside `runs`, by where it is (the table) and whether it may go (a version's may: its id stays),
/// given to `fate`; what it says of each is written. Err: one it refused.
fn visit(tx: &Transaction<'_>, mut fate: impl FnMut(&str, &Label, bool) -> Fate) -> Result<(), Refused> {
    let mut refused = None;
    let mut decide = |site: &str, label: Label, optional: bool| -> Fate {
        match fate(site, &label, optional) {
            Fate::Refuse(why) => {
                if refused.is_none() { refused = Some(why); }
                Fate::Keep
            }
            kept => kept,
        }
    };
    // A version's own number and the run that made it, in its record.
    let versions: Vec<(String, i64, String)> = rows(tx, "SELECT key, number, record FROM versions")?;
    for (key, number, record) in versions {
        let mut value = parse(&record, "versions")?;
        if labelled(&mut value, "run", "run_id", true, &mut |label, optional| decide("versions", label, optional)) {
            tx.execute("UPDATE versions SET record = ?3 WHERE key = ?1 AND number = ?2", params![key, number, value.to_string()]).map_err(sql)?;
        }
    }
    // What `cozy run`s left here: a record each, and the same row by its number alone.
    let files: Vec<(i64, String)> = rows(tx, "SELECT ord, record FROM lock_runs")?;
    let mut by_ord = HashMap::new();
    for (ord, record) in files {
        let mut value = parse(&record, "lock_runs")?;
        let was = value.get("run").and_then(Value::as_i64);
        if labelled(&mut value, "run", "run_id", false, &mut |label, optional| decide("lock_runs", label, optional)) {
            tx.execute("UPDATE lock_runs SET record = ?2 WHERE ord = ?1", params![ord, value.to_string()]).map_err(sql)?;
        }
        if let Some(was) = was { by_ord.insert(ord, (was, value.get("run").and_then(Value::as_i64).unwrap_or(was))); }
    }
    let files: Vec<(i64, i64)> = rows(tx, "SELECT ord, run FROM run_files")?;
    for (ord, run) in files {
        let now = match by_ord.get(&ord) {
            Some((was, now)) if *was == run => *now,
            _ => match decide("run_files", Label { number: run, id: None }, false) { Fate::Number(now) => now, _ => run },
        };
        if now != run { tx.execute("UPDATE run_files SET run = ?2 WHERE ord = ?1", params![ord, now]).map_err(sql)?; }
    }
    // The walk: each run it sent and follows, by number, and what each landed so far.
    let walk: Vec<(String,)> = rows(tx, "SELECT record FROM walk WHERE id = 1")?;
    if let Some((record,)) = walk.into_iter().next() {
        let mut value = parse(&record, "walk")?;
        let mut changed = false;
        if let Some(pending) = value.get_mut("pending").and_then(Value::as_object_mut) {
            for held in pending.values_mut() {
                // 0: not sent yet.
                if held.get("run").and_then(Value::as_i64).is_some_and(|run| run > 0) {
                    changed |= labelled(held, "run", "", false, &mut |label, optional| decide("walk", label, optional));
                }
                if let Some(version) = held.get_mut("version") {
                    changed |= labelled(version, "run", "run_id", true, &mut |label, optional| decide("walk", label, optional));
                }
            }
        }
        if let Some(owed) = value.get_mut("walk") {
            if let Some(runs) = owed.get_mut("runs").and_then(Value::as_object_mut) {
                let held = std::mem::take(runs);
                for (number, coming) in held {
                    let key = match number.parse::<i64>() {
                        Ok(run) => match decide("walk", Label { number: run, id: None }, false) {
                            Fate::Number(now) => { changed = true; now.to_string() }
                            _ => number,
                        },
                        Err(_) => number,
                    };
                    runs.insert(key, coming);
                }
            }
            if let Some(landing) = owed.get_mut("landing").and_then(Value::as_array_mut) {
                for file in landing {
                    changed |= labelled(file, "run", "run_id", false, &mut |label, optional| decide("walk", label, optional));
                }
            }
        }
        if changed { tx.execute("UPDATE walk SET record = ?1 WHERE id = 1", [value.to_string()]).map_err(sql)?; }
    }
    // A failed take's run, as an asset's record kept it before cl-1070.
    let assets: Vec<(String, String)> = rows(tx, "SELECT key, record FROM lock_assets")?;
    for (key, record) in assets {
        let mut value = parse(&record, "lock_assets")?;
        let mut changed = false;
        if let Some(ended) = value.get_mut("ended").and_then(Value::as_array_mut) {
            for take in ended {
                changed |= labelled(take, "run", "", true, &mut |label, optional| decide("lock_assets", label, optional));
            }
        }
        if changed { tx.execute("UPDATE lock_assets SET record = ?2 WHERE key = ?1", params![key, value.to_string()]).map_err(sql)?; }
    }
    refused.map_or(Ok(()), Err)
}

/// The label at `number` in `value` (its run's id at `id`, when named), as `fate` says: whether it changed.
fn labelled(value: &mut Value, number: &str, id: &str, optional: bool, fate: &mut impl FnMut(Label, bool) -> Fate) -> bool {
    let Some(held) = value.get(number).and_then(Value::as_i64) else { return false };
    let run_id = (!id.is_empty()).then(|| value.get(id).and_then(Value::as_str).map(str::to_owned)).flatten();
    let Some(object) = value.as_object_mut() else { return false };
    match fate(Label { number: held, id: run_id }, optional) {
        Fate::Number(now) if now != held => { object.insert(number.to_owned(), Value::from(now)); true }
        Fate::Clear => { object.remove(number); true }
        _ => false,
    }
}

fn parse(record: &str, site: &str) -> Result<Value, Refused> {
    serde_json::from_str(record).map_err(|_| format!("{site}: a record isn't JSON"))
}

fn rows<T: FromRow>(tx: &Transaction<'_>, query: &str) -> Result<Vec<T>, Refused> {
    let mut statement = tx.prepare(query).map_err(sql)?;
    statement.query_map([], T::from_row).map_err(sql)?.collect::<rusqlite::Result<_>>().map_err(sql)
}

trait FromRow: Sized {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self>;
}
impl<A: rusqlite::types::FromSql> FromRow for (A,) {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> { Ok((row.get(0)?,)) }
}
impl<A: rusqlite::types::FromSql, B: rusqlite::types::FromSql> FromRow for (A, B) {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> { Ok((row.get(0)?, row.get(1)?)) }
}
impl<A: rusqlite::types::FromSql, B: rusqlite::types::FromSql, C: rusqlite::types::FromSql> FromRow for (A, B, C) {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> { Ok((row.get(0)?, row.get(1)?, row.get(2)?)) }
}
