# Changelog

All notable changes to this project are recorded here. The format follows [Keep a Changelog 2.0.0](https://keepachangelog.com/en/2.0.0/), and this project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed

- The Dockerfile and compose healthchecks send the bearer token with their `/readyz` probe, so a deployment that sets `OWNSEARCH_BEARER_TOKENS` no longer reports unhealthy forever.
- The scanner policy, the README, the threat model, and SECURITY.md state where the headless browser runs and what `OWNSEARCH_CAPSOLVER_KEY` does, and they name the CapSolver service as an outbound boundary.

## [0.2.0] - 2026-10-27

### Fixed

- The scanner and both server transports no longer call the state database on the async runtime. Every store call runs on the blocking pool, so a busy database can no longer park a worker thread.
- The Dockerfile and compose healthchecks call `/readyz`, which reads the state database, instead of `/healthz`, which always answers.

### Changed

- rmcp 3.7.0 to 3.8.0, plus brotli, openssl-probe, and rustix patch updates.
- The compose service runs with a read-only root filesystem, a `/tmp` tmpfs, all capabilities dropped, fixed volume names, and a container user created with `--no-log-init`.

## [0.1.2] - 2026-10-27

### Fixed

- A keychain failure at construction or at read no longer stops the server. It logs a warning and reads no token, and the non-loopback bind guard still refuses to serve without a bearer token.
- The startup test prints the error it receives when the public bind check fails.

## [0.1.1] - 2026-10-27

### Fixed

- The server starts on a machine with no keychain service instead of refusing to serve, which unblocked headless servers and the release gate.
- The MCP registry and bundle descriptions fit the 100 character registry limit.
- The release script stamps the hoisted `ownsearch-core` version.

## [0.1.0] - 2026-10-27

### Added

- MCP server over stdio and Streamable HTTP, serving the 2026-07-28 and 2025-11-25 protocol versions.
- The MCP service answers on `/` and `/mcp`.
- REST endpoints `POST /v1/search` and `POST /v1/fetch`, behind the same bearer, origin, host, and body checks as the MCP stream.
- `ownsearch sbom`, a CycloneDX 1.7 bill of materials built from `Cargo.lock`.
- Eight tools: search, fetch, instance info, instance health, scan, session, database query, and typed database changes.
- `ownsearch_fetch` follows same-host links and returns each page under `pages`.
- `ownsearch_fetch` extracts main-content Markdown with navigation, banners, comment sections, and footers pruned.
- `ownsearch_fetch` sanitizes HTML to an allowlist, with links limited to HTTP and HTTPS.
- `ownsearch_fetch` renders a JavaScript page in headless Chrome when the page is an empty shell or the call sets `render: always`.
- `ownsearch_fetch` reports `finalUrl` and the redirect chain.
- `ownsearch_fetch` extracts text from a PDF answer into Markdown paragraphs.
- A lowercase kebab-case host name, unique per instance, returned as `slug` in instance info, health, and scan status changes, and as `instanceSlug` in every search source.
- Discovery from searx.space, with an embedded 67-instance bootstrap snapshot.
- Instance lifecycle in SQLite: candidate, active, probation, missing, quarantined, and retired.
- A TLS probe that resolves the host, validates the chain and certificate dates, and records the protocol and grade.
- A `/config` probe that records engines, categories, locales, plugins, and format support.
- A JSON search probe, a policy-gated HTML search probe, and an exact-Google probe.
- Hard gates, weighted scoring, hysteresis, cooldown, and fleet selection with a size limit and a per-network limit.
- JSON and HTML SearXNG clients with a bounded reader and challenge detection.
- Result merge with URL normalization, deduplication, per-source attribution, preset shaping, and a byte budget.
- A hash-chained call ledger with verify and rotate.
- Typed AI database changes with a preview and a one-use confirmation token.
- CLI commands: serve, doctor, scan, instances, db, config, audit, sbom, completions, and man.
- A Dockerfile, an MCP bundle (MCPB) manifest, an MCP Registry manifest, and `docker-compose.yml`.
- A verified pre-migration backup.
- Checksummed schema versions.
- An ownership check on the database file.
- Two consecutive searches may start from different instances.
- A failed instance request is retried before the instance is skipped.
- Failover across the whole scored fleet.
- A cleanup pass with a 3 day keep window by default, adjustable from 1 hour to 90 days, which also covers the backups folder.
- A public-address guard on discovery records and page fetches, with a bounded DNS lookup.
- A bounded search queue: 8 searches run, 64 wait, each waits up to 30 seconds, and the rest answer `ownsearch.rate_limited`.
- RFC 9457 problem details on every REST refusal.
- Per-route HTTP counts with latency in `/metrics`.
- A process hang guard: with no progress for `OWNSEARCH_WATCHDOG_SECS`, the process exits with code 75 so a supervisor restarts it.
- A browser watchdog that closes the session this process launched after two missed answers.
- A browser session that also closes on idle time and at its maximum age.
- A per-process browser profile under the state directory, owner-only, removed when the session closes.
- Startup reaping of a browser profile whose owning process is gone, which kills only a Chrome whose command line names that profile.
- A `maintenance_runs` row when a maintenance sweep reports a problem.
- An explicit 404 problem body for an unknown path.
- A built-in scheduler that refreshes discovery at startup and on the discovery window, then probes due instances at the scan interval.
- A pin that keeps an instance in the fleet through score order, hysteresis, the network cap, and a Google requirement, while a failed hard gate still excludes it.
- Candidate and missing records that pass their hard gate join the fleet when pinned.
- The instance name from `/config` stored with each capability snapshot and returned by a capability query.
- `sources.fallbacks` in a search answer naming each instance whose result came from the HTML parser.

### Changed

- A cache entry stays fresh for 300 seconds by default.
- Typed database writes are enabled by default.
- Exhaustive mode covers 16 instances by default.
- A search deadline is 30 seconds by default.
- A search answer holds at most 25 results by default.
- The unscored fallback tries 32 instances by default.
- A search gives discovery five seconds inside its own deadline.
- The outer search ceiling is the deadline plus 20 seconds, so discovery and engine time fit.
- A request `timeout_secs` reaches the engine.
- A failed search never changes the probe counters.
- A rate-limited instance cools down and keeps its status.
- A scan never overwrites a status an operator changed while it ran.
- The events file is left alone when it is larger than 64 MiB.
- One client address may hold at most 16 connections.
- The no-fleet fallback skips an instance that failed a hard gate.
- Config text fields pass the same sanitizer and length cap as engines and categories.
- Maintenance sweeps run under a 120 second ceiling.
- The outbound client caps idle connections per host at eight and drops them after 30 seconds.
- Two processes on one state database take turns at maintenance through a lock file.
- An unknown path answers 404 instead of a plain-text 405.
- A wrong method answers 405 with a problem body instead of an empty one.
- **Breaking:** `statusChanges` entries report `previousStatus` and `nextStatus`; the `from` and `to` keys are removed.
- `GET /startupz` and `GET /readyz` answer a fixed reason when unhealthy and keep the database error in the debug log.
- The container image sets `OWNSEARCH_BROWSER=false`.
- A search skips an instance that is cooling down.

### Fixed

- A search falls back to scanned candidates that passed every hard gate when no instance is active.
- The expiry sweep deletes capability children before their parent, so the 90-day keep window runs.
- The TLS prober honors `OWNSEARCH_ALLOW_PRIVATE_INSTANCES`.
- The HTTP client refuses a connection that lands on a private address.
- The IPv6 guard rejects IPv4-compatible, site-local, local-use NAT64, benchmarking, ORCHID, RFC 9637 documentation, and SRv6 prefixes.
- An instance change and its event are stored together or not at all.
- The event, snapshot, and run queries honor `offset`.
- A stored status that does not parse is reported as corruption when an instance is read for display.
- A probe is stored completely or not at all.
- A scan rewrites the state snapshot after selecting the fleet.
- A writing scan runs one at a time within a process per state database.
- A writing scan waits `OWNSEARCH_SCAN_MIN_INTERVAL_SECS` between runs.
- A writing scan refuses a named instance that is not yet due unless the call sets `force`.
- A refused scan does not arm the minimum interval.
- Stored fleet snapshots are capped at 200 rows.
- A hostile result array is cut at 200 entries.
- The change-preview store is capped at 100.
- A busy database becomes a retryable rate limit.
- A constraint violation becomes a refused write.
- Database corruption is named as corruption.
- An HTTP 404 from an instance is not retried.
- The database query tool refuses a filter the selected entity cannot apply.
- The quality entity returns quality fields.
- Cache answers are invalidated when a scan or a typed change alters the fleet.
- A render in flight during shutdown closes its browser instead of returning it to the pool.
- A fetch render receives only the time left before the fetch deadline.
- A hung browser no longer blocks the next render.
- An abandoned browser profile is removed at startup, so cookies and cache from an earlier run are not reused.
- A cleanup or expiry sweep that exceeds 120 seconds is skipped and logged.
- Two discovery refreshes never run at the same time.
- A missing instance returns to probation only after a clean probe.
- A PDF answer reports truncation.
- A followed PDF page is parsed as a PDF instead of read as text.
- `ownsearch sbom` dependency references resolve to the declared component.
- A ledger append holds the file lock across catch-up and the write.

### Security

- PDF text is read with `lopdf`, so the tree no longer pulls the unmaintained `pdf-extract` dependency.
- Browser rendering checks every request and the peer address Chromium connects to against the public-address guard, and refuses a render whose final URL is not public.

[Unreleased]: https://github.com/devops-infinity/ownsearch/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/devops-infinity/ownsearch/releases/tag/v0.1.0

[0.1.1]: https://github.com/devops-infinity/ownsearch/compare/v0.1.0...v0.1.1

[0.1.2]: https://github.com/devops-infinity/ownsearch/compare/v0.1.1...v0.1.2

[0.2.0]: https://github.com/devops-infinity/ownsearch/compare/v0.1.2...v0.2.0
