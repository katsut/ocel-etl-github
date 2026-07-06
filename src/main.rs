use std::collections::BTreeSet;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::{DateTime, NaiveDate, Utc};
use clap::{Parser, Subcommand};
use ocel_etl::StagingLog;
use ocel_etl_github::client::GithubClient;
use ocel_etl_github::mapper::RepoMapper;
use ocel_etl_github::sync::prune_refreshed;

/// GitHub → OCEL 2.0 extraction.
#[derive(Debug, Parser)]
#[command(name = "ocel-github", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Pull repositories' issue/PR history into an OCEL 2.0 file.
    ///
    /// `GITHUB_TOKEN` is optional: public repositories work anonymously
    /// (60 requests/hour), a token raises the limit and unlocks private
    /// repositories. When the output file already exists, only issues
    /// updated since its newest event are refreshed and merged in
    /// (incremental sync).
    Pull {
        /// Repository as owner/name; repeat or comma-separate for several
        /// (e.g. --repo katsut/ocel-mine,katsut/ocel-studio).
        #[arg(long = "repo", value_delimiter = ',', required = true)]
        repos: Vec<String>,
        /// Output file (.json/.jsonocel, .sqlite/.db, .xml/.xmlocel).
        #[arg(long)]
        out: PathBuf,
        /// Only refresh issues updated at or after this time
        /// (RFC 3339 or YYYY-MM-DD). Defaults to the newest event in --out.
        #[arg(long)]
        since: Option<String>,
        /// Ignore any existing --out file and pull everything.
        #[arg(long)]
        full: bool,
        /// Do not store comment text as a `body` event attribute.
        #[arg(long)]
        no_comment_bodies: bool,
    },
}

// --- connector contract v2: NDJSON progress events on stdout -----------------

fn emit(value: &serde_json::Value) {
    println!("{value}");
}

fn emit_progress(stage: &str, done: usize, total: Option<usize>) {
    let mut event = serde_json::json!({"event": "progress", "stage": stage, "done": done});
    if let Some(total) = total {
        event["total"] = total.into();
    }
    emit(&event);
}

fn emit_log(message: &str) {
    emit(&serde_json::json!({"event": "log", "level": "info", "message": message}));
}

fn emit_done(events: usize, objects: usize) {
    emit(&serde_json::json!({"event": "done", "events": events, "objects": objects}));
}

// -----------------------------------------------------------------------------

fn parse_since(s: &str) -> Result<DateTime<Utc>, Box<dyn Error>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.to_utc());
    }
    let date = NaiveDate::parse_from_str(s, "%Y-%m-%d")?;
    Ok(date
        .and_hms_opt(0, 0, 0)
        .expect("midnight is valid")
        .and_utc())
}

fn pull(
    repos: &[String],
    out: &Path,
    since_arg: Option<&str>,
    full: bool,
    comment_bodies: bool,
) -> Result<(), Box<dyn Error>> {
    for repo in repos {
        let valid = repo.split('/').count() == 2 && !repo.contains('|');
        if !valid {
            return Err(format!("not an owner/name repository: {repo}").into());
        }
    }
    let client = GithubClient::from_env();

    let existing = if !full && out.exists() {
        eprintln!("existing log found: {}", out.display());
        Some(ocel::io::read_path(out)?)
    } else {
        None
    };
    let since: Option<DateTime<Utc>> = match (since_arg, &existing) {
        (Some(s), _) => Some(parse_since(s)?),
        (None, Some(log)) => log.events.iter().map(|e| e.time).max(),
        (None, None) => None,
    };
    if let Some(s) = since {
        eprintln!("incremental: refreshing issues updated at/after {s}");
    }

    // pass 1: updated-issue listings decide what to refresh
    let mut per_repo = Vec::with_capacity(repos.len());
    let mut refreshed: BTreeSet<String> = BTreeSet::new();
    for repo in repos {
        emit_progress("issues", 0, None);
        let issues = client.issues(repo, since)?;
        emit_log(&format!("{repo}: {} updated issues/PRs", issues.len()));
        eprintln!("{repo}: {} updated issues/PRs", issues.len());
        for issue in &issues {
            refreshed.insert(format!("{repo}#{}", issue.number));
        }
        per_repo.push((repo, issues));
    }

    // base: the existing log minus everything belonging to refreshed subjects
    let mut staging = match &existing {
        Some(log) => StagingLog::from_ocel(prune_refreshed(log, &refreshed)),
        None => StagingLog::new(),
    };

    // pass 2: timelines (and reviews for PRs), streamed per subject
    for (repo, issues) in &per_repo {
        // cross-references may only link subjects that exist: the current
        // listing, plus subjects already in the log on incremental runs
        let mut known: BTreeSet<u64> = issues.iter().map(|i| i.number).collect();
        if let Some(log) = &existing {
            let prefix = format!("{repo}#");
            known.extend(
                log.objects
                    .iter()
                    .filter_map(|o| o.id.strip_prefix(&prefix))
                    .filter_map(|n| n.parse::<u64>().ok()),
            );
        }
        let mut mapper = RepoMapper::new(repo, known, comment_bodies);
        mapper.register(&mut staging);
        let total = issues.len();
        for (index, issue) in issues.iter().enumerate() {
            let timeline = client.timeline(repo, issue.number)?;
            let reviews = if issue.is_pull_request() {
                client.reviews(repo, issue.number)?
            } else {
                Vec::new()
            };
            mapper.map_issue(&mut staging, issue, &timeline, &reviews);
            emit_progress(repo, index + 1, Some(total));
        }
        if !mapper.skipped_kinds().is_empty() {
            let summary: Vec<String> = mapper
                .skipped_kinds()
                .iter()
                .map(|(kind, n)| format!("{kind} x{n}"))
                .collect();
            eprintln!("  skipped timeline kinds: {}", summary.join(", "));
        }
    }

    let log = staging
        .into_ocel()
        .map_err(|violations| format!("staged data is not a valid OCEL log: {violations:?}"))?;
    eprintln!(
        "log: {} events / {} objects",
        log.events.len(),
        log.objects.len()
    );

    ocel::io::write_path(&log, out)?;
    eprintln!("wrote {}", out.display());
    emit_done(log.events.len(), log.objects.len());
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Pull {
            repos,
            out,
            since,
            full,
            no_comment_bodies,
        } => match pull(&repos, &out, since.as_deref(), full, !no_comment_bodies) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("error: {err}");
                ExitCode::FAILURE
            }
        },
    }
}
