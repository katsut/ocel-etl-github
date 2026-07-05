use std::collections::BTreeSet;

use chrono::{DateTime, TimeZone, Utc};
use ocel::AttrValue;
use ocel_etl::StagingLog;
use ocel_etl_github::mapper::RepoMapper;
use ocel_etl_github::models::{
    Actor, CrossRefIssue, CrossRefSource, Issue, Label, PullRequestMarker, RepoRef, Review,
    TimelineEvent,
};

fn ts(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 9, minute, 0).unwrap()
}

fn actor(login: &str) -> Actor {
    Actor {
        login: login.into(),
    }
}

fn issue(number: u64, pr: bool) -> Issue {
    Issue {
        number,
        title: format!("thing {number}"),
        state: "open".into(),
        user: Some(actor("alice")),
        created_at: ts(0),
        updated_at: ts(30),
        pull_request: pr.then_some(PullRequestMarker { merged_at: None }),
    }
}

fn mapper<'a>(repo: &'a str, known: &[u64]) -> RepoMapper<'a> {
    RepoMapper::new(repo, known.iter().copied().collect::<BTreeSet<u64>>())
}

fn entry(event: &str, minute: u32) -> TimelineEvent {
    TimelineEvent {
        event: event.into(),
        id: Some(u64::from(minute)),
        actor: Some(actor("bob")),
        user: Some(actor("bob")),
        created_at: Some(ts(minute)),
        label: None,
        assignee: None,
        source: None,
    }
}

#[test]
fn issue_lifecycle_maps_to_events_and_state() {
    let mut staging = StagingLog::new();
    let mut m = mapper("o/r", &[1, 2]);
    m.register(&mut staging);

    let mut labeled = entry("labeled", 5);
    labeled.label = Some(Label { name: "bug".into() });
    let timeline = vec![
        entry("commented", 2),
        labeled,
        entry("closed", 10),
        entry("reopened", 20),
        entry("closed", 25),
    ];
    m.map_issue(&mut staging, &issue(1, false), &timeline, &[]);

    let log = staging.into_ocel().expect("valid log");
    let types: Vec<&str> = log.events.iter().map(|e| e.event_type.as_str()).collect();
    assert_eq!(
        types,
        vec![
            "open issue",
            "comment",
            "label",
            "close issue",
            "reopen issue",
            "close issue"
        ]
    );

    // every event links subject, actor, and repo
    for event in &log.events {
        let quals: Vec<&str> = event
            .relationships
            .iter()
            .map(|r| r.qualifier.as_str())
            .collect();
        assert!(quals.contains(&"subject"));
        assert!(quals.contains(&"actor"));
        assert!(quals.contains(&"repo"));
    }

    // dynamic state: open at creation, closed at the end
    let subject = log.objects.iter().find(|o| o.id == "o/r#1").unwrap();
    assert_eq!(subject.object_type, "issue");
    assert_eq!(
        subject.attribute_at("state", ts(1)),
        Some(&AttrValue::String("open".into()))
    );
    assert_eq!(
        subject.attribute_at("state", ts(59)),
        Some(&AttrValue::String("closed".into()))
    );

    // objects: issue + repository + alice + bob
    assert_eq!(log.objects.len(), 4);
}

#[test]
fn pull_request_gets_reviews_and_merge() {
    let mut staging = StagingLog::new();
    let mut m = mapper("o/r", &[1, 2]);
    m.register(&mut staging);

    let reviews = vec![Review {
        id: 900,
        user: Some(actor("carol")),
        state: "APPROVED".into(),
        submitted_at: Some(ts(7)),
    }];
    let timeline = vec![entry("merged", 9)];
    m.map_issue(&mut staging, &issue(2, true), &timeline, &reviews);

    let log = staging.into_ocel().expect("valid log");
    let types: Vec<&str> = log.events.iter().map(|e| e.event_type.as_str()).collect();
    assert_eq!(
        types,
        vec!["open pull request", "merge pull request", "review"]
    );

    let review = log
        .events
        .iter()
        .find(|e| e.event_type == "review")
        .unwrap();
    assert_eq!(
        review.attributes[0].value,
        AttrValue::String("approved".into())
    );

    let subject = log.objects.iter().find(|o| o.id == "o/r#2").unwrap();
    assert_eq!(subject.object_type, "pull_request");
    assert_eq!(
        subject.attribute_at("state", ts(59)),
        Some(&AttrValue::String("merged".into()))
    );
}

#[test]
fn same_repo_cross_reference_links_and_foreign_is_counted() {
    let mut staging = StagingLog::new();
    let mut m = mapper("o/r", &[1, 2]);
    m.register(&mut staging);

    // the referencing PR must exist in the log
    m.map_issue(&mut staging, &issue(2, true), &[], &[]);

    let mut same_repo = entry("cross-referenced", 3);
    same_repo.source = Some(CrossRefSource {
        issue: Some(CrossRefIssue {
            number: 2,
            repository: Some(RepoRef {
                full_name: "o/r".into(),
            }),
        }),
    });
    let mut foreign = entry("cross-referenced", 4);
    foreign.source = Some(CrossRefSource {
        issue: Some(CrossRefIssue {
            number: 9,
            repository: Some(RepoRef {
                full_name: "other/repo".into(),
            }),
        }),
    });
    m.map_issue(&mut staging, &issue(1, false), &[same_repo, foreign], &[]);

    assert_eq!(
        m.skipped_kinds().get("cross-referenced (other repo)"),
        Some(&1)
    );

    let log = staging.into_ocel().expect("valid log");
    let reference = log
        .events
        .iter()
        .find(|e| e.event_type == "reference")
        .unwrap();
    assert!(reference
        .relationships
        .iter()
        .any(|r| r.object_id == "o/r#2" && r.qualifier == "referenced by"));
}

#[test]
fn same_repo_reference_to_missing_subject_is_counted_not_linked() {
    // #55 was deleted / converted to a discussion: same repo, but absent
    // from the listing and therefore not a known subject
    let mut staging = StagingLog::new();
    let mut m = mapper("o/r", &[1, 2]);
    m.register(&mut staging);

    let mut ghost_ref = entry("cross-referenced", 3);
    ghost_ref.source = Some(CrossRefSource {
        issue: Some(CrossRefIssue {
            number: 55,
            repository: Some(RepoRef {
                full_name: "o/r".into(),
            }),
        }),
    });
    m.map_issue(&mut staging, &issue(1, false), &[ghost_ref], &[]);

    assert_eq!(
        m.skipped_kinds().get("cross-referenced (missing subject)"),
        Some(&1)
    );
    let log = staging.into_ocel().expect("valid log — nothing dangles");
    assert!(log.events.iter().all(|e| e.event_type != "reference"));
}

#[test]
fn unknown_timeline_kinds_are_counted_not_dropped_silently() {
    let mut staging = StagingLog::new();
    let mut m = mapper("o/r", &[1, 2]);
    m.register(&mut staging);
    m.map_issue(
        &mut staging,
        &issue(1, false),
        &[entry("committed", 3), entry("committed", 4)],
        &[],
    );
    assert_eq!(m.skipped_kinds().get("committed"), Some(&2));
}
