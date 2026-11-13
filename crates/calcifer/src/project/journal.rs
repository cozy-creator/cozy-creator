//! Project-owned controller records, on the admitted project's existing SQLite connection.
use std::{path::PathBuf, sync::{Arc, Weak}};
use api::error::Error;
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use tokio::sync::OwnedRwLockReadGuard;
use crate::{api::Api, store::{self, Answer, StoreError}};
use super::{Project, Projects, vault};

/// The sole durable outside reference. Activity and display data belong to project.db.
#[derive(Clone, Debug)]
pub(crate) struct ProjectRef {
    pub id: String,
    pub path: PathBuf,
    pub code: String,
}

/// A bounded operation on one verified mounted owner. Never retain across a worker wait.
pub(crate) struct ProjectStore {
    project: Arc<Project>,
    controller: Weak<crate::Calcifer>,
    _view: OwnedRwLockReadGuard<()>,
    access: Access,
}
enum Access { Viewing, Work }

#[derive(Clone)]
pub(crate) struct RunOwner {
    project: Arc<Project>,
    controller: Weak<crate::Calcifer>,
}
/// A project as Calcifer's registry holds it, never what keeps it open: an unended run's route, a request or a view does.
#[derive(Clone)]
pub(crate) struct OpenProject {
    project: Weak<Project>,
    controller: Weak<crate::Calcifer>,
    /// This open's attach (`Calcifer::attach_project`): its unended runs routed, driven and exported.
    pub(crate) attached: bool,
    /// Attached once in this Calcifer's life (its legacy history checked): it may open again by itself once it closed.
    pub(crate) known: bool,
}
impl OpenProject {
    pub(crate) fn owner(&self) -> Option<RunOwner> { Some(RunOwner { project: self.project.upgrade()?, controller: self.controller.clone() }) }
    pub(crate) fn is(&self, owner: &RunOwner) -> bool { std::ptr::eq(self.project.as_ptr(), Arc::as_ptr(&owner.project)) }
}

impl RunOwner {
    pub(super) fn of(project: &Arc<Project>, calcifer: &Arc<crate::Calcifer>) -> Self { Self { project: project.clone(), controller: Arc::downgrade(calcifer) } }
    pub(crate) fn open(&self, attached: bool, known: bool) -> OpenProject { OpenProject { project: Arc::downgrade(&self.project), controller: self.controller.clone(), attached, known } }
    pub(crate) fn same_view(&self, other:&Self)->bool { Arc::ptr_eq(&self.project,&other.project) }
    pub(crate) fn project_id(&self) -> &str { &self.project.id }
    pub(crate) fn root(&self) -> &std::path::Path { &self.project.root }
    pub(crate) fn store(&self) -> store::Result<ProjectStore> {
        let view = vault::try_admit_work(&self.project).map_err(project_error)?;
        Ok(ProjectStore { project: self.project.clone(), controller: self.controller.clone(), _view: view, access:Access::Work })
    }
}

pub(crate) struct SealingStore<'a> {
    project: Arc<Project>,
    controller: Weak<crate::Calcifer>,
    transition: &'a vault::ProjectTransition,
}
impl SealingStore<'_> {
    pub(crate) fn keep_worker_migration_receipt(&self,machine:&str,receipt:&api::machine::ProjectExecutionMigrationReceipt)->store::Result<usize> {self.write(|tx|keep_worker_receipt(tx,self.project_id(),machine,receipt))}
    pub(crate) fn owner_id(&self)->Option<&str> {Some(&self.project.id)}
    pub(crate) fn legacy_worker_pending(&self)->store::Result<bool> { self.read(|conn|Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM runs WHERE owner=?1 AND custody='legacy_pending')",[self.project_id()],|row|row.get(0))?)) }
    pub(crate) fn legacy_worker_requests(&self)->store::Result<Vec<(String,String,Option<u64>)>> {
        self.read(|conn| {let mut stmt=conn.prepare("SELECT id,machine,machine_number FROM runs WHERE owner=?1 AND custody='legacy_pending' ORDER BY number")?;Ok(stmt.query_map([self.project_id()],|row|Ok((row.get(0)?,row.get(1)?,row.get::<_,Option<i64>>(2)?.map(|value|value as u64))))?.collect::<rusqlite::Result<_>>()?)})
    }
    pub(crate) fn project_id(&self) -> &str { &self.project.id }
    pub(crate) fn execution_scope(&self) -> (&str,&std::path::Path) { (&self.project.id,&self.project.root) }
    pub(crate) fn input_root(&self) -> store::Result<PathBuf> {
        self.transition.validate(&self.project).map_err(project_error)?;
        let path=self.project.folder.dir.join(".vault-inputs");
        std::fs::create_dir_all(&path).map_err(|_|StoreError::Protected("project_input_storage".into()))?;
        platform::durable::set_mode(&path,0o700).map_err(|_|StoreError::Protected("project_input_storage".into()))?;
        Ok(path)
    }
    pub(crate) fn read<T>(&self, read: impl FnOnce(&Connection) -> store::Result<T>) -> store::Result<T> {
        self.transition.validate(&self.project).map_err(project_error)?;
        self.project.folder.journal_read(read)
    }
    pub(crate) fn write<T>(&self, write: impl FnOnce(&Transaction<'_>) -> store::Result<T>) -> store::Result<T> {
        self.transition.validate(&self.project).map_err(project_error)?;
        self.project.folder.journal_write(write)
    }
    pub(crate) fn admit_submission(&self, _new: &store::NewRun) -> store::Result<()> {
        Err(StoreError::Locked("project_transition".into()))
    }
    pub(crate) fn require_machine(&self, name: &str) -> store::Result<()> {
        let controller = self.controller.upgrade().ok_or_else(|| StoreError::Protected("controller_stopped".into()))?;
        controller.maintenance_store().machine(name)?.ok_or_else(|| StoreError::UnknownMachine(name.into())).map(drop)
    }
    pub(crate) fn keep_machine(&self, machine: &store::MachineRow) -> store::Result<()> {
        let controller = self.controller.upgrade().ok_or_else(|| StoreError::Protected("controller_stopped".into()))?;
        controller.maintenance_store().put_machine(machine)
    }
    /// Machines are global: a project's journal reads whether one is a rental from Calcifer's own list.
    pub(crate) fn rental(&self, _tx: &Transaction<'_>, name: &str) -> store::Result<bool> {
        let controller = self.controller.upgrade().ok_or_else(|| StoreError::Protected("controller_stopped".into()))?;
        Ok(controller.maintenance_store().machine(name)?.is_some_and(|machine| machine.kind == api::calcifer::MachineKind::Rental))
    }
    pub(crate) fn append_diagnostic(&self, code: &'static str, detail: &str) -> store::Result<()> {
        self.write(|tx| { tx.execute("INSERT INTO project_diagnostics(at_ms,code,detail) VALUES(?1,?2,?3)", params![store::now_ms(),code,detail])?; Ok(()) })
    }
}

impl Projects {
    pub(crate) fn reference(&self, api: &Arc<Api>, named: &str) -> Result<ProjectRef, Error> {
        let refs = api.calcifer.maintenance_store().known_projects().map_err(|_| Error::new(api::error::Kind::Unavailable, "project_index", "the project reference index is unavailable"))?;
        if api::target::written_as_path(named.trim()) {
            let root = vault::logical(&super::holding(std::path::Path::new(named.trim()))?);
            if let Some(known) = refs.iter().find(|entry| vault::logical(&entry.path) == root) {
                return Ok(ProjectRef { id: known.id.clone(), path: root, code: known.code.clone() });
            }
            let keybag = project_vault::Keybag::load(&root.join(project_vault::KEYBAG_FILE))
                .map_err(|_| Error::new(api::error::Kind::NotFound, "project_reference_unavailable", "this project has no registered or public encrypted reference"))?;
            let reference = ProjectRef { id: keybag.info().project_id, path: root, code: String::new() };
            // Register only the public reference, never open a mounted view for bootstrap.
            api.calcifer.maintenance_store().write(|tx| {
                tx.execute("INSERT INTO projects(id,path,code) VALUES(?1,?2,'') ON CONFLICT(id) DO UPDATE SET path=excluded.path", params![reference.id, reference.path.to_string_lossy()])?;
                Ok(())
            }).map_err(|_| Error::new(api::error::Kind::Unavailable, "project_index", "the project reference could not be recorded"))?;
            return Ok(reference);
        }
        let mut matches = refs.into_iter().filter(|entry| entry.id == named || entry.code == named || entry.path.file_name().is_some_and(|value| value.to_string_lossy().eq_ignore_ascii_case(named)));
        let entry = matches.next().ok_or_else(|| Error::new(api::error::Kind::NotFound, "project_unknown", "this computer knows no such project reference"))?;
        if matches.next().is_some() { return Err(Error::new(api::error::Kind::Conflict, "project_ambiguous", "use the project's opaque id, code or absolute path")); }
        Ok(ProjectRef { id: entry.id, path: entry.path, code: entry.code })
    }

    pub(super) fn transition_store<'a>(&self, api: &Arc<Api>, named: &str, transition: &'a vault::ProjectTransition) -> Result<SealingStore<'a>, Error> {
        // The held transition validates the view itself. Ordinary viewing (`open_now`)
        // refuses while this same owner converts the storage, so it cannot be used here.
        let open = self.open.lock().expect("projects lock").get(named).and_then(Weak::upgrade)
            .filter(|project| transition.validate(project).is_ok());
        let project = match open { Some(project) => project, None => self.get(api, named).map_err(|error| error.0)? };
        transition.validate(&project)?;
        Ok(SealingStore { project, controller: Arc::downgrade(&api.calcifer), transition })
    }

    pub(crate) async fn store(&self, api: &Arc<Api>, named: &str) -> Result<ProjectStore, Error> {
        let view = vault::admit_project(api, named).await?;
        let project = self.get(api, named).map_err(|error| error.0)?;
        project.require_view()?;
        let mut store = ProjectStore { project, controller: Arc::downgrade(&api.calcifer), _view: view, access:Access::Viewing };
        api.calcifer.maintenance_store().migrate_project(&store, &api.calcifer.inputs).map_err(|_| Error::new(api::error::Kind::Conflict,"project_migration_pending","legacy project history could not be verified; its source is preserved"))?;
        super::unseen::refresh(&store.project)?;
        // A history read never starts a worker. If the registered local worker is
        // already available, resolve actual bindings without retaining this view lease.
        if store.legacy_worker_pending().map_err(|_|Error::new(api::error::Kind::Conflict,"project_migration_pending","legacy custody remains unresolved"))? && api.machines.local.running() {
            if let Ok(selectors)=api.calcifer.maintenance_store().verify_legacy_worker_requests(&store) {
                if !selectors.is_empty() && selectors.iter().all(|(_,machine,_)|machine=="local") {
                    if let Ok(scope)=crate::private_io::scope_for_project(store.project_id(),store.execution_scope().1) {
                        let owner=store.owner();
                        let _activity=self.track_request(api,store.execution_scope().1);
                        drop(store);
                        if let Ok(receipt)=super::legacy_worker::migrate(api,scope,&selectors).await {
                            let work=owner.store().map_err(|_|Error::new(api::error::Kind::Conflict,"project_migration_pending","the original view changed during worker migration"))?;
                            work.keep_worker_migration_receipt("local",&receipt).map_err(|_|Error::new(api::error::Kind::Conflict,"project_migration_pending","worker coverage could not be recorded"))?;
                            api.calcifer.maintenance_store().migrate_project(&work,&api.calcifer.inputs).map_err(|_|Error::new(api::error::Kind::Conflict,"project_migration_pending","verified source retirement remains pending"))?;
                            let fresh=api.calcifer.attach_project(&work).map_err(|_|Error::new(api::error::Kind::Conflict,"project_migration_pending","verified work could not be resumed"))?;
                            if fresh {tokio::spawn(crate::hub::jobs::resume_project(api.clone(),work.project_id().into()));}
                        }
                        let view=vault::admit_project(api,named).await?;
                        let project=self.get(api,named).map_err(|error|error.0)?;
                        store=ProjectStore {project,controller:Arc::downgrade(&api.calcifer),_view:view,access:Access::Viewing};
                    }
                }
            }
        }
        let fresh=api.calcifer.attach_project(&store).map_err(|_| Error::new(api::error::Kind::Unavailable, "project_journal", "the project journal is unavailable"))?;
        if fresh {
            tokio::spawn(crate::hub::jobs::resume_project(api.clone(),store.project_id().into()));
            if store.resume_allowed().map_err(|_|Error::new(api::error::Kind::Conflict,"project_journal","the project journal is unavailable"))? && store.project.folder.lock()?.owes() { super::walk(api,&store.project); }
        }
        Ok(store)
    }

    /// Currently accessible owners only; no locked project is opened for this snapshot.
    pub(crate) fn accessible(&self) -> Vec<String> {
        let mut ids: Vec<_> = self.open.lock().expect("projects lock").values().filter_map(|project| {
            let project = project.upgrade()?;
            project.require_view().ok()?;
            Some(project.id.clone())
        }).collect();
        ids.sort();
        ids
    }
}

impl ProjectStore {
    pub(crate) fn keep_worker_migration_receipt(&self,machine:&str,receipt:&api::machine::ProjectExecutionMigrationReceipt)->store::Result<usize> {self.write(|tx|keep_worker_receipt(tx,self.project_id(),machine,receipt))}
    fn validate(&self)->store::Result<()> {
        match self.access {
            Access::Viewing=>self.project.require_view().map_err(project_error),
            Access::Work=>vault::try_admit_work(&self.project).map(drop).map_err(project_error),
        }
    }
    pub(crate) fn admit_new_work(&self)->store::Result<()> {self.project.require_view().map_err(project_error)?;if self.legacy_worker_pending()? {return Err(StoreError::Protected("legacy_worker_custody_pending".into()));} Ok(())}
    pub(crate) fn owner_id(&self)->Option<&str> {Some(&self.project.id)}
    pub(crate) fn resume_allowed(&self)->store::Result<bool> { if self.legacy_worker_pending()? {return Ok(false);} self.read(|conn|Ok(!conn.query_row("SELECT EXISTS(SELECT 1 FROM runs WHERE owner<>?1)",[self.project_id()],|row|row.get::<_,bool>(0))?)) }
    pub(crate) fn legacy_worker_pending(&self)->store::Result<bool> { self.read(|conn|Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM runs WHERE owner=?1 AND custody='legacy_pending')",[self.project_id()],|row|row.get(0))?)) }
    pub(crate) fn legacy_worker_requests(&self)->store::Result<Vec<(String,String,Option<u64>)>> {
        self.read(|conn| {let mut stmt=conn.prepare("SELECT id,machine,machine_number FROM runs WHERE owner=?1 AND custody='legacy_pending' ORDER BY number")?;Ok(stmt.query_map([self.project_id()],|row|Ok((row.get(0)?,row.get(1)?,row.get::<_,Option<i64>>(2)?.map(|value|value as u64))))?.collect::<rusqlite::Result<_>>()?)})
    }
    pub(crate) fn project_id(&self) -> &str { &self.project.id }
    pub(crate) fn owner(&self) -> RunOwner { RunOwner { project: self.project.clone(), controller: self.controller.clone() } }
    pub(crate) fn input_root(&self) -> store::Result<PathBuf> {
        self.validate()?;
        let path=self.project.folder.dir.join(".vault-inputs");
        std::fs::create_dir_all(&path).map_err(|_| StoreError::Protected("project_input_storage".into()))?;
        platform::durable::set_mode(&path,0o700).map_err(|_| StoreError::Protected("project_input_storage".into()))?;
        Ok(path)
    }
    pub(crate) fn execution_scope(&self) -> (&str, &std::path::Path) { (&self.project.id, &self.project.root) }


    pub(crate) fn admit_submission(&self, new: &store::NewRun) -> store::Result<()> {
        self.admit_new_work()?;
        if new.project.as_deref() != Some(self.project.id.as_str()) || new.out_dir != super::landing(&self.project.root) {
            return Err(StoreError::Protected("project_submission_scope".into()));
        }
        Ok(())
    }
    pub(crate) fn require_machine(&self, name: &str) -> store::Result<()> {
        let controller = self.controller.upgrade().ok_or_else(|| StoreError::Protected("controller_stopped".into()))?;
        controller.maintenance_store().machine(name)?.ok_or_else(|| StoreError::UnknownMachine(name.into())).map(drop)
    }
    pub(crate) fn keep_machine(&self, machine: &store::MachineRow) -> store::Result<()> {
        let controller = self.controller.upgrade().ok_or_else(|| StoreError::Protected("controller_stopped".into()))?;
        controller.maintenance_store().put_machine(machine)
    }
    /// Machines are global: a project's journal reads whether one is a rental from Calcifer's own list.
    pub(crate) fn rental(&self, _tx: &Transaction<'_>, name: &str) -> store::Result<bool> {
        let controller = self.controller.upgrade().ok_or_else(|| StoreError::Protected("controller_stopped".into()))?;
        Ok(controller.maintenance_store().machine(name)?.is_some_and(|machine| machine.kind == api::calcifer::MachineKind::Rental))
    }


    pub(crate) fn read<T>(&self, read: impl FnOnce(&Connection) -> store::Result<T>) -> store::Result<T> {
        self.validate()?;
        self.project.folder.journal_read(read)
    }

    pub(crate) fn write<T>(&self, write: impl FnOnce(&Transaction<'_>) -> store::Result<T>) -> store::Result<T> {
        self.validate()?;
        self.project.folder.journal_write(write)
    }

    /// Whether this thread is inside one of the project's own edits, which holds its writer until it commits.
    pub(crate) fn acting(&self) -> bool { self.project.folder.acting_here() }

    pub(crate) fn answer(&self, key: &str) -> store::Result<Option<Answer>> {
        self.read(|conn| {
            Ok(conn.query_row("SELECT digest, status, content_type, body FROM request_keys WHERE key = ?1 AND status IS NOT NULL", [key], |row| {
                Ok(Answer { digest: row.get(0)?, status: row.get::<_, i64>(1)? as u16, content_type: row.get(2)?, body: row.get(3)? })
            }).optional()?)
        })
    }

    pub(crate) fn keep_answer(&self, key: &str, answer: &Answer) -> store::Result<()> {
        self.project.require_view().map_err(project_error)?;
        self.project.folder.keep_answer(key, &answer.digest, answer.status, &answer.content_type, &answer.body).map_err(project_error)
    }

    pub(crate) fn rendered_here(&self, machine: &str) -> store::Result<std::collections::HashMap<String,u64>> {
        self.read(|conn| { let mut stmt=conn.prepare("SELECT tool,bytes FROM project_rendered_here WHERE machine=?1")?; Ok(stmt.query_map([machine], |row| Ok((row.get(0)?,row.get::<_,i64>(1)? as u64)))?.collect::<rusqlite::Result<_>>()?) })
    }
    pub(crate) fn keep_rendered_here(&self, machine: &str, tools: &[(String,u64)]) -> store::Result<()> {
        self.write(|tx| { for (tool,bytes) in tools { tx.execute("INSERT OR REPLACE INTO project_rendered_here(tool,machine,bytes) VALUES(?1,?2,?3)",params![tool,machine,*bytes as i64])?; } Ok(()) })
    }

    pub(crate) fn append_diagnostic(&self, code: &'static str, detail: &str) -> store::Result<()> {
        self.write(|tx| {
            tx.execute("INSERT INTO project_diagnostics (at_ms, code, detail) VALUES (?1, ?2, ?3)", params![store::now_ms(), code, detail])?;
            Ok(())
        })
    }
}

fn project_error(error: Error) -> StoreError {
    match error.code.as_str() {
        "project_locked" | "project_view_changed" | "project_busy" => StoreError::Locked(error.code.to_owned()),
        _ => StoreError::Protected(error.code.to_owned()),
    }
}

/// Called only after the selected authenticated worker returned its COMPLETE
/// migration receipt. A computation memo or a client-supplied spec hash is not a binding.
fn keep_worker_receipt(tx:&Transaction<'_>,project:&str,machine:&str,receipt:&api::machine::ProjectExecutionMigrationReceipt)->store::Result<usize> {
    if machine!="local" {return Err(StoreError::Protected("legacy_remote_worker_pending".into()));}
    let mut covered=0;
    for binding in &receipt.bindings {
        let canonical=tx.query_row("SELECT EXISTS(SELECT 1 FROM runs JOIN journal_migrations ON journal_migrations.source='controller-v7:run:'||runs.id WHERE runs.id=?1 AND runs.owner=?2 AND runs.machine=?3)",params![binding.request_id,project,machine],|row|row.get::<_,bool>(0))?;
        if !canonical {continue;}
        let digest=binding.invocation_spec_digest.strip_prefix("sha256:").unwrap_or(&binding.invocation_spec_digest);
        if binding.source_workspace_id.is_empty() || digest.len()!=64 || !digest.bytes().all(|byte|byte.is_ascii_hexdigit()) {return Err(StoreError::Protected("legacy_worker_binding_invalid".into()));}
        let source=format!("worker-custody:{}",binding.request_id);
        let record=serde_json::to_string(binding).map_err(|_|StoreError::Protected("legacy_worker_binding_invalid".into()))?;
        let prior=tx.query_row("SELECT record FROM journal_migrations WHERE source=?1",[&source],|row|row.get::<_,String>(0)).optional()?;
        if prior.as_ref().is_some_and(|prior|prior!=&record) {return Err(StoreError::Protected("legacy_worker_binding_changed".into()));}
        tx.execute("INSERT OR IGNORE INTO journal_migrations(source,digest,record) VALUES(?1,?2,?3)",params![source,binding.invocation_spec_digest,record])?;
        tx.execute("UPDATE runs SET custody='owned' WHERE id=?1 AND owner=?2 AND machine=?3",params![binding.request_id,project,machine])?;
        covered+=1;
    }
    Ok(covered)
}

/// Where the plan sent each run, and its offer there (cl-967): beside `runs`, so a journal of any version takes it.
pub(crate) const PLACEMENTS: &str = "CREATE TABLE IF NOT EXISTS run_placements (run TEXT PRIMARY KEY, placed TEXT NOT NULL) STRICT, WITHOUT ROWID;";

pub(super) const UNSEEN_SCHEMA:&str="CREATE TABLE IF NOT EXISTS project_unseen (
    project TEXT NOT NULL, box TEXT NOT NULL, number INTEGER NOT NULL,
    PRIMARY KEY(project,box,number)
) STRICT, WITHOUT ROWID;";

pub(super) const SCHEMA: &str = "    CREATE TABLE runs (
        id TEXT PRIMARY KEY,
        number INTEGER NOT NULL UNIQUE,
        idempotency_key TEXT NOT NULL UNIQUE,
        digest TEXT NOT NULL,
        hub TEXT NOT NULL,
        kind TEXT NOT NULL,
        target TEXT NOT NULL,
        machine TEXT,
        pinned INTEGER NOT NULL,
        spec BLOB NOT NULL,
        out_dir TEXT NOT NULL,
        state TEXT NOT NULL CHECK (state IN ('queued', 'sent', 'accepted', 'settled')),
        cancel_requested INTEGER NOT NULL DEFAULT 0,
        cursor INTEGER NOT NULL DEFAULT 0,
        machine_number INTEGER,
        machine_state TEXT,
        status TEXT CHECK (status IN ('completed', 'failed', 'canceled')),
        reason_code TEXT,
        reason TEXT,
        outcome BLOB,
        seq INTEGER NOT NULL DEFAULT 1,
        state_seq INTEGER NOT NULL DEFAULT 1,
        created_at INTEGER NOT NULL,
        accepted_at INTEGER,
        settled_at INTEGER,
        warnings TEXT NOT NULL DEFAULT '[]',
        inputs TEXT NOT NULL DEFAULT '[]',
        wants TEXT,
        agreed TEXT,
        started_at INTEGER,
        stays TEXT NOT NULL DEFAULT '[]',
        batch TEXT NOT NULL DEFAULT '',
        job BLOB,
        owner TEXT NOT NULL DEFAULT '',
        custody TEXT NOT NULL DEFAULT 'owned' CHECK(custody IN ('owned','legacy_pending')),
        attempt INTEGER NOT NULL DEFAULT 1
    ) STRICT;
    CREATE INDEX runs_owed ON runs (machine) WHERE state <> 'settled';
    CREATE TABLE outputs (
        run TEXT NOT NULL REFERENCES runs (id),
        name TEXT NOT NULL,
        idx INTEGER NOT NULL,
        rev INTEGER NOT NULL,
        length INTEGER NOT NULL,
        digest TEXT NOT NULL,
        media_type TEXT NOT NULL,
        label TEXT NOT NULL DEFAULT '',
        duration_us INTEGER NOT NULL DEFAULT 0,
        width INTEGER NOT NULL DEFAULT 0,
        height INTEGER NOT NULL DEFAULT 0,
        measured_ms INTEGER NOT NULL DEFAULT 0,
        done INTEGER NOT NULL DEFAULT 0,
        path TEXT,
        exported_rev INTEGER,
        created_seq INTEGER NOT NULL,
        seq INTEGER NOT NULL,
        failed_rev INTEGER,
        export_error TEXT,
        checkpoint TEXT,
        PRIMARY KEY (run, name, idx)
    ) STRICT, WITHOUT ROWID;
    CREATE INDEX outputs_unexported ON outputs (run) WHERE exported_rev IS NOT rev;
    CREATE TABLE run_logs (
        run TEXT NOT NULL REFERENCES runs (id),
        seq INTEGER NOT NULL,
        at_ms INTEGER NOT NULL,
        level TEXT NOT NULL,
        text TEXT NOT NULL,
        PRIMARY KEY (run, seq)
    ) STRICT, WITHOUT ROWID;
    CREATE TABLE asks (
        key TEXT PRIMARY KEY,
        hub TEXT NOT NULL,
        digest TEXT NOT NULL,
        body BLOB NOT NULL,
        machine TEXT,
        state TEXT NOT NULL CHECK (state IN ('pending', 'done', 'failed')),
        reason TEXT,
        created_at INTEGER NOT NULL,
        kind TEXT NOT NULL DEFAULT 'rent' CHECK (kind IN ('rent', 'end')),
        run TEXT REFERENCES runs (id)
    ) STRICT;
    CREATE TABLE project_view (singleton INTEGER PRIMARY KEY CHECK (singleton = 1), document TEXT NOT NULL) STRICT;
    CREATE TABLE project_diagnostics (id INTEGER PRIMARY KEY, at_ms INTEGER NOT NULL, code TEXT NOT NULL, detail TEXT NOT NULL) STRICT;
    CREATE TABLE journal_migrations (source TEXT PRIMARY KEY, digest TEXT NOT NULL, record TEXT NOT NULL) STRICT;
    CREATE TABLE after_steps (key_sha256 TEXT PRIMARY KEY, step TEXT NOT NULL) STRICT;
    CREATE TABLE project_headers (hub TEXT NOT NULL, sha256 TEXT NOT NULL, families TEXT NOT NULL, lora INTEGER NOT NULL, family TEXT, PRIMARY KEY (hub, sha256)) STRICT, WITHOUT ROWID;
    CREATE TABLE project_rendered_here (tool TEXT NOT NULL, machine TEXT NOT NULL, bytes INTEGER NOT NULL, PRIMARY KEY (tool, machine)) STRICT, WITHOUT ROWID;
    CREATE TABLE project_vault_operations (id TEXT PRIMARY KEY, binding TEXT NOT NULL, state TEXT NOT NULL) STRICT;
";
