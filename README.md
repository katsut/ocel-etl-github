# ocel-etl-github

GitHub → [OCEL 2.0](https://www.ocel-standard.org/) extraction: mine your
development process — issues, pull requests, reviews, the people connecting
them — as an object-centric event log.

```sh
# public repos need no token; GITHUB_TOKEN raises rate limits / unlocks private repos
ocel-github pull --repo katsut/ocel-mine --out gh.sqlite
```

The output opens directly in [ocel-studio](https://github.com/katsut/ocel-studio)
or any OCEL 2.0 tool. When `--out` exists, only issues updated since its newest
event are refreshed (incremental sync); `--full` forces a complete pull.
Progress is emitted as [contract-v2 NDJSON](https://github.com/katsut/ocel-studio/blob/main/docs/connector-contract.md)
on stdout, so the studio shows a live bar.

## Mapping

- **Objects**: `issue`, `pull_request` (shared `owner/repo#N` id space),
  `user` (`@login`), `repository`
- **Events**: open / comment / label / assign / review / close / merge /
  reopen / reference — every event links its subject, actor, and repository
- Timeline kinds that are not modeled are counted and reported, never
  silently dropped

See [ARCHITECTURE.md](ARCHITECTURE.md) for the full mapping and sync
semantics.

## The ocel family

| Layer | Repo | License |
|---|---|---|
| Core model, I/O, validation | [ocel-rs](https://github.com/katsut/ocel-rs) (crates.io: [`ocel`](https://crates.io/crates/ocel)) | MIT |
| ETL engine (StagingLog → OCEL) | [ocel-etl](https://github.com/katsut/ocel-etl) | MIT |
| Backlog connector | [ocel-etl-backlog](https://github.com/katsut/ocel-etl-backlog) | MIT |
| **GitHub connector (this repo)** | ocel-etl-github | MIT |
| Analysis library | [ocel-mine](https://github.com/katsut/ocel-mine) (crates.io: [`ocel-mine`](https://crates.io/crates/ocel-mine)) | MIT |
| Studio — UI + data sources | [ocel-studio](https://github.com/katsut/ocel-studio) | ELv2 |

## License

MIT
