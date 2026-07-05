//! GitHub data → OCEL 2.0 mapping (via the `StagingLog` gate).
//!
//! Objects: `issue` / `pull_request` (shared `owner/repo#N` id space),
//! `user` (`@login`), `repository` (`owner/repo`). Events come from the
//! issue timeline plus PR reviews; every event links its subject
//! (`subject`), actor (`actor`), and repository (`repo`). Timeline kinds we
//! do not model are counted, not silently dropped.
//!
//! Event ids are `<subject>|<kind>` (e.g. `o/r#12|open`, `o/r#12|t123`,
//! `o/r#12|r456`) — `|` cannot appear in repository names, so incremental
//! sync can prune by subject prefix.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use ocel::AttrValue;
use ocel_etl::{StagingEvent, StagingLog};

use crate::models::{Actor, Issue, Review, TimelineEvent};

/// Fallback for events whose author GitHub no longer knows.
const GHOST: &str = "@ghost";

/// Maps one repository's issues and pull requests.
#[derive(Debug)]
pub struct RepoMapper<'a> {
    repo: &'a str,
    /// Issue/PR numbers that exist as objects (current listing, plus any
    /// already in the log on incremental runs). Cross-references are linked
    /// only against this set: numbers of issues that were deleted,
    /// transferred, or converted to discussions still appear as reference
    /// sources but never in the listing, and linking them would dangle.
    known_subjects: BTreeSet<u64>,
    skipped: BTreeMap<String, usize>,
}

fn user_id(actor: Option<&Actor>) -> String {
    actor.map_or_else(|| GHOST.to_owned(), |a| format!("@{}", a.login))
}

impl<'a> RepoMapper<'a> {
    #[must_use]
    pub fn new(repo: &'a str, known_subjects: BTreeSet<u64>) -> Self {
        Self {
            repo,
            known_subjects,
            skipped: BTreeMap::new(),
        }
    }

    /// Timeline kinds seen but not modeled, with counts.
    #[must_use]
    pub fn skipped_kinds(&self) -> &BTreeMap<String, usize> {
        &self.skipped
    }

    /// Register the repository object itself.
    pub fn register(&self, staging: &mut StagingLog) {
        staging.upsert_object(self.repo, "repository");
    }

    /// The object id of issue/PR `number` in this repository.
    #[must_use]
    pub fn subject_id(&self, number: u64) -> String {
        format!("{}#{number}", self.repo)
    }

    /// Map one issue / pull request with its timeline (and reviews for PRs).
    pub fn map_issue(
        &mut self,
        staging: &mut StagingLog,
        issue: &Issue,
        timeline: &[TimelineEvent],
        reviews: &[Review],
    ) {
        let sid = self.subject_id(issue.number);
        let is_pr = issue.is_pull_request();
        let noun = if is_pr { "pull request" } else { "issue" };
        let object_type = if is_pr { "pull_request" } else { "issue" };

        staging.upsert_object(&sid, object_type);
        staging.add_object_attribute(
            &sid,
            "title",
            AttrValue::String(issue.title.clone()),
            issue.created_at,
        );
        staging.add_object_attribute(
            &sid,
            "state",
            AttrValue::String("open".to_owned()),
            issue.created_at,
        );

        self.add_event(
            staging,
            format!("{sid}|open"),
            format!("open {noun}"),
            issue.created_at,
            &sid,
            issue.user.as_ref(),
            vec![],
            vec![],
        );

        for (index, entry) in timeline.iter().enumerate() {
            self.map_timeline_entry(staging, &sid, noun, index, entry);
        }

        for review in reviews {
            let Some(time) = review.submitted_at else {
                continue;
            };
            self.add_event(
                staging,
                format!("{sid}|r{}", review.id),
                "review".to_owned(),
                time,
                &sid,
                review.user.as_ref(),
                vec![(
                    "state".to_owned(),
                    AttrValue::String(review.state.to_lowercase()),
                )],
                vec![],
            );
        }
    }

    fn map_timeline_entry(
        &mut self,
        staging: &mut StagingLog,
        sid: &str,
        noun: &str,
        index: usize,
        entry: &TimelineEvent,
    ) {
        let Some(time) = entry.created_at else {
            return;
        };
        let uid = entry
            .id
            .map_or_else(|| format!("i{index}"), |id| id.to_string());
        let event_id = format!("{sid}|t{uid}");

        match entry.event.as_str() {
            "commented" => {
                self.add_event(
                    staging,
                    event_id,
                    "comment".to_owned(),
                    time,
                    sid,
                    entry.user.as_ref(),
                    vec![],
                    vec![],
                );
            }
            kind @ ("labeled" | "unlabeled") => {
                let verb = if kind == "labeled" {
                    "label"
                } else {
                    "unlabel"
                };
                let name = entry.label.as_ref().map_or("", |l| l.name.as_str());
                self.add_event(
                    staging,
                    event_id,
                    verb.to_owned(),
                    time,
                    sid,
                    entry.actor.as_ref(),
                    vec![("label".to_owned(), AttrValue::String(name.to_owned()))],
                    vec![],
                );
            }
            kind @ ("assigned" | "unassigned") => {
                let verb = if kind == "assigned" {
                    "assign"
                } else {
                    "unassign"
                };
                let mut extra = Vec::new();
                if let Some(assignee) = &entry.assignee {
                    let assignee_id = user_id(Some(assignee));
                    staging.upsert_object(&assignee_id, "user");
                    extra.push((assignee_id, "assignee".to_owned()));
                }
                self.add_event(
                    staging,
                    event_id,
                    verb.to_owned(),
                    time,
                    sid,
                    entry.actor.as_ref(),
                    vec![],
                    extra,
                );
            }
            kind @ ("closed" | "reopened" | "merged") => {
                let (new_state, event_type) = match kind {
                    "closed" => ("closed", format!("close {noun}")),
                    "reopened" => ("open", format!("reopen {noun}")),
                    _ => ("merged", "merge pull request".to_owned()),
                };
                staging.add_object_attribute(
                    sid,
                    "state",
                    AttrValue::String(new_state.to_owned()),
                    time,
                );
                self.add_event(
                    staging,
                    event_id,
                    event_type,
                    time,
                    sid,
                    entry.actor.as_ref(),
                    vec![],
                    vec![],
                );
            }
            "cross-referenced" => self.map_cross_reference(staging, sid, index, time, entry),
            other => {
                *self.skipped.entry(other.to_owned()).or_insert(0) += 1;
            }
        }
    }

    /// Only same-repo references to known subjects are linked. A same-repo
    /// number can still be absent from the log: issues deleted, transferred,
    /// or converted to discussions keep their number in reference sources
    /// but never appear in the listing, and linking them would dangle.
    fn map_cross_reference(
        &mut self,
        staging: &mut StagingLog,
        sid: &str,
        index: usize,
        time: DateTime<Utc>,
        entry: &TimelineEvent,
    ) {
        let source = entry
            .source
            .as_ref()
            .and_then(|s| s.issue.as_ref())
            .filter(|i| {
                i.repository
                    .as_ref()
                    .is_some_and(|r| r.full_name == self.repo)
            });
        let Some(source) = source else {
            *self
                .skipped
                .entry("cross-referenced (other repo)".to_owned())
                .or_insert(0) += 1;
            return;
        };
        if !self.known_subjects.contains(&source.number) {
            *self
                .skipped
                .entry("cross-referenced (missing subject)".to_owned())
                .or_insert(0) += 1;
            return;
        }
        let source_id = self.subject_id(source.number);
        self.add_event(
            staging,
            format!("{sid}|x{index}"),
            "reference".to_owned(),
            time,
            sid,
            entry.actor.as_ref(),
            vec![],
            vec![(source_id, "referenced by".to_owned())],
        );
    }

    #[allow(clippy::too_many_arguments)] // internal builder, call sites read fine
    fn add_event(
        &self,
        staging: &mut StagingLog,
        id: String,
        event_type: String,
        time: DateTime<Utc>,
        sid: &str,
        actor: Option<&Actor>,
        attributes: Vec<(String, AttrValue)>,
        extra_relations: Vec<(String, String)>,
    ) {
        let actor_id = user_id(actor);
        staging.upsert_object(&actor_id, "user");
        let mut relations = vec![
            (sid.to_owned(), "subject".to_owned()),
            (actor_id, "actor".to_owned()),
            (self.repo.to_owned(), "repo".to_owned()),
        ];
        relations.extend(extra_relations);
        staging.add_event(StagingEvent {
            id,
            event_type,
            time,
            attributes,
            relations,
        });
    }
}
