# CLAUDE.md — ocel-etl-github

GitHub connector: issue/PR timelines + reviews → OCEL, with incremental
sync. Binary `ocel-github`, connector contract v1/v2. Works **anonymously**
on public repos (60 req/h) — real-data E2E needs no credentials. Concepts in
[ARCHITECTURE.md](ARCHITECTURE.md).

## Build, test, verify

```sh
cargo test          # transport is scripted — no network
cargo clippy --all-targets -- -D warnings && cargo fmt --check
cargo run --release -- pull --repo owner/name --out out.sqlite   # GITHUB_TOKEN optional
```

After changing the binary: `cargo install --path .` (studio resolves it from
PATH). Long pulls of big repos: run detached (nohup) — rate-limit sleeps can
exceed foreground timeouts.

## Map

- `src/client.rs` — REST client over a transport trait: paging, retry on
  transient errors, 429/403 backoff **capped at 120 s** with every sleep
  logged to stderr (an uncapped X-RateLimit-Reset once slept 80 silent
  minutes)
- `src/models.rs` — timeline/review deserialization
- `src/mapper.rs` — objects: issue/pull_request (`owner/repo#N`), user
  (`@login`, deleted authors → `@ghost`), repository; events from timeline +
  reviews (open/comment/label/assign/review/close/merge/reopen/reference);
  every event carries subject + actor + repo relations; unknown timeline
  kinds are counted, never dropped silently
- `src/sync.rs` — incremental via event-id prefix (`<subject>|<kind>` — `|`
  cannot appear in repo names)
- `src/main.rs` — CLI (`pull`, `--no-comment-bodies`, `--since`/`--full`),
  NDJSON progress

## Invariants and traps

- Cross-references only become `reference` events when the target subject
  is in the pulled set — issues moved/deleted/converted to discussions
  otherwise produce dangling E2O and fail the gate (bitten once; now
  counted as `cross-referenced (missing subject)`).
- Comment bodies default **on** (public data; `--no-comment-bodies` to
  drop) — the Backlog connector is the opposite.
- Issue/PR `state` is a dynamic attribute (open/closed/merged).
- Incremental correctness bar: equality with a full re-pull.

## Conventions

Issue → branch → PR → CI green → squash-merge. Unpublished (PATH binary).
Design docs live in the private ocel-workspace (`docs/etl-github/`).
