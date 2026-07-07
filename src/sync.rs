//! Incremental sync: merge a fresh pull of updated issues into an existing log.
//!
//! Event ids are `<owner/repo#N>|<kind>`, so everything belonging to a
//! subject can be pruned before its refreshed data is mapped back in. As in
//! ocel-etl-backlog, incremental sync is checkpointed re-writes — OCEL 2.0
//! is a static exchange format.

use std::collections::BTreeSet;

use ocel::Ocel;
use ocel_etl::StagingLog;

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

/// Re-add `closes` links from refreshed pull requests to unrefreshed issues.
///
/// The link lives on the PR object, which is rebuilt when the PR is
/// refreshed — but only the closed issue's timeline knows the relation, so
/// issues outside the refresh set must contribute it from the existing log.
pub fn repair_closes_links(
    staging: &mut StagingLog,
    existing: &Ocel,
    refreshed: &BTreeSet<String>,
) {
    for object in &existing.objects {
        if object.object_type != "pull_request" || !refreshed.contains(&object.id) {
            continue;
        }
        for rel in &object.relationships {
            if rel.qualifier == "closes" && !refreshed.contains(&rel.object_id) {
                staging.add_o2o(&object.id, &rel.object_id, "closes");
            }
        }
    }
}
