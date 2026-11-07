//! The one fit rule: whether a model runs on a card of a given capacity, as varena runs it. Pure:
//! the machine's lane chooser and placement ask it with a card's capacity, never its free bytes,
//! so the same model gets the same answer on the same card.

/// What a model needs of a card, from its install's facts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    /// Every weight byte its lane holds.
    pub weights: u64,
    /// Its largest unit of weights, the most one block holds at once when it streams.
    pub largest_unit: u64,
    /// The working memory its smallest split level needs beside its weights.
    pub working_floor: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// Its weights stay on the card beside its working floor.
    Resident,
    /// Its weights stream one unit at a time beside its working floor.
    Streamed,
    /// Not even one unit fits beside its working floor.
    No,
}

/// How a model with `profile` runs on a card of `capacity` bytes (its cap, if it has one).
pub fn fit(capacity: u64, profile: Profile) -> Fit {
    let floor = profile.working_floor;
    if profile.weights.saturating_add(floor) <= capacity {
        Fit::Resident
    } else if profile.largest_unit.saturating_add(floor) <= capacity {
        Fit::Streamed
    } else {
        Fit::No
    }
}

/// Whether a model with `profile` runs at all on a card of `capacity` bytes.
pub fn feasible(capacity: u64, profile: Profile) -> bool {
    fit(capacity, profile) != Fit::No
}
