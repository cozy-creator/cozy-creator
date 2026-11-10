//! A run as its client reads it (`api::machine`): its state, its log and its outputs, and what the
//! machine holds for a caller. A finished run's log is made once, at its first read, and kept.
use super::Runs;
use crate::{
    api::auth::VerifiedActor,
    gpu_service::{without_gpus, KeptMember, Level},
    journal::{Execution, State},
    objects::refusal,
    stored,
};
use api::{error::Error, machine as v1};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    io::{self, Read},
    time::Duration,
};
use tensorfs::format::ObjectRef;
use tensorfs::sha256;

/// One consistent view of an output's current bytes: its parts in order.
pub(crate) struct OutputSnapshot {
    pub parts: Vec<(std::fs::File, u64)>,
    pub length: u64,
    /// The 1-based ordinal of this (output, index)'s products in the run's log.
    pub rev: u64,
    pub media_type: String,
    /// `sha256:<hex>`, set once the run is terminal (the output is final).
    pub sha256: Option<String>,
}

const LIVE: [&str; 3] = ["queued", "starting", "running"];

/// A run's state by name. An attempt is queued until it runs; only a run sent here is accepted
/// before it is prepared.
fn state_name(record: &Execution) -> &'static str {
    match record.state {
        State::Completed => "succeeded",
        State::Failed => "failed",
        State::Canceled => "canceled",
        State::Queued if record.waiting_reason.as_deref() == Some(crate::journal::PREPARING) => "preparing",
        State::Queued if record.waiting_reason.as_deref() == Some(crate::journal::NEEDS_ACCESS) => "needs_access",
        State::Queued | State::Starting => "queued",
        State::Running => "running",
        State::Unknown => "unknown",
    }
}

impl Runs {
    fn workspace_id(&self) -> String {
        self.service.engine.workspace_id()
    }
    /// `actor`'s run `id`.
    fn record(&self, actor: VerifiedActor, id: &str) -> Result<Execution, Error> {
        self.service
            .engine
            .get_public(reader(actor).as_deref(), id)
            .map_err(|error| match error.kind() {
                io::ErrorKind::NotFound => Error::not_found("run_not_found", "this machine has no such run"),
                _ => problem(error),
            })
    }
    fn state_of(&self, record: &Execution) -> Result<v1::RunState, Error> {
        let context = record
            .submission
            .as_ref()
            .ok_or_else(|| Error::internal("internal", "public submission context absent"))?;
        let sequence = self
            .service
            .engine
            .public_terminal(&record.id)
            .map_err(problem)?
            .map(|held| {
                stored::decode::<stored::LogPage>(&held)
                    .map(|page| page.head_sequence)
                    .map_err(|_| Error::internal("custody_corrupt", "durable event projection corrupt"))
            })
            .transpose()?
            .unwrap_or(record.revision)
            .max(record.revision);
        let invocation = &record.invocation;
        Ok(v1::RunState {
            id: context.request_id.clone(),
            number: record
                .id
                .parse()
                .map_err(|_| Error::internal("internal", "invalid journal run number"))?,
            state: state_name(record).into(),
            sequence,
            attempt: record.attempt.max(1),
            started_at_ms: record.started_at_ms,
            target: [invocation.package.as_str(), invocation.entrypoint.as_str()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("/"),
            accepted_at_ms: record.accepted_at_ms,
            finished_at_ms: record.finished_at_ms,
            ..Default::default()
        })
    }
    /// `actor`'s run `id` now.
    pub(crate) fn state(&self, actor: VerifiedActor, id: &str) -> Result<v1::RunState, Error> {
        self.state_of(&self.record(actor, id)?)
    }
    fn page(&self, actor: VerifiedActor, states: &[String], before: u64, newest: bool, limit: usize) -> Result<Vec<v1::RunState>, Error> {
        let records = self
            .service
            .engine
            .actor_page(reader(actor).as_deref(), 0, before, newest, states, limit)
            .map_err(problem)?;
        records.iter().map(|record| self.state_of(record)).collect()
    }
    /// The runs `actor` reads, newest first, before the run numbered `before` (0: the newest).
    pub(crate) fn list(&self, actor: VerifiedActor, before: u64, limit: u32) -> Result<v1::RunList, Error> {
        let limit = if limit == 0 { 64 } else { limit.min(256) };
        Ok(v1::RunList { runs: self.page(actor, &[], before, true, limit as usize)? })
    }
    /// The runs of `actor`'s that have not ended, oldest first.
    pub(crate) fn live(&self, actor: VerifiedActor) -> Result<Vec<v1::RunState>, Error> {
        self.page(actor, &LIVE.map(String::from), 0, false, 256)
    }
    /// Cancels `actor`'s run `id`; an id this machine never received is recorded canceled.
    pub(crate) fn cancel(&self, actor: VerifiedActor, id: &str) -> Result<v1::RunState, Error> {
        let canceled = self.service.engine.cancel_run(&actor_id(actor), id).map_err(|error| match error.kind() {
            io::ErrorKind::Unsupported => refusal("update_control_unsupported", error.to_string()),
            io::ErrorKind::ResourceBusy => refusal("cancel_unavailable", error.to_string()),
            _ => problem(error),
        })?;
        if let Some(calls) = self.service.calls() {
            calls.canceled(&canceled);
        }
        self.state_of(&canceled)
    }
    /// The run's journaled log: its published products, its memoized calls' results and its
    /// settled calls, as events with their journaled sequences.
    fn product_events(&self, record: &Execution) -> Result<Vec<stored::LogEvent>, Error> {
        let attempt = record.attempt.max(1) as u64;
        let engine = &self.service.engine;
        let mut events = engine
            .products(&record.id)
            .map_err(problem)?
            .iter()
            .map(|stored| {
                let product = crate::products::decode(stored).map_err(problem)?;
                Ok(stored::LogEvent {
                    sequence: stored.sequence,
                    attempt_ordinal: attempt,
                    at_ms: stored.at_ms,
                    kind: "product".into(),
                    body_canonical_bytes: product_document(&product)?,
                    product: Some(product),
                    ..Default::default()
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let calls = engine.calls(&record.id).map_err(problem)?.into_iter().map(|c| ("call", c));
        let logs = engine.logs(&record.id).map_err(problem)?.into_iter().map(|l| ("log", l));
        for (kind, stored) in calls.chain(logs) {
            events.push(stored::LogEvent {
                sequence: stored.sequence,
                attempt_ordinal: attempt,
                at_ms: stored.at_ms,
                kind: kind.into(),
                body_canonical_bytes: stored.product,
                ..Default::default()
            });
        }
        events.sort_by_key(|event| event.sequence);
        Ok(events)
    }
    fn terminal(&self, record: &Execution) -> Result<stored::LogPage, Error> {
        let _guard = self.projection.lock().unwrap();
        if let Some(held) = self
            .service
            .engine
            .public_terminal(&record.id)
            .map_err(problem)?
        {
            return stored::decode::<stored::LogPage>(&held)
                .map_err(|_| Error::internal("custody_corrupt", "durable event projection is corrupt"));
        }
        if !record.state.terminal() {
            return Err(Error::conflict("precondition_failed", "execution is not terminal"));
        }
        let context = record
            .submission
            .as_ref()
            .ok_or_else(|| Error::internal("internal", "public submission context absent"))?;
        let mut value = record.result.as_ref().map(|r| r.value.clone());
        let mut products = vec![];
        let mut schema_digest = None;
        // A warm run's result describes its preparation; no callable produced it.
        if let Some(result) = record
            .result
            .as_ref()
            .filter(|_| !record.invocation.generation.is_empty())
        {
            let held = self
                .service
                .engine
                .installation_for_generation(&context.actor, &record.invocation.generation)
                .map_err(problem)?;
            // Its installation removed since (`cozy package remove`): the environment's own record.
            // Without either this is a refusal, never a 500 its controller asks about forever.
            let interface: Value = match held {
                Some(held) => serde_json::from_slice(&held.interface)
                    .map_err(|_| Error::internal("custody_corrupt", "held installation interface corrupt"))?,
                None => self.service.catalog.resolve(&record.invocation.generation)
                    .map(|generation| generation.record.interface.clone())
                    .map_err(|_| Error::conflict("result_unreadable",
                        format!("{} was removed from this machine before this run's result was read: run it again", record.invocation.package)))?,
            };
            let declaration =
                entrypoint(&interface, &record.invocation.entrypoint)?;
            schema_digest =
                Some(identity(api::interface::result(declaration).ok_or_else(|| {
                    Error::conflict("precondition_failed", "result schema absent")
                })?)?);
            let mut references = std::collections::HashMap::new();
            for binding in &result.asset_bindings {
                let index = result
                    .artifacts
                    .iter()
                    .position(|a| a.name == binding.relative_path)
                    .ok_or_else(|| Error::internal("custody_corrupt", "output binding artifact absent"))?;
                let artifact = &result.artifacts[index];
                let object = ObjectRef {
                    sha256: artifact.sha256.clone(),
                    length: artifact.length,
                };
                let owner = identity(
                    &json!({"workspace":self.workspace_id(),"actor":context.actor,"request":context.request_id,"asset":binding.asset_ref}),
                )?;
                let mut source = self
                    .service
                    .engine
                    .open_result(&record.id, index)
                    .map_err(problem)?;
                let native = crate::products::retain(
                    &self.service.engine,
                    &context.actor,
                    &record.id,
                    &owner,
                    &mut source,
                    &object,
                )
                .map_err(problem)?;
                // A tree's members are held too: Read serves them, a child may be handed it.
                if binding.media_type == api::tree::MEDIA {
                    let mut manifest = Vec::new();
                    self.service.engine.open_result(&record.id, index).map_err(problem)?
                        .read_to_end(&mut manifest).map_err(problem)?;
                    for api::tree::Member { sha256: sha, length, .. } in api::tree::parse(&manifest).map_err(problem)?.into_values() {
                        let member = result.artifacts.iter().position(|a| a.sha256 == sha && a.length == length)
                            .ok_or_else(|| Error::internal("custody_corrupt", "a tree member is not held"))?;
                        let owner = identity(&json!({"workspace":self.workspace_id(),"actor":context.actor,
                            "request":context.request_id,"asset":binding.asset_ref,"member":sha}))?;
                        crate::products::retain(&self.service.engine, &context.actor, &record.id, &owner,
                            &mut self.service.engine.open_result(&record.id, member).map_err(problem)?,
                            &ObjectRef { sha256: sha, length }).map_err(problem)?;
                    }
                }
                references.insert(
                    binding.asset_ref.clone(),
                    (artifact.clone(), binding.media_type.clone(), native),
                );
            }
            if let Some(value) = &mut value {
                rewrite_assets(
                    value,
                    api::interface::result(declaration)
                        .ok_or_else(|| Error::internal("custody_corrupt", "result schema absent"))?,
                    "",
                    stored::RunProductOp::Set,
                    0,
                    &references,
                    &mut products,
                )?;
            }
        }
        let output_entries = output_entries(&products)?;
        // The log's published products keep their sequences; the result publishes only what
        // the log does not already show, after them.
        let mut events = self.product_events(record)?;
        let shown: Vec<_> = events.iter().filter_map(|e| e.product.clone()).collect();
        // An observer that last saw it queued still sees it start before it ends.
        if record.running_revision > 0 {
            events.push(stored::LogEvent {
                sequence: record.running_revision,
                attempt_ordinal: record.attempt.max(1) as u64,
                at_ms: record.started_at_ms,
                kind: "running".into(),
                body_canonical_bytes: canonical(&json!({"generation":record.attempt.max(1)}))?,
                ..Default::default()
            });
            events.sort_by_key(|event| event.sequence);
        }
        let first_sequence = record
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::capacity("exhausted", "event cursor exhausted"))?;
        let fresh = products
            .into_iter()
            .filter(|product| !crate::products::shown(&shown, product));
        for (index, product) in fresh.enumerate() {
            events.push(stored::LogEvent {
                sequence: first_sequence + index as u64,
                attempt_ordinal: record.attempt.max(1) as u64,
                at_ms: record.finished_at_ms,
                kind: "product".into(),
                body_canonical_bytes: product_document(&product)?,
                product: Some(product),
                ..Default::default()
            });
        }
        // The Python worker's run facts: which Runtime executed it, and for how long.
        let attempt = record.attempt.max(1) as u64;
        let mut facts = vec![];
        if let Some(executor) = record
            .executor
            .as_ref()
            .filter(|e| !e.runtime_version.is_empty())
        {
            facts.push(("executor", json!({"request":context.request_id,"attempt":attempt,"pid":executor.pid,"runtime_version":executor.runtime_version,"tensorfs_version":executor.tensorfs_version})));
        }
        let mut timing = json!({"attempt":attempt,"terminal":true});
        if record.started_at_ms > 0 && record.finished_at_ms >= record.started_at_ms {
            timing["execution_ms"] = json!((record.finished_at_ms - record.started_at_ms) as f64);
        }
        facts.push(("run.timing", timing));
        for (kind, body) in facts {
            events.push(stored::LogEvent {
                sequence: first_sequence + events.len() as u64,
                attempt_ordinal: attempt,
                at_ms: record.finished_at_ms,
                kind: kind.into(),
                body_canonical_bytes: canonical(&body)?,
                ..Default::default()
            });
        }
        let mut body = json!({"format":"cozy.machine.outcome/1", "request_id":context.request_id, "attempt_ordinal":record.attempt.max(1), "invocation_spec_digest":context.invocation_digest});
        // Executor return clears live process custody, not the persisted start authorization.
        if record.started_at_ms != 0 {
            body["execution_started"] = json!(true);
        }
        let (status, code, origin, message, error_code) = match record.state {
            State::Completed => (1, 0, 2, "completed".to_string(), String::new()),
            State::Canceled if record.cancel_actor.as_deref() == Some("machine:gpu_pressure") => (
                4,
                11,
                3,
                "yielded to external GPU memory pressure; this run was not restarted".to_string(),
                "canceled".into(),
            ),
            State::Canceled => (4, 11, 6, "explicitly canceled".to_string(), "canceled".into()),
            _ => {
                let failure = crate::journal::Failure::decode(
                    record
                        .failure
                        .as_deref()
                        .unwrap_or_default(),
                ).map_err(problem)?;
                (
                    failure.status,
                    failure.cause,
                    failure.origin,
                    safe(&failure.message, 4096),
                    failure.code,
                )
            }
        };
        body["status"] = json!(status);
        body["cause"] = if code == 0 {
            json!({"origin":origin})
        } else {
            json!({"code":code,"origin":origin,"detail":safe(&message, 1024)})
        };
        body["safe_message"] = json!(message);
        if !error_code.is_empty() {
            body["error_code"] = json!(error_code);
        }
        // Observation only: the failed attempt's bundle, written before the run settled.
        if let Some((_, bundle)) = self.service.engine.triage(&record.id).map_err(problem)? {
            if let Some(detail) = crate::triage::detail(&bundle) {
                body["failure_detail"] = json!(detail);
            }
        }
        if !output_entries.is_empty() {
            body["output_manifest"] = json!({"outputs":output_entries});
        }
        if let Some(value) = value {
            body["result"] = json!({"result_schema_digest":schema_digest, "inline_result":STANDARD.encode(crate::boundary_json::exact(&value))});
        }
        if let Some(result) = record.result.as_ref().filter(|r| !r.adjustments.is_empty()) {
            body["adjustments"] = json!(result.adjustments);
        }
        let bytes = canonical(&body)?;
        let digest = sha256::digest(&bytes);
        let outcome = stored::AttemptOutcome {
            worker_boot_id: record.acceptance_boot_id.clone(),
            request_id: context.request_id.clone(),
            attempt_ordinal: record.attempt.max(1) as u64,
            invocation_spec_digest: if record.canceled_before_acceptance {
                Vec::new() // cancellation accepted no invocation spec
            } else {
                digest_bytes(&context.invocation_digest)?
            },
            outcome_id: format!("out-{}", sha256::hex(&digest)),
            outcome_digest: digest.to_vec(),
            outcome_canonical_bytes: bytes,
        };
        let sequence = events
            .last()
            .map_or(record.revision, |event| event.sequence.max(record.revision))
            + 1;
        events.push(stored::LogEvent {
            sequence,
            attempt_ordinal: outcome.attempt_ordinal,
            at_ms: record.finished_at_ms,
            kind: "outcome".into(),
            body_canonical_bytes: canonical(&json!({
                "state":match record.state { State::Completed => "completed", State::Canceled => "canceled", _ => "failed" },
                "outcome_id":outcome.outcome_id,
                "outcome_digest":format!("sha256:{}",sha256::hex(&outcome.outcome_digest))
            }))?,
            outcome: Some(outcome),
            ..Default::default()
        });
        let page = stored::LogPage {
            events,
            next_after: sequence,
            head_sequence: sequence,
            compacted_through: record.revision,
        };
        let committed = self
            .service
            .engine
            .commit_public_terminal(&record.id, stored::encode(&page))
            .map_err(problem)?;
        stored::decode::<stored::LogPage>(&committed)
            .map_err(|_| Error::internal("custody_corrupt", "durable event projection is corrupt"))
    }
    /// One output of `actor`'s run `id`: its current bytes. `index` is a list item's 1-based index.
    pub(crate) fn open_output(
        &self,
        actor: VerifiedActor,
        id: &str,
        output: &str,
        index: Option<u32>,
    ) -> Result<OutputSnapshot, Error> {
        let absent = || Error::not_found("output_not_found", "this machine has no such run or output");
        let record = self.record(actor, id)?;
        let terminal = record.state.terminal();
        let events = if terminal {
            self.terminal(&record)?.events
        } else {
            self.product_events(&record)?
        };
        let (op, position) = match index {
            Some(index) => (
                stored::RunProductOp::Append,
                index.checked_sub(1).ok_or_else(absent)?,
            ),
            None => (stored::RunProductOp::Set, 0),
        };
        let revisions: Vec<_> = events
            .into_iter()
            .filter_map(|event| event.product)
            .filter(|p| p.output == output && p.op == op as i32 && p.index == position)
            .collect();
        let current = revisions.last().ok_or_else(absent)?;
        if current.checkpoint.is_some() {
            return Err(Error::validation("output_is_a_checkpoint", "checkpoint outputs are TensorFS references"));
        }
        let content = current
            .content
            .as_ref()
            .ok_or_else(|| Error::internal("custody_corrupt", "product content absent"))?;
        let refs: Vec<&stored::Ref> = if current.parts.is_empty() {
            vec![content]
        } else {
            current
                .parts
                .iter()
                .filter_map(|p| p.content.as_ref())
                .collect()
        };
        let parts = refs
            .into_iter()
            .map(|r| {
                let object = ObjectRef { sha256: sha256::hex(&r.digest), length: r.length };
                let file = crate::storage::file(&self.service.engine.store, &object).map_err(problem)?;
                Ok((file, r.length))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(OutputSnapshot {
            parts,
            length: content.length,
            rev: revisions.len() as u64,
            media_type: current.media_type.clone(),
            sha256: terminal.then(|| format!("sha256:{}", sha256::hex(&content.digest))),
        })
    }
    /// One file of a tree output (`member`, its manifest path), at the output's revision.
    pub(crate) fn open_member(
        &self,
        actor: VerifiedActor,
        id: &str,
        output: &str,
        index: Option<u32>,
        member: &str,
    ) -> Result<OutputSnapshot, Error> {
        let tree = self.open_output(actor, id, output, index)?;
        if tree.media_type != api::tree::MEDIA {
            return Err(Error::validation("invalid_request", "only a tree output has members"));
        }
        let mut manifest = Vec::new();
        for (file, length) in tree.parts {
            file.take(length).read_to_end(&mut manifest).map_err(problem)?;
        }
        let document = crate::trees::parse(&manifest)
            .map_err(|_| Error::internal("custody_corrupt", "tree manifest is invalid"))?;
        let object = document
            .get(member)
            .ok_or_else(|| Error::not_found("output_not_found", "the tree has no such member"))?;
        let (sha, length) = (object.sha256.as_str(), object.length);
        let file = crate::storage::file(&self.service.engine.store, object).map_err(storage)?;
        Ok(OutputSnapshot {
            parts: vec![(file, length)],
            length,
            rev: tree.rev,
            media_type: "application/octet-stream".into(),
            sha256: Some(format!("sha256:{sha}")),
        })
    }
    /// A complete object this signer wrote, verified against `sha256` (hex) as it was stored.
    pub(crate) fn object(&self, actor: VerifiedActor, sha256: &str) -> Result<std::fs::File, Error> {
        // Only an object this signer wrote: another signer's reads as absent.
        let held = self
            .service
            .engine
            .with_journal(|j| j.object(&actor_id(actor), sha256))
            .map_err(problem)?;
        let Some(length) = held else {
            return Err(Error::not_found("object_not_found", "this signer wrote no such object"));
        };
        let object = ObjectRef { sha256: sha256.to_string(), length };
        crate::storage::file(&self.service.engine.store, &object).map_err(storage)
    }
    /// `actor`'s installations, each with what it holds now (`StatusFrame.environments`).
    pub(crate) fn environments(&self, actor: VerifiedActor) -> Result<Vec<v1::Environment>, Error> {
        let (actor, gpu) = (actor_id(actor), self.service.gpu());
        let held = gpu.as_ref().map(|gpu| gpu.levels()).unwrap_or_default();
        let mut environments = vec![];
        for installed in self.service.engine.installations(&actor).map_err(problem)? {
            let record = match self.service.catalog.record(&installed.generation) {
                Ok(record) => record,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(problem(error)),
            };
            // `downloaded`: the store still holds every model of a preparation bound to it.
            let bound = self
                .service
                .engine
                .with_journal(|journal| journal.preparations_of(&actor, &installed.alias))
                .map_err(problem)?;
            let downloaded = gpu.as_ref().is_some_and(|gpu| {
                bound.iter().any(|bound| gpu.plan(bound).is_ok_and(|plan| gpu.downloaded(&plan)))
            });
            let base = match downloaded {
                true => Level::Downloaded,
                false => Level::Installed,
            };
            // Only the installation's own App: a callee's executors in its environment are the
            // callee's.
            let level = held
                .get(&(installed.generation.clone(), record.application))
                .map_or(base, |held| base.max(*held));
            let interface: Value = serde_json::from_slice(&installed.interface)
                .map_err(|_| Error::internal("custody_corrupt", "held interface corrupt"))?;
            let mut entrypoints = api::interface::entrypoints(&interface)
                .filter_map(|e| api::interface::name(e).map(str::to_string))
                .collect::<Vec<_>>();
            entrypoints.sort();
            environments.push(v1::Environment {
                level: level.name().into(),
                installation: installed.alias,
                package: installed.package,
                release: installed.release,
                entrypoints,
                runtime: (record.dependencies.into_iter())
                    .find(|d| d.name == "cozy-runtime")
                    .map(|d| d.version)
                    .unwrap_or_default(),
                // Why this environment does not run the machine's own SDK; empty when it does.
                warning: record.sdk_fallback,
                hub: installed.hub,
            });
        }
        environments.sort_by(|a, b| (&a.package, &a.release, &a.installation).cmp(&(&b.package, &b.release, &b.installation)));
        Ok(environments)
    }
    /// Each GPU's page accounting by UUID (`StatusFrame.gpus[].memory`).
    pub(crate) fn memory(&self) -> BTreeMap<String, v1::GpuMemory> {
        self.service.gpu().map(|gpu| gpu.memory()).unwrap_or_default()
    }
    /// The caller's warm set, each member with what it holds now (`StatusFrame.warm`).
    pub(crate) fn warm(&self, actor: VerifiedActor) -> Result<Vec<v1::WarmItem>, Error> {
        let actor = actor_id(actor);
        let kept = self
            .service
            .engine
            .with_journal(|journal| journal.warm_sets())
            .map_err(problem)?;
        let holds = self.service.gpu().map(|gpu| gpu.members(&actor));
        let mut items: Vec<v1::WarmItem> = vec![];
        fn corrupt<E>(_: E) -> Error {
            Error::internal("custody_corrupt", "kept warm set corrupt")
        }
        // An item an orchestrator's callees joined is several members in a row: one item, at
        // its lowest member's level.
        let mut last: Option<(String, Level)> = None;
        for (position, (_, record)) in kept.iter().filter(|(of, _)| *of == actor).enumerate() {
            let kept: KeptMember = serde_json::from_str(record).map_err(corrupt)?;
            // Before the pool has taken the set up (a restart in progress) it holds its code.
            let (level, held_back) = match &holds {
                Some(holds) => holds.get(position).copied().unwrap_or((Level::Installed, "")),
                None => without_gpus(Level::named(&kept.level).ok_or_else(|| corrupt(()))?),
            };
            if let (Some((sent, lowest)), Some(item)) = (&mut last, items.last_mut()) {
                if *sent == kept.item {
                    if level < *lowest {
                        *lowest = level;
                        (item.holds, item.held_back) = (level.name().into(), held_back.into());
                    }
                    continue;
                }
            }
            let sent = STANDARD.decode(&kept.item).map_err(corrupt)?;
            let mut item: v1::WarmItem = serde_json::from_slice(&sent).map_err(corrupt)?;
            (item.holds, item.held_back) = (level.name().into(), held_back.into());
            items.push(item);
            last = Some((kept.item, level));
        }
        Ok(items)
    }
    /// Up to 256 of the run's stored events after `after`, waiting up to 30 s for one.
    fn log_page(&self, actor: VerifiedActor, id: &str, after: u64) -> Result<stored::LogPage, Error> {
        let mut record = self.record(actor, id)?;
        if !record.state.terminal() && record.revision <= after {
            let epoch = self.service.engine.activity_epoch();
            record = self.record(actor, id)?;
            if !record.state.terminal() && record.revision <= after {
                self.service
                    .engine
                    .wait_activity(epoch, Some(Duration::from_secs(30)));
                record = self.record(actor, id)?;
            }
        }
        if record.state.terminal() {
            let mut page = self.terminal(&record)?;
            page.events.retain(|event| event.sequence > after);
            page.events.truncate(256);
            page.next_after = page.events.last().map(|e| e.sequence).unwrap_or(after);
            return Ok(page);
        }
        // The Python worker's live kinds: `running` once, then the latest `progress`.
        let attempt = record.attempt.max(1) as u64;
        let event = |sequence: u64,
                     kind: &str,
                     body: &Value|
         -> Result<stored::LogEvent, Error> {
            Ok(stored::LogEvent {
                sequence,
                attempt_ordinal: attempt,
                at_ms: now_ms(),
                kind: kind.into(),
                body_canonical_bytes: canonical(body)?,
                ..Default::default()
            })
        };
        // Published products keep their journaled sequences; see `products.rs`.
        let mut events = self.product_events(&record)?;
        let published = events.last().map_or(0, |event| event.sequence);
        events.retain(|event| event.sequence > after);
        let running = record.state == State::Running && record.running_revision > 0;
        if running && record.running_revision > after {
            let mut started = event(
                record.running_revision,
                "running",
                &json!({"generation":attempt}),
            )?;
            started.at_ms = record.started_at_ms;
            events.push(started);
        }
        let floor = after.max(if running { record.running_revision } else { 0 });
        if record.revision > floor && record.revision > published {
            // A run preparing inside itself (`runs`) shows its stage and bytes as progress.
            let preparing = record.state == State::Queued
                && record.waiting_reason.as_deref() == Some(crate::journal::PREPARING);
            // What a queued run waits for (another run's install, its place in the queue) is its state,
            // until its weights arrive meanwhile: then their bytes are its progress, the client
            // saying both.
            let waiting = record.waiting.clone().filter(|_| record.state == State::Queued);
            let payload = record.progress.as_ref().map(|progress| {
                serde_json::from_str::<Value>(progress)
                    .ok()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| json!({"stage":progress,"step_ms":0.0}))
            });
            let arriving = payload.as_ref().is_some_and(|p| p["bytes_done"].as_u64() < p["bytes_total"].as_u64());
            if let Some(payload) = payload.filter(|_| (running || preparing) && waiting.is_none() || arriving) {
                // When it last moved, never when it is read: a silent run shows how long it is.
                let mut moved = event(record.revision, "progress", &json!({"type":"progress","payload":payload}))?;
                let at = record.progress_at_ms.max(record.started_at_ms);
                if at > 0 {
                    moved.at_ms = at;
                }
                events.push(moved);
            } else {
                // Every cursor up to the head is answered, or a waiting client would spin.
                let waiting = waiting.or_else(|| record.waiting_reason.clone());
                events.push(event(record.revision, "state", &json!({"state":state_name(&record),"completed_units":record.completed_units,"waiting_reason":waiting}))?);
            }
        }
        events.sort_by_key(|event| event.sequence);
        events.truncate(256);
        Ok(stored::LogPage {
            next_after: events.last().map(|e| e.sequence).unwrap_or(after),
            events,
            head_sequence: record.revision,
            compacted_through: record.revision.saturating_sub(1),
        })
    }
}
/// Printable ASCII only, bounded to its head and tail: a reason is shown to the client verbatim.
fn safe(text: &str, limit: usize) -> String {
    crate::triage::ends(text, limit)
        .chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { ' ' })
        .collect()
}
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
pub(crate) fn actor_id(actor: VerifiedActor) -> String {
    sha256::hex(&actor.public_key)
}
/// Whose runs `actor` reads: its own, or, for a rental's owner, every client's (none named).
fn reader(actor: VerifiedActor) -> Option<String> {
    (!actor.owner).then(|| actor_id(actor))
}
fn canonical(value: &Value) -> Result<Vec<u8>, Error> {
    serde_json_canonicalizer::to_vec(value)
        .map_err(|_| Error::validation("invalid_request", "JSON value is outside canonical profile"))
}
fn identity(value: &Value) -> Result<String, Error> {
    Ok(format!(
        "sha256:{}",
        sha256::hex(&sha256::digest(&canonical(value)?))
    ))
}
fn digest_bytes(value: &str) -> Result<Vec<u8>, Error> {
    let value = value
        .strip_prefix("sha256:")
        .ok_or_else(|| Error::internal("custody_corrupt", "digest algorithm differs"))?;
    if value.len() != 64 {
        return Err(Error::internal("custody_corrupt", "digest length differs"));
    }
    (0..32)
        .map(|i| {
            u8::from_str_radix(&value[2 * i..2 * i + 2], 16)
                .map_err(|_| Error::internal("custody_corrupt", "digest spelling differs"))
        })
        .collect()
}
/// A callable's declaration.
fn entrypoint<'a>(interface: &'a Value, name: &str) -> Result<&'a Value, Error> {
    api::interface::entrypoint(interface, name)
        .ok_or_else(|| Error::validation("invalid_request", "entrypoint is not declared by this held package: `cozy run <package>` lists its functions"))
}
fn problem(error: io::Error) -> Error {
    match error.kind() {
        io::ErrorKind::NotFound => Error::not_found("not_found", error.to_string()),
        io::ErrorKind::AlreadyExists => Error::conflict("already_exists", error.to_string()),
        io::ErrorKind::InvalidInput => Error::validation("invalid_request", error.to_string()),
        io::ErrorKind::PermissionDenied => Error::forbidden("capability_scope", error.to_string()),
        _ => Error::conflict("precondition_failed", error.to_string()),
    }
}
fn storage(error: impl std::fmt::Display) -> Error {
    Error::conflict("precondition_failed", format!("native custody: {error}"))
}
type AssetSources = std::collections::HashMap<
    String,
    (
        crate::journal::Artifact,
        String,
        stored::NativeByteRetentionRequest,
    ),
>;
fn rewrite_assets(
    value: &mut Value,
    schema: &Value,
    path: &str,
    op: stored::RunProductOp,
    index: u32,
    sources: &AssetSources,
    products: &mut Vec<stored::RunProduct>,
) -> Result<(), Error> {
    if value.is_null() {
        return Ok(());
    }
    // A present optional (`T | None`) is a `T`.
    let schema = crate::products::present(schema);
    // A returned tree (`{"input": "tree"}`) is an output like an asset: its manifest's bytes.
    if schema.get("asset").is_some() || schema["input"] == "tree" {
        let object = value
            .as_object_mut()
            .ok_or_else(|| Error::internal("custody_corrupt", "file result differs from declared schema"))?;
        if let Some(reference) = object.get("asset_ref").and_then(Value::as_str) {
            let (artifact, mime, source) = sources.get(reference).ok_or_else(|| {
                Error::conflict("precondition_failed",
                    "executor did not provide native output binding for this asset",
                )
            })?;
            object.insert(
                "digest".into(),
                json!(format!("sha256:{}", artifact.sha256)),
            );
            products.push(stored::RunProduct {
                output: path.into(),
                op: op as i32,
                index,
                content: Some(stored::Ref {
                    digest: digest_bytes(&format!("sha256:{}", artifact.sha256))?,
                    length: artifact.length,
                }),
                media_type: mime.clone(),
                source: Some(source.clone()),
                ..Default::default()
            });
        } else {
            return Err(Error::internal("custody_corrupt", "declared file has no reference"));
        }
    } else if let Some(fields) = schema.get("fields").and_then(Value::as_array) {
        let object = value
            .as_object_mut()
            .ok_or_else(|| Error::internal("custody_corrupt", "result record differs from declared schema"))?;
        for field in fields {
            let name = field
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::internal("custody_corrupt", "result field name absent"))?;
            if let Some(child) = object.get_mut(name) {
                let path = if path.is_empty() {
                    name.to_string()
                } else {
                    format!("{path}.{name}")
                };
                rewrite_assets(
                    child,
                    field
                        .get("type")
                        .ok_or_else(|| Error::internal("custody_corrupt", "result field type absent"))?,
                    &path,
                    op,
                    index,
                    sources,
                    products,
                )?;
            }
        }
    } else if let Some(element) = schema.get("list") {
        let items = value
            .as_array_mut()
            .ok_or_else(|| Error::internal("custody_corrupt", "result list differs from declared schema"))?;
        for (position, child) in items.iter_mut().enumerate() {
            if element.get("asset").is_some() {
                rewrite_assets(
                    child,
                    element,
                    path,
                    stored::RunProductOp::Append,
                    position.try_into().map_err(|_| {
                        Error::validation("out_of_range", "list output index is outside deployed protocol")
                    })?,
                    sources,
                    products,
                )?;
            } else {
                rewrite_assets(
                    child,
                    element,
                    &format!("{path}[{position}]"),
                    op,
                    index,
                    sources,
                    products,
                )?;
            }
        }
    }
    Ok(())
}
fn product_document(product: &stored::RunProduct) -> Result<Vec<u8>, Error> {
    canonical(
        &crate::products::document(product)
            .map_err(|_| Error::internal("custody_corrupt", "product reference absent"))?,
    )
}

fn output_entries(products: &[stored::RunProduct]) -> Result<Vec<Value>, Error> {
    let mut entries = std::collections::BTreeMap::new();
    for product in products {
        let document: Value = serde_json::from_slice(&product_document(product)?)
            .map_err(|_| Error::internal("internal", "product document is invalid"))?;
        let output_id = if product.op == stored::RunProductOp::Append as i32 {
            format!("{}[{}]", product.output, product.index)
        } else {
            product.output.clone()
        };
        let entry = if let Some(checkpoint) = &product.checkpoint {
            json!({"output_id": output_id, "checkpoint": checkpoint})
        } else { json!({
            "output_id":output_id,
            "digest":document["content"]["digest"],
            "length":document["content"]["length"],
            "mime_type":product.media_type,
            "native_tree":document["source"]["source"]
        }) };
        if entries.insert(output_id, entry).is_some() {
            return Err(Error::internal("custody_corrupt", "output binding path is ambiguous"));
        }
    }
    Ok(entries.into_values().collect())
}

/// The outcome document's adjustment rows, as TensorD kept them.
fn adjustments(rows: &Value) -> Vec<v1::Adjustment> {
    let rows: Vec<crate::journal::Adjustment> = serde_json::from_value(rows.clone()).unwrap_or_default();
    rows.into_iter()
        .map(|row| v1::Adjustment {
            field: row.field,
            kind: row.kind.as_str().into(),
            requested: row.requested,
            applied: row.applied,
            reason: row.reason,
            source: row.source,
        })
        .collect()
}

fn spell(bytes: &[u8]) -> String {
    format!("sha256:{}", tensorfs::sha256::hex(bytes))
}

/// Canonical JSON bytes as the wire's raw JSON; none when empty or not JSON.
fn raw(bytes: Vec<u8>) -> Option<v1::Json> {
    String::from_utf8(bytes).ok().and_then(|text| v1::Json::parse(text).ok())
}

/// The current outcome carries a stable code and a plain detail separately.
fn reason(body: &Value) -> v1::Reason {
    let origin = match body["cause"]["origin"].as_u64() {
        Some(1) => "author",
        Some(2) => "runtime",
        Some(6) => "request",
        _ => "machine",
    };
    let mut reason = message_reason(
        body["error_code"].as_str().unwrap_or_default(),
        body["safe_message"].as_str().unwrap_or_default(),
        origin,
    );
    reason.detail = body["failure_detail"].as_str().unwrap_or_default().into();
    reason
}

fn message_reason(code: &str, message: &str, origin: &str) -> v1::Reason {
    if code.is_empty() {
        return v1::Reason {
            code: "failure_record_invalid".into(),
            message: "the recorded failure has no structured error code".into(),
            origin: "machine".into(),
            ..Default::default()
        };
    }
    v1::Reason {
        code: code.into(),
        message: message.into(),
        origin: origin.into(),
        ..Default::default()
    }
}

/// One revision's parts: (digest, length) each.
type Parts = Vec<(Vec<u8>, u64)>;

/// A reader's place in a run's log. It is read from its start, so a reattach numbers each output's
/// revisions and lists the outcome's outputs as the run made them.
#[derive(Default)]
pub(crate) struct Log {
    /// The last stored event read.
    after: u64,
    revisions: HashMap<(String, u32), u64>,
    products: Vec<v1::Product>,
    /// Each item's latest revision as parts (digest, length): a revision whose parts extend
    /// them appends.
    parts: HashMap<(String, u32), Parts>,
}
impl Log {
    fn event(&mut self, event: stored::LogEvent) -> Option<v1::RunEvent> {
        let body: Value =
            serde_json::from_slice(&event.body_canonical_bytes).unwrap_or(Value::Null);
        let kind = match event.kind.as_str() {
            "state" | "running" => {
                let state = body["state"].as_str().unwrap_or("running").replace("starting", "queued");
                v1::Event::State(v1::RunState {
                    // Running from this event: its time counts from here.
                    started_at_ms: if state == "running" { event.at_ms } else { 0 },
                    state,
                    sequence: event.sequence,
                    attempt: event.attempt_ordinal as u32,
                    waiting: body["waiting_reason"].as_str().unwrap_or_default().into(),
                    ..Default::default()
                })
            }
            "progress" => {
                let payload = &body["payload"];
                v1::Event::Progress(v1::Progress {
                    step_ms: payload["step_ms"].as_f64().unwrap_or(0.0),
                    step_steady_ms: payload["step_steady_ms"].as_f64().unwrap_or(0.0),
                    stage: payload["stage"].as_str().unwrap_or_default().into(),
                    fraction: payload["overall_fraction"].as_f64().unwrap_or(-1.0),
                    completed: payload["position"].as_u64().unwrap_or(0),
                    total: payload["total"].as_u64().unwrap_or(0),
                    bytes_done: payload["bytes_done"].as_u64().unwrap_or(0),
                    bytes_total: payload["bytes_total"].as_u64().unwrap_or(0),
                    stage_fraction: payload["stage_fraction"].as_f64(),
                })
            }
            "call" => v1::Event::Call(v1::Call {
                run: body["request"].as_str().unwrap_or_default().into(),
                index: body["index"].as_u64().unwrap_or_default() as u32,
                function: body["export"].as_str().unwrap_or_default().into(),
                label: body["label"].as_str().unwrap_or_default().into(),
                status: body["status"].as_str().unwrap_or_default().into(),
                called_at_ms: body["called_unix_ms"].as_i64().unwrap_or_default(),
                finished_at_ms: body["finished_unix_ms"].as_i64().unwrap_or_default(),
                measurements: match &body["measurements"] {
                    Value::Null => None,
                    measured => raw(crate::boundary_json::exact(measured)),
                },
                memoized: body["memoized"].as_bool().unwrap_or_default(),
                computation_digest: body["computation_digest"].as_str().unwrap_or_default().into(),
                unmemoized: body["unmemoized"].as_str().unwrap_or_default().into(),
                memo_basis: body["memo_basis"].as_str().unwrap_or_default().into(),
                reason: (!body["error_code"].as_str().unwrap_or_default().is_empty()
                    || matches!(
                        body["status"].as_str(),
                        Some("failed" | "canceled" | "refused")
                    ))
                .then(|| {
                    message_reason(
                        body["error_code"].as_str().unwrap_or_default(),
                        body["error"].as_str().unwrap_or_default(),
                        "runtime",
                    )
                }),
            }),
            "log" => match body.get("gpu") {
                Some(gpu) => v1::Event::Gpu(v1::GpuAllocation {
                    state: gpu["state"].as_str().unwrap_or_default().into(),
                    gpus: gpu["gpus"].as_array().into_iter().flatten().filter_map(|g| g.as_u64()).map(|g| g as u32).collect(),
                    width: gpu["width"].as_u64().unwrap_or_default() as u32,
                    behind: gpu["behind"].as_str().unwrap_or_default().into(),
                }),
                None => v1::Event::Log(v1::LogLine {
                    level: body["level"].as_str().unwrap_or("info").into(),
                    text: body["text"].as_str().unwrap_or_default().into(),
                }),
            },
            "product" => {
                let product = event.product?;
                let list = product.op == stored::RunProductOp::Append as i32;
                let index = if list { product.index + 1 } else { 0 };
                let rev = self
                    .revisions
                    .entry((product.output.clone(), index))
                    .or_default();
                *rev += 1;
                let (length, digest) = product.content.as_ref()
                    .map(|content| (content.length, spell(&content.digest)))
                    .unwrap_or_default();
                let parts: Parts = product
                    .parts
                    .iter()
                    .filter_map(|p| p.content.as_ref().map(|c| (c.digest.clone(), c.length)))
                    .collect();
                let previous = self
                    .parts
                    .insert((product.output.clone(), index), parts.clone());
                let appended_from = previous
                    .filter(|before| {
                        !before.is_empty()
                            && before.len() <= parts.len()
                            && parts.starts_with(before)
                    })
                    .map(|before| before.iter().map(|(_, length)| length).sum());
                let converted = v1::Product {
                    output: product.output,
                    checkpoint: product.checkpoint,
                    index,
                    rev: *rev,
                    length,
                    digest,
                    media_type: product.media_type,
                    label: product.label,
                    duration_us: product.parts.iter().map(|p| p.duration_us).sum(),
                    appended_from,
                };
                self.products.retain(|p| {
                    (p.output.as_str(), p.index) != (converted.output.as_str(), converted.index)
                });
                self.products.push(converted.clone());
                v1::Event::Product(converted)
            }
            "outcome" => {
                let outcome = event.outcome?;
                let body: Value =
                    serde_json::from_slice(&outcome.outcome_canonical_bytes).unwrap_or(Value::Null);
                let status = match body["status"].as_u64() {
                    Some(1) => "succeeded",
                    Some(4) => "canceled",
                    _ => "failed",
                };
                let result = body["result"]["inline_result"]
                    .as_str()
                    .and_then(|encoded| STANDARD.decode(encoded).ok())
                    .and_then(raw);
                v1::Event::Outcome(v1::Outcome {
                    status: status.into(),
                    reason: (status != "succeeded").then(|| reason(&body)),
                    result,
                    outputs: self.products.clone(),
                    adjustments: adjustments(&body["adjustments"]),
                    // `Runs::read` adds the measurements, the execution time and the memo.
                    ..Default::default()
                })
            }
            _ => return None,
        };
        Some(v1::RunEvent {
            sequence: event.sequence,
            at_ms: event.at_ms as i64,
            event: kind,
        })
    }
}

impl Runs {
    /// The next events of `actor`'s run `id` after those `log` has read (`api::machine`), waiting
    /// up to 30 s for one. Its outcome ends it.
    pub(crate) fn read(&self, actor: VerifiedActor, id: &str, log: &mut Log) -> Result<Vec<v1::RunEvent>, Error> {
        let page = self.log_page(actor, id, log.after)?;
        let mut events = vec![];
        for event in page.events {
            log.after = log.after.max(event.sequence);
            let Some(mut event) = log.event(event) else { continue };
            if let v1::Event::Outcome(outcome) = &mut event.event {
                let record = self.record(actor, id)?;
                let measured = (self.service.engine)
                    .with_journal(|journal| journal.measurements(&record.id))
                    .map_err(problem)?;
                outcome.measurements = measured.and_then(raw);
                // Its own time running its callable, every attempt summed (0: unknown).
                outcome.execution_ms = record.executed_ms;
                let memo = record.memo;
                (outcome.computation_digest, outcome.memoized, outcome.unmemoized, outcome.memo_basis) =
                    (memo.computation, memo.memoized, memo.unmemoized, memo.basis);
            }
            events.push(event);
        }
        log.after = log.after.max(page.next_after);
        Ok(events)
    }
}
