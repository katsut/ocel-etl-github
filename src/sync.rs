//! Incremental sync: merge a fresh pull of updated issues into an existing log.
//!
//! Event ids are `<owner/repo#N>|<kind>`, so everything belonging to a
//! subject can be pruned before its refreshed data is mapped back in. As in
//! ocel-etl-backlog, incremental sync is checkpointed re-writes — OCEL 2.0
//! is a static exchange format.

use std::collections::BTreeSet;

use ocel::Ocel;

/// The subject an event id belongs to (`"o/r#1|open"` → `"o/r#1"`).
fn subject_of(event_id: &str) -> &str {
    event_id.split('|').next().unwrap_or(event_id)
}

/// Drop everything belonging to the subjects about to be refreshed. Objects
/// referenced only by pruned events disappear too and come back with the
/// refreshed mapping.
#[must_use]
pub fn prune_refreshed(existing: &Ocel, refreshed: &BTreeSet<String>) -> Ocel {
    existing.filter_events(|e| !refreshed.contains(subject_of(&e.id)))
}
