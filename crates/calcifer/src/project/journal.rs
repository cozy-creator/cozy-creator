//! Project-owned controller records, on the admitted project's existing SQLite connection.
use std::{path::PathBuf, sync::{Arc, Weak}};
use api::error::Error;
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use crate::{api::Api, store::{self, Answer, StoreError}};
use super::{Project, Projects, vault};

/// The sole durable outside reference. Activity and display data belong to project.db.
#[derive(Clone, Debug)]
pub(crate) struct ProjectRef {
    pub id: String,
    pub path: PathBuf,
    pub code: String,
}

/// A bounded operation on one verified mounted owner, under a read of its gate. Never retain across a worker wait.
pub(crate) struct ProjectStore {
    project: Arc<Project>,
    controller: Weak<crate::Calcifer>,
    _view: vault::Read,
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
    /// Attached once in this Calcifer's life: it may open again by itself once it closed.
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
    pub(crate) fn key(&self) -> String { self.project.key() }
    pub(crate) fn root(&self) -> &std::path::Path { &self.project.root }
    /// A read of the project for one write into it (cl-1060), waited for behind a lock that has its turn: the lock waits for the
    /// write, and none starts into a view the lock took.
    pub(crate) async fn writing(&self) -> Result<vault::Read, Error> {
        vault::admit_work(&self.project).await
    }
    /// The project's journal, under a read of its gate waited for in order: behind a transition that has the project, refused
    /// `project_locked` once a closing one has it, never `project_busy` for a moment's turn.
    pub(crate) fn store(&self) -> store::Result<ProjectStore> {
        let view = vault::admit_work_blocking(&self.project).map_err(project_error)?;
        Ok(ProjectStore { project: self.project.clone(), controller: self.controller.clone(), _view: view, access:Access::Work })
    }
    /// [`Self::store`] now or not at all (`project_busy` while a transition waits for the project or has it): for what reads every
    /// project, which skips one under a lock rather than wait on it (cl-1025).
    pub(crate) fn try_store(&self) -> store::Result<ProjectStore> {
        let view = vault::try_admit_work(&self.project).map_err(project_error)?;
        Ok(ProjectStore { project: self.project.clone(), controller: self.controller.clone(), _view: view, access:Access::Work })
    }
}

pub(crate) struct SealingStore<'a> {
    project: Arc<Project>,
    controller: Weak<crate::Calcifer>,
    authority: Authority<'a>,
}
/// What a [`SealingStore`] reads and writes under: a storage transition, or an inspection beside the project's readers, which only reads.
enum Authority<'a> {
    Transition(&'a vault::ProjectTransition),
    Inspection(&'a vault::ProjectInspection),
}
impl Authority<'_> {
    fn validate(&self, project: &Project) -> Result<(), Error> {
        match self { Self::Transition(transition) => transition.validate(project), Self::Inspection(inspection) => inspection.validate(project) }
    }
}
impl SealingStore<'_> {
    pub(crate) fn owner_id(&self)->Option<&str> {Some(&self.project.id)}
    pub(crate) fn read<T>(&self, read: impl FnOnce(&Connection) -> store::Result<T>) -> store::Result<T> {
        self.authority.validate(&self.project).map_err(project_error)?;
        self.project.folder.journal_read(read)
    }
    pub(crate) fn write<T>(&self, write: impl FnOnce(&Transaction<'_>) -> store::Result<T>) -> store::Result<T> {
        self.writable()?;
        self.project.folder.journal_write(write)
    }
    /// `write` of what the project's held rows keep too (`Folder::journal_write_held`).
    pub(crate) fn write_held<T>(&self, write: impl FnOnce(&Transaction<'_>) -> store::Result<T>) -> store::Result<T> {
        self.writable()?;
        self.project.folder.journal_write_held(write)
    }
    fn writable(&self) -> store::Result<()> {
        if matches!(self.authority, Authority::Inspection(_)) { return Err(StoreError::Locked("project_inspection".into())); }
        self.authority.validate(&self.project).map_err(project_error)
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
    /// Whether the machine ever served, from Calcifer's own list, as `rental`.
    pub(crate) fn served(&self, _tx: &Transaction<'_>, name: &str) -> store::Result<bool> {
        let controller = self.controller.upgrade().ok_or_else(|| StoreError::Protected("controller_stopped".into()))?;
        Ok(controller.maintenance_store().machine(name)?.is_some_and(|machine| machine.served))
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
        Ok(SealingStore { project, controller: Arc::downgrade(&api.calcifer), authority: Authority::Transition(transition) })
    }

    /// [`Projects::transition_store`] for reading alone, beside the project's other readers (`vault::ProjectInspection`).
    pub(super) fn inspection_store<'a>(&self, api: &Arc<Api>, named: &str, inspection: &'a vault::ProjectInspection) -> Result<SealingStore<'a>, Error> {
        let open = self.open.lock().expect("projects lock").get(named).and_then(Weak::upgrade)
            .filter(|project| inspection.validate(project).is_ok());
        let project = match open { Some(project) => project, None => self.get(api, named).map_err(|error| error.0)? };
        inspection.validate(&project)?;
        Ok(SealingStore { project, controller: Arc::downgrade(&api.calcifer), authority: Authority::Inspection(inspection) })
    }

    pub(crate) async fn store(&self, api: &Arc<Api>, named: &str) -> Result<ProjectStore, Error> {
        let view = vault::admit_project(api, named).await?;
        let project = self.get(api, named).map_err(|error| error.0)?;
        project.require_view()?;
        let store = ProjectStore { project, controller: Arc::downgrade(&api.calcifer), _view: view, access:Access::Viewing };
        super::unseen::refresh(&store.project)?;
        // A lock revokes the view without the gate: one that lands while this reads says so (409), not that the journal is gone.
        let fresh=api.calcifer.attach_project(&store).map_err(|error| match error {
            StoreError::Locked(code) => Error::new(api::error::Kind::Conflict, code, "the project was locked, or its view changed, while it was read"),
            _ => Error::new(api::error::Kind::Unavailable, "project_journal", "the project journal is unavailable"),
        })?;
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
    fn validate(&self)->store::Result<()> {
        match self.access {
            Access::Viewing=>self.project.require_view().map_err(project_error),
            // Its own read holds the project: no transition has it while this store lives, so its view is checked, not asked for again.
            Access::Work=>vault::validate_work(&self.project).map_err(project_error),
        }
    }
    pub(crate) fn admit_new_work(&self)->store::Result<()> {self.project.require_view().map_err(project_error)}
    pub(crate) fn owner_id(&self)->Option<&str> {Some(&self.project.id)}
    pub(crate) fn resume_allowed(&self)->store::Result<bool> { self.read(|conn|Ok(!conn.query_row("SELECT EXISTS(SELECT 1 FROM runs WHERE owner<>?1)",[self.project_id()],|row|row.get::<_,bool>(0))?)) }
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
    /// Whether the machine ever served, from Calcifer's own list, as `rental`.
    pub(crate) fn served(&self, _tx: &Transaction<'_>, name: &str) -> store::Result<bool> {
        let controller = self.controller.upgrade().ok_or_else(|| StoreError::Protected("controller_stopped".into()))?;
        Ok(controller.maintenance_store().machine(name)?.is_some_and(|machine| machine.served))
    }


    pub(crate) fn read<T>(&self, read: impl FnOnce(&Connection) -> store::Result<T>) -> store::Result<T> {
        self.validate()?;
        self.project.folder.journal_read(read)
    }

    pub(crate) fn write<T>(&self, write: impl FnOnce(&Transaction<'_>) -> store::Result<T>) -> store::Result<T> {
        self.validate()?;
        self.project.folder.journal_write(write)
    }

    /// `write` of what the project's held rows keep too (`Folder::journal_write_held`).
    pub(crate) fn write_held<T>(&self, write: impl FnOnce(&Transaction<'_>) -> store::Result<T>) -> store::Result<T> {
        self.validate()?;
        self.project.folder.journal_write_held(write)
    }

    /// Whether this thread is inside one of the project's own edits, which holds its writer until it commits.
    pub(crate) fn acting(&self) -> bool { self.project.folder.acting_here() }

    /// Run `id`'s cancel written in the project's edit going on this thread (`Folder::join_act`): it commits with the edit or not at
    /// all, and `then` has its events once it committed; dropped unrun, with the edit, when it doesn't. False: no edit is going here.
    pub(crate) fn cancel_in_act(&self, id: &str, then: impl FnOnce(Vec<api::calcifer::Event>) + 'static) -> bool {
        let (id, events) = (id.to_owned(), std::rc::Rc::new(std::cell::RefCell::new(vec![])));
        let kept = events.clone();
        let write = move |tx: &Transaction<'_>| {
            *kept.borrow_mut() = store::cancel_in(tx, &id).map_err(|e| crate::api::Refusal::from(crate::Error::from(e)).0)?;
            Ok(())
        };
        self.project.folder.join_act(Box::new(write), move |_| then(events.take()))
    }

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

/// Where the plan sent each run, and its offer there (cl-967): beside `runs`, so a journal of any version takes it.
pub(crate) const PLACEMENTS: &str = "CREATE TABLE IF NOT EXISTS run_placements (run TEXT PRIMARY KEY, placed TEXT NOT NULL) STRICT, WITHOUT ROWID;";
/// Why a run's attempt before its current one stopped (cl-1173), its record's `retry`: kept beside `runs`, so a project.db or store an
/// older Calcifer wrote opens as it was, and one never retried has no row.
pub(crate) const RETRIES: &str = "CREATE TABLE IF NOT EXISTS run_retries (run TEXT PRIMARY KEY, why TEXT NOT NULL) STRICT, WITHOUT ROWID;";

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
    CREATE TABLE project_vault_operations (id TEXT PRIMARY KEY, binding TEXT NOT NULL, state TEXT NOT NULL) STRICT;
";
