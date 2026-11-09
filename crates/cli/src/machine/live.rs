//! `cozy machine ls` as a live view (`crate::tui`), fed by Calcifer's events.
use api::{
    calcifer::{Machine, MachineList, MachineState, StreamEvent},
    error::{Error, Kind},
};

use tokio::task::JoinHandle;

use super::{list, route};
use crate::{
    Context,
    calcifer_client::{Client, Watch},
};

/// One machine as Calcifer says it changes, known by its rental id where it has one: the Hub may
/// rename a rental while it boots.
pub(crate) struct Following {
    watch: Watch<StreamEvent>,
    name: String,
    rental_id: Option<String>,
    /// Subscribing and reading again, which takes as long as the machine takes to answer: a live
    /// view's next redraw drops `next`, and this goes on.
    again: Option<JoinHandle<Result<(Watch<StreamEvent>, Machine), Error>>>,
}

impl Following {
    /// Subscribes, then reads the machine (by name, alias or rental id): a change in between is
    /// in the stream.
    pub(crate) async fn open(calcifer: &Client, name: &str) -> Result<(Self, Machine), Error> {
        let (watch, machine) = Self::read(calcifer, name).await?;
        let following = Self {
            watch,
            name: machine.name.clone(),
            rental_id: machine.rental_id.clone(),
            again: None,
        };
        Ok((following, machine))
    }

    async fn read(calcifer: &Client, name: &str) -> Result<(Watch<StreamEvent>, Machine), Error> {
        let watch = calcifer.changes().await?;
        let machine = calcifer.get(&route(name)?).await?;
        Ok((watch, machine))
    }

    /// For a live view: Calcifer's silence goes in its title (`Watch::silence`), not in lines.
    pub(crate) fn quiet(self) -> Self {
        Self { watch: self.watch.quiet(), ..self }
    }

    /// The machine as it next changes; `None` once it left the list.
    pub(crate) async fn next(&mut self, calcifer: &Client) -> Result<Option<Machine>, Error> {
        loop {
            match self.watch.change().await {
                Some(StreamEvent::MachineUpdated { machine }) if self.is(&machine) => {
                    self.name.clone_from(&machine.name);
                    return Ok(Some(*machine));
                }
                Some(StreamEvent::MachineRemoved { name }) if name == self.name => return Ok(None),
                Some(_) => {}
                // A stream that ended (Calcifer restarted, or this fell behind) may have missed a
                // change: subscribe and read again. One no longer named here has left the list.
                None => {
                    let again = self.again.get_or_insert_with(|| {
                        let id = self.rental_id.clone().unwrap_or_else(|| self.name.clone());
                        let calcifer = calcifer.clone();
                        tokio::spawn(async move { Self::read(&calcifer, &id).await })
                    });
                    let read = again.await;
                    self.again = None;
                    let read = read.map_err(|e| Error::new(Kind::Internal, "follow_failed", e.to_string()))?;
                    let (mut watch, machine) = match read {
                        Err(e) if e.kind == Kind::NotFound => return Ok(None),
                        read => read?,
                    };
                    watch.quiet = self.watch.quiet;
                    self.watch = watch;
                    self.name.clone_from(&machine.name);
                    self.rental_id.clone_from(&machine.rental_id);
                    return Ok(Some(machine));
                }
            }
        }
    }

    fn is(&self, machine: &Machine) -> bool {
        match &self.rental_id {
            Some(id) => machine.rental_id.as_ref() == Some(id),
            None => machine.name == self.name,
        }
    }
}

/// Every machine as `cozy machine ls` shows it, kept as Calcifer says it changes.
pub(crate) struct Machines {
    list: MachineList,
    watch: Watch<StreamEvent>,
    /// The Hubs' answer: until it comes, what is drawn is their last word.
    asked: Option<JoinHandle<Result<MachineList, Error>>>,
}

impl Machines {
    /// Subscribes, then lists what Calcifer holds, so a slow Hub holds up no draw, and asks the
    /// Hubs once behind it. A change in between is in the stream.
    pub(crate) async fn open(ctx: &Context) -> Result<Self, Error> {
        let calcifer = ctx.calcifer().await?;
        let watch = calcifer.changes().await?.quiet();
        let list = calcifer.get("/v1/machines").await?;
        let asked = tokio::spawn(async move { calcifer.get("/v1/machines?refresh=true").await });
        Ok(Self { list, watch, asked: Some(asked) })
    }

    pub(crate) async fn changed(&mut self, ctx: &Context) -> Result<(), Error> {
        loop {
            let (asked, watch) = (&mut self.asked, &mut self.watch);
            let event = tokio::select! {
                answer = async { match asked { Some(asked) => asked.await, None => std::future::pending().await } } => {
                    self.asked = None;
                    if let Ok(Ok(list)) = answer {
                        self.list = list;
                    }
                    return Ok(());
                }
                event = watch.change() => event,
            };
            match event {
                Some(StreamEvent::MachineUpdated { machine }) => {
                    let machines = &mut self.list.machines;
                    match machines.iter_mut().find(|m| m.name == machine.name) {
                        Some(held) => *held = *machine,
                        None => machines.push(*machine),
                    }
                }
                Some(StreamEvent::MachineRemoved { name }) => self.list.machines.retain(|m| m.name != name),
                // What isn't a machine changes nothing here.
                Some(_) => continue,
                // A stream that ended (Calcifer restarted, or this view fell behind) may have
                // missed a change: subscribe and list again.
                None => *self = Self::open(ctx).await?,
            }
            return Ok(());
        }
    }

    pub(crate) fn title(&self) -> String {
        let note = super::local_note().map(|n| format!(" · {n}")).unwrap_or_default();
        format!("machines{note}") + &self.watch.silence().map(|s| format!(" · {s}")).unwrap_or_default()
    }

    pub(crate) fn headers(&self) -> Vec<&'static str> {
        list::HEADERS.to_vec()
    }

    pub(crate) fn rows(&self, ctx: &Context) -> Vec<Vec<String>> {
        list::rows(ctx, &self.list)
    }

    pub(crate) fn list(&self) -> &MachineList {
        &self.list
    }

    /// The machine on row `selected` (the rows are the live machines), by a name that keeps
    /// resolving: its rental id where it has one.
    pub(crate) fn id(&self, selected: usize) -> Option<String> {
        let mut live = self
            .list
            .machines
            .iter()
            .filter(|m| m.state != MachineState::Ended);
        let machine = live.nth(selected)?;
        Some(
            machine
                .rental_id
                .clone()
                .unwrap_or_else(|| machine.name.clone()),
        )
    }
}

/// One machine as `cozy machine show` draws it, kept as Calcifer says it changes.
pub(crate) struct Shown {
    calcifer: Client,
    following: Following,
    machine: Machine,
}

impl Shown {
    pub(crate) async fn open(ctx: &Context, name: &str) -> Result<Self, Error> {
        let calcifer = ctx.calcifer().await?;
        let (following, machine) = Following::open(&calcifer, name).await?;
        Ok(Self {
            calcifer,
            following: following.quiet(),
            machine,
        })
    }

    /// Cancel-safe, as [`Following::next`] is.
    pub(crate) async fn changed(&mut self) -> Result<(), Error> {
        match self.following.next(&self.calcifer).await? {
            Some(machine) => self.machine = machine,
            // It left the list: nothing more will change.
            None => std::future::pending().await,
        }
        Ok(())
    }

    pub(crate) fn title(&self) -> String {
        let silence = self.following.watch.silence();
        format!("machine {}", self.machine.name) + &silence.map(|s| format!(" · {s}")).unwrap_or_default()
    }

    pub(crate) fn lines(&self, ctx: &Context) -> Vec<String> {
        list::record(ctx, &self.machine).lines()
    }
}
