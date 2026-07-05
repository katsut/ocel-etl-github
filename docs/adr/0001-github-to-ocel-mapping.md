# ADR 0001: GitHub → OCEL 2.0 mapping

- **Date:** 2026-07-05
- **Status:** Accepted

## Context

Second connector of the ocel family, after
[ocel-etl-backlog](https://github.com/katsut/ocel-etl-backlog). Two reasons it
comes now: the roadmap names GitHub as the next source, and — unlike Backlog —
it is **verifiable end-to-end today**: public repositories need no credential,
and the ocel family's own development history is a real, honest test log
(dogfooding: mine the process that builds the miner).

Everything follows the connector CLI contract
([v1 + v2](https://github.com/katsut/ocel-studio/blob/main/docs/connector-contract.md)):
credentials via environment, `--out` incremental, exit 0, human diagnostics on
stderr, NDJSON progress on stdout. This connector emits v2 progress from day
one.

## Decisions

### 1. Objects: issue, pull_request, user, repository

| Object type | id | Notes |
|---|---|---|
| `issue` | `owner/repo#123` | dynamic attributes: `state`, `title`; static: `number` |
| `pull_request` | `owner/repo#456` | same id space as issues (GitHub numbers them together); dynamic `state` (open/closed/merged), `title`; static `number` |
| `user` | `@login` | the actor of every event — deliberately convergent, like `employees` in the official sample |
| `repository` | `owner/repo` | one per pulled repo; every event links it |

Labels, milestones and assignees are **attributes/events, not objects** for
now (the label taxonomy varies too much across repos to commit to an object
type in the MVP).

O2O: `pull_request --closes--> issue` from cross-references where GitHub
reports the closing relationship; `issue --sub_issue_of--> issue` when the
sub-issue API exposes it (best-effort).

### 2. Events: from the issue timeline, named in plain verbs

One vocabulary for issues and PRs, distinguished by the object they touch:

| Event type | Source (timeline / API) |
|---|---|
| `open issue` / `open pull request` | `created_at` of the issue/PR |
| `comment` | timeline `commented` |
| `label` / `unlabel` | timeline `labeled` / `unlabeled` (label name as event attribute) |
| `assign` / `unassign` | timeline `assigned` / `unassigned` (assignee also E2O-linked as `user`) |
| `review` | pulls reviews API (`state` = approved / changes_requested / commented as event attribute) |
| `close issue` / `close pull request` | timeline `closed` |
| `merge pull request` | timeline `merged` |
| `reopen issue` / `reopen pull request` | timeline `reopened` |
| `reference` | timeline `cross-referenced` (links the referencing PR/issue only when its number is a *known subject*: the current listing plus, on incremental runs, subjects already in the log. Same-repo numbers can be absent — issues deleted, transferred, or converted to discussions keep their number in reference sources but never appear in the listing; found the hard way on sharkdp/fd, where linking them dangled hundreds of E2O relations) |

Every event carries E2O links: the subject (`issue`/`pull_request`,
qualifier `subject`), the actor (`user`, qualifier `actor`), and the
`repository` (qualifier `repo`). Commits/pushes are out of scope for the MVP
(`synchronize` timeline noise; a `commit` object type is a candidate follow-up
once real questions need it).

### 3. API and increments

REST v3. `GET /repos/{o}/{r}/issues?state=all&sort=updated&since=...` lists
issues **and** PRs together (GitHub's model); per issue,
`GET /repos/{o}/{r}/issues/{n}/timeline` yields the events above, and for PRs
`GET /repos/{o}/{r}/pulls/{n}/reviews` adds reviews. Incremental sync mirrors
ocel-etl-backlog: when `--out` exists, only issues updated since its newest
event are re-fetched, their old events pruned and remapped
(`prune_refreshed`-style), so a cron loop stays cheap.

`GITHUB_TOKEN` is **optional**: public repos work anonymously (60 req/h — fine
for small repos), a token raises the limit to 5,000 req/h and unlocks private
repos. That keeps the zero-credential demo path real while following the
contract's env-credential rule when a token is present.

### 4. CLI

```
ocel-github pull --repo katsut/ocel-mine[,katsut/ocel-studio...] --out gh.sqlite [--since 2026-01-01] [--full]
```

Mirrors `ocel-backlog pull` exactly, so the studio's source mechanism (and a
future GitHub preset) needs nothing new.

## Out of scope, recorded deliberately

**Unstructured organizational knowledge** (Slack threads, wiki pages, meeting
notes) is where much of the real process lives — and it is out of scope for
*connectors*. The architecture's answer, kept from the workspace roadmap: the
structured sources (Backlog, GitHub) form a **backbone of reliable timestamps
and identities**; unstructured sources attach to that backbone later as
*enrichment* — an adapter that extracts references ("this thread discusses
PR #123") into E2O/O2O links with `provenance = llm`, rather than trying to
mine free text into a standalone event log (which per the noisy-log survey is
still research-grade). Connectors stay deterministic; interpretation gets its
own layer with its own honesty labels.
