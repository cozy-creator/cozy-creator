//! One explicitly declared native operation, committed before eager phase entry.
use super::*;

struct Active {
    phase: u64,
    leaf: Option<u64>,
    ended: bool,
    submitted: bool,
    _exclusion: crate::ProducerGuard,
}

enum Progress {
    Committed,
    Active(Active),
    Busy,
    Closed,
}

/// Owned current configuration and selected geometry. This does not materialize weights.
/// Source/binding preparation precedes commit; the producer executes one declared operation.
pub struct Execution {
    device: i32,
    pid: u32,
    site: String,
    key: String,
    choices: Vec<(String, Vec<usize>)>,
    preferred: String,
    selected: SplitSelection,
    progress: Mutex<Progress>,
}

fn field(out: &mut String, value: &str) {
    use std::fmt::Write;
    write!(out, "{}:{value}", value.len()).expect("String write");
}

fn execution_key(
    configuration: &str,
    site: &str,
    choices: &[(String, Vec<usize>)],
    preferred: &str,
    selected: &SplitSelection,
) -> String {
    // Length-prefixing is unambiguous. Configuration is the consumer's current immutable
    // descriptor, not an observed prior key. Region intent/generation is revalidated at
    // entry/begin, but its address-derived name is not compute demand. The fixed choice
    // already records its effect on the operation's allocation geometry.
    let mut key = String::from("varena.closed-eager/1:");
    for value in [configuration, site, preferred, &selected.choice_id] {
        field(&mut key, value);
    }
    field(&mut key, &choices.len().to_string());
    for (id, allocations) in choices {
        field(&mut key, id);
        field(&mut key, &allocations.len().to_string());
        for bytes in allocations {
            field(&mut key, &bytes.to_string());
        }
    }
    key
}

fn inactive(owner: &Working) -> Result<()> {
    let state = owner.lock()?;
    if state.phases.active.is_some() || state.splits.active_is_present() {
        return Err(Error::Invalid(
            "closed execution requires an idle phase boundary",
        ));
    }
    Ok(())
}

impl Execution {
    pub fn commit(
        device: i32,
        configuration: &str,
        site: &str,
        choices: Vec<(String, Vec<usize>)>,
        preferred: &str,
        stream: u64,
    ) -> Result<Self> {
        if configuration.is_empty() || site.is_empty() {
            return Err(Error::Invalid(
                "closed execution requires current configuration and site",
            ));
        }
        let _exclusion = crate::enter_producer(device)?;
        let owner = initialize_owner(device)?;
        inactive(&owner)?;
        let selected = split_select(device, site, choices.clone(), preferred, stream)?;
        inactive(&owner)?;
        Ok(Self {
            device,
            pid: std::process::id(),
            site: site.to_owned(),
            key: execution_key(configuration, site, &choices, preferred, &selected),
            choices,
            preferred: preferred.to_owned(),
            selected,
            progress: Mutex::new(Progress::Committed),
        })
    }

    fn check(&self) -> Result<()> {
        if self.pid != std::process::id() {
            return Err(Error::Invalid(
                "committed executions cannot be used after fork",
            ));
        }
        crate::check_process()
    }

    fn take(&self) -> Result<Progress> {
        self.check()?;
        let mut state = self
            .progress
            .lock()
            .map_err(|_| Error::Poisoned("committed execution mutex".into()))?;
        if matches!(*state, Progress::Busy) {
            return Err(Error::Busy);
        }
        Ok(std::mem::replace(&mut *state, Progress::Busy))
    }

    fn put(&self, state: Progress) -> Result<()> {
        *self
            .progress
            .lock()
            .map_err(|_| Error::Poisoned("committed execution mutex".into()))? = state;
        Ok(())
    }

    pub fn key(&self) -> Result<&str> {
        self.check()?;
        Ok(&self.key)
    }

    pub fn selection(&self) -> Result<&SplitSelection> {
        self.check()?;
        Ok(&self.selected)
    }

    /// Enter exactly this current key. No observed-key substitution is accepted.
    pub fn enter(&self) -> Result<u64> {
        match self.take()? {
            Progress::Committed => {}
            other => {
                self.put(other)?;
                return Err(Error::Invalid("committed execution can enter only once"));
            }
        }
        let result = (|| {
            let exclusion = crate::enter_producer(self.device)?;
            let owner = owner(self.device)?;
            inactive(&owner)?;
            split::validate_committed(&owner, &self.site, self.choices.clone(), &self.preferred)?;
            let lease = crate::plan::Lease::acquire(self.device)?;
            if lease.validate()? != self.selected.plan_generation {
                return Err(Error::Invalid("committed phase-plan generation changed"));
            }
            let phase = phase_enter_inner(
                self.device,
                Some(self.key.clone()),
                PhaseMode::Eager,
                Some((self.site.clone(), self.selected.choice_id.clone())),
            )?;
            Ok(Active {
                phase,
                leaf: None,
                ended: false,
                submitted: false,
                _exclusion: exclusion,
            })
        })();
        match result {
            Ok(active) => {
                let phase = active.phase;
                self.put(Progress::Active(active))?;
                Ok(phase)
            }
            Err(error) => {
                self.put(Progress::Closed)?;
                Err(error)
            }
        }
    }

    /// Revalidate the already-fixed geometry before writes; never select again.
    pub fn begin(&self) -> Result<()> {
        let mut active = match self.take()? {
            Progress::Active(active) if active.leaf.is_none() && !active.ended => active,
            other => {
                self.put(other)?;
                return Err(Error::Invalid("committed operation cannot begin twice"));
            }
        };
        let result = split_begin(
            self.device,
            &self.site,
            &self.selected.choice_id,
            Some(active.phase),
            self.selected.stream,
            Some(self.selected.plan_generation),
        );
        match result {
            Ok(leaf) => {
                active.leaf = Some(leaf);
                self.put(Progress::Active(active))
            }
            Err(error) => {
                let _ = phase_leave(self.device, active.phase, false, None);
                self.put(Progress::Closed)?;
                Err(error)
            }
        }
    }

    pub fn end(&self, submitted: bool) -> Result<SplitObservation> {
        let mut active = match self.take()? {
            Progress::Active(active) if active.leaf.is_some() && !active.ended => active,
            other => {
                self.put(other)?;
                return Err(Error::Invalid("committed operation is not active"));
            }
        };
        let result = split_end(self.device, active.leaf.expect("checked leaf"), submitted);
        if result.is_ok() {
            active.leaf = None;
            active.ended = true;
            active.submitted = submitted;
        }
        self.put(Progress::Active(active))?;
        result
    }

    /// `completed` is the producer's checked same-stream completion proof, not Python return.
    /// The initial Runtime runner synchronizes its whole explicit phase before this call.
    pub fn finish(&self, completed: bool) -> Result<PhaseObservation> {
        let active = match self.take()? {
            Progress::Active(active) if active.ended && active.leaf.is_none() => active,
            other => {
                self.put(other)?;
                return Err(Error::Invalid("committed operation has not ended"));
            }
        };
        let result = phase_leave(
            self.device,
            active.phase,
            completed && active.submitted,
            None,
        );
        self.put(Progress::Closed)?;
        result
    }

    /// Abort metadata; live allocations and stream owners retain their normal lifetimes.
    pub fn abort(&self) -> Result<Option<PhaseObservation>> {
        match self.take()? {
            Progress::Committed | Progress::Closed => {
                self.put(Progress::Closed)?;
                Ok(None)
            }
            Progress::Busy => {
                self.put(Progress::Busy)?;
                Err(Error::Busy)
            }
            Progress::Active(mut active) => {
                if let Some(leaf) = active.leaf.take() {
                    let _ = split_end(self.device, leaf, false);
                }
                let result = phase_leave(self.device, active.phase, false, None);
                self.put(Progress::Closed)?;
                result.map(Some)
            }
        }
    }
}

impl Drop for Execution {
    fn drop(&mut self) {
        if self.pid == std::process::id() {
            let _ = self.abort();
        }
    }
}
