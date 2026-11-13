//! Native admission of genuinely project-independent package source.
//! A caller-supplied digest or LocalCode.at is never evidence of ownership.
use std::{collections::HashMap, io::Write as _, path::{Path,PathBuf}, sync::Arc};
use api::{code::LocalCode, error::{Error,Kind}, ids::Digest};
use axum::{Json, extract::State, http::HeaderMap};
use rusqlite::{OptionalExtension as _,params};
use crate::api::{Api,Refusal};

pub(super) const TABLES: &str="CREATE TABLE IF NOT EXISTS maintenance_source_admissions(tree TEXT PRIMARY KEY) STRICT;
CREATE TABLE IF NOT EXISTS maintenance_source_objects(digest TEXT PRIMARY KEY) STRICT;";
static OWNERSHIP: std::sync::LazyLock<tokio::sync::RwLock<()>> = std::sync::LazyLock::new(||tokio::sync::RwLock::new(()));

/// Creation changes an unknown folder into a project. Serialize that boundary
/// against independent source capture before the first project marker is made.
pub(crate) fn begin_project_creation() -> Result<tokio::sync::RwLockWriteGuard<'static,()>,Error> {
    OWNERSHIP.try_write().map_err(|_|Error::new(Kind::Conflict,"source_capture_busy","a source snapshot is being captured; retry project creation after it finishes"))
}

#[derive(serde::Deserialize)]
pub(crate) struct Capture { path: PathBuf }

fn refused(code: &'static str) -> Refusal {
    Refusal(Error::new(Kind::Validation,code,"independent source admission could not verify the complete source ownership"))
}

fn independent(paths: &[PathBuf]) -> Result<(),Refusal> {
    for path in paths {
        if crate::project::source_project(path).map_err(Refusal)?.is_some() {
            return Err(refused("source_project_owned"));
        }
    }
    Ok(())
}

pub(crate) fn require(api: &Api, code: &LocalCode) -> Result<(),Error> {
    if Path::new(&code.at).is_absolute() || Path::new(&code.at).components().any(|part| !matches!(part,std::path::Component::Normal(_))) && !code.at.is_empty() {
        return Err(refused("source_scope_required").0);
    }
    let admitted=api.calcifer.maintenance_store().read(|conn| Ok(conn.query_row("SELECT tree FROM maintenance_source_admissions WHERE tree=?1",[code.tree.to_string()],|row|row.get::<_,String>(0)).optional()?.is_some()))
        .map_err(|_|refused("source_admission_unavailable").0)?;
    if !admitted { return Err(refused("source_scope_required").0); }
    Ok(())
}

/// Cache entrypoints without an Api still verify the controller's durable
/// independent-source admission before reading or materializing global code.
pub(crate) fn require_home(home:&platform::paths::Home, code:&LocalCode)->Result<(),Error> {
    if Path::new(&code.at).is_absolute() || (!code.at.is_empty() && Path::new(&code.at).components().any(|part|!matches!(part,std::path::Component::Normal(_)))) { return Err(refused("source_scope_required").0); }
    let conn=rusqlite::Connection::open_with_flags(home.calcifer().join("state.db"),rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY|rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|_|refused("source_admission_unavailable").0)?;
    let admitted=conn.query_row("SELECT EXISTS(SELECT 1 FROM maintenance_source_admissions WHERE tree=?1)",[code.tree.to_string()],|row|row.get::<_,bool>(0)).map_err(|_|refused("source_admission_unavailable").0)?;
    if admitted { Ok(()) } else { Err(refused("source_scope_required").0) }
}

/// Whether `digest` is an object derived from admitted independent source (`keep_derived`).
pub(crate) fn admitted_object(store: &crate::store::Store, digest: &str) -> bool {
    store.read(|conn| Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM maintenance_source_objects WHERE digest=?1)", [digest], |row| row.get::<_, bool>(0))?))
        .unwrap_or(false)
}

/// A derived manifest belongs globally only when its original exact tree was
/// admitted as independent, and every materialized byte still verifies.
pub(crate) fn keep_derived(api:&Api,code:&LocalCode,source:&crate::local_code::Source)->Result<(),Error> {
    if crate::private_io::current().is_some() { return Ok(()); }
    require(api,code)?;
    let paths=source.inputs.iter().map(|input|input.path.parent().unwrap_or(&input.path).to_owned()).collect::<Vec<_>>();
    independent(&paths).map_err(|error|error.0)?;
    for input in &source.inputs {
        let bytes=std::fs::read(&input.path).map_err(|_|refused("source_snapshot_changed").0)?;
        if bytes.len() as u64!=input.length || Digest::of(&bytes)!=input.digest { return Err(refused("source_snapshot_changed").0); }
    }
    api.calcifer.maintenance_store().write(|tx| {for input in &source.inputs {tx.execute("INSERT OR IGNORE INTO maintenance_source_objects(digest) VALUES(?1)",[input.digest.to_string()])?;} Ok(())}).map_err(|_|refused("source_admission_unavailable").0)
}

pub(crate) async fn capture(State(api): State<Arc<Api>>, headers: HeaderMap, Json(ask): Json<Capture>) -> Result<Json<LocalCode>,Refusal> {
    if !crate::api::bearer(&headers).is_some_and(|token| api.admits(token)) {
        return Err(Refusal(Error::new(Kind::Credential,"native_holder_required","source capture requires this daemon's native holder")));
    }
    if !ask.path.is_absolute() { return Err(refused("source_absolute_required")); }
    let _ownership=OWNERSHIP.read().await;
    let plan=api::code::plan(&ask.path).map_err(Refusal)?;
    // Check every actual package/dependency root, including empty dependencies,
    // and every file's parent before hashing, digest memo, staging or global writes.
    let mut paths:Vec<_>=plan.roots().map(Path::to_owned).collect();
    paths.extend(plan.files().map(|file|file.parent().unwrap_or(file).to_owned()));
    paths.sort(); paths.dedup();
    independent(&paths)?;
    let staged=plan.stage(None).map_err(Refusal)?;
    let code=staged.code();
    let members=api::tree::parse(&staged.document).map_err(|_|refused("source_snapshot_changed"))?;
    let mut captured=HashMap::new();
    for file in &staged.files {
        let path=std::fs::canonicalize(&file.path).map_err(|_|refused("source_snapshot_changed"))?;
        if path!=file.path { return Err(refused("source_snapshot_changed")); }
        let bytes=std::fs::read(&path).map_err(|_|refused("source_snapshot_changed"))?;
        if bytes.len() as u64!=file.length || Digest::of(&bytes)!=file.digest { return Err(refused("source_snapshot_changed")); }
        captured.insert(file.digest,bytes);
    }
    independent(&paths)?;
    // Hash the captured snapshot's independent tree document again; every leaf is
    // matched to the exact captured bytes before any durable global registration.
    if Digest::of(&staged.document)!=code.tree || members.values().any(|member| {
        let Ok(digest)=format!("sha256:{}",member.sha256).parse::<Digest>() else { return true; };
        captured.get(&digest).is_none_or(|bytes| bytes.len() as u64!=member.length || Digest::of(bytes)!=digest)
    }) { return Err(refused("source_snapshot_changed")); }
    let home=api.machines.hubs.home();
    let temporary=tempfile::Builder::new().prefix("maintenance-source-").tempdir_in(home.calcifer()).map_err(|_|refused("source_capture_unavailable"))?;
    let mut files=HashMap::new();
    for (digest,bytes) in captured {
        let path=temporary.path().join(hex::encode(digest.bytes()));
        let mut file=std::fs::OpenOptions::new().create_new(true).write(true).open(&path).map_err(|_|refused("source_capture_unavailable"))?;
        file.write_all(&bytes).and_then(|()|file.sync_all()).map_err(|_|refused("source_capture_unavailable"))?;
        files.insert(digest,path);
    }
    crate::local_code::materialize(home,&code.tree,&members,|digest|files.get(digest).cloned()).map_err(Refusal)?;
    let copied=crate::local_code::copy_of(home,&code).map_err(Refusal)?.join("tree");
    for (name,member) in &members {
        let path=copied.join(name);
        let meta=std::fs::symlink_metadata(&path).map_err(|_|refused("source_snapshot_changed"))?;
        if !meta.is_file() { return Err(refused("source_snapshot_changed")); }
        let bytes=std::fs::read(&path).map_err(|_|refused("source_snapshot_changed"))?;
        if bytes.len() as u64!=member.length || hex::encode(Digest::of(&bytes).bytes())!=member.sha256 { return Err(refused("source_snapshot_changed")); }
    }
    api.calcifer.maintenance_store().write(|tx| { tx.execute("INSERT OR IGNORE INTO maintenance_source_admissions(tree) VALUES(?1)",params![code.tree.to_string()])?; Ok(()) })
        .map_err(|_|refused("source_admission_unavailable"))?;
    Ok(Json(code))
}

/// A package of the dev home's packages checkout (xs-195) read where no project is: Calcifer's own first-party source, named by its
/// config, so it is admitted as independent source is when captured (`capture`): no project owns any of its files, its copy is
/// whole and verified, and its tree is recorded admitted. It is then read, described and run as the Hub's packages are.
pub(crate) fn admit_checkout(api: &Api, dir: &Path) -> Result<LocalCode, Error> {
    let _ownership = OWNERSHIP.try_read().map_err(|_| Error::new(Kind::Conflict, "source_capture_busy", "a project is being created; ask again"))?;
    let home = api.machines.hubs.home();
    let memo = home.root().join("cache").join("digests");
    let staged = crate::checkout::stage(dir, Some(memo.as_path()), &home.calcifer().join("patched"))?;
    let mut paths: Vec<PathBuf> = staged.files.iter().map(|file| file.path.parent().unwrap_or(&file.path).to_owned()).collect();
    paths.push(dir.to_owned());
    paths.sort();
    paths.dedup();
    independent(&paths).map_err(|refusal| refusal.0)?;
    let code = staged.code();
    let members = api::tree::parse(&staged.document).map_err(|_| refused("source_snapshot_changed").0)?;
    let copied = crate::local_code::copy_of(home, &code)?.join("tree");
    if !copied.is_dir() {
        let from = |digest: &Digest| staged.files.iter().find(|file| file.digest == *digest).map(|file| file.path.clone());
        crate::local_code::materialize(home, &code.tree, &members, from)?;
    }
    for (name, member) in &members {
        let bytes = std::fs::read(copied.join(name)).map_err(|_| refused("source_snapshot_changed").0)?;
        if bytes.len() as u64 != member.length || hex::encode(Digest::of(&bytes).bytes()) != member.sha256 {
            return Err(refused("source_snapshot_changed").0);
        }
    }
    api.calcifer.maintenance_store().write(|tx| { tx.execute("INSERT OR IGNORE INTO maintenance_source_admissions(tree) VALUES(?1)", params![code.tree.to_string()])?; Ok(()) })
        .map_err(|_| refused("source_admission_unavailable").0)?;
    Ok(code)
}
