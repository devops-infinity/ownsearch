# Operations

How to watch a running OwnSearch service, recognize each failure it can show, and fix it. Installation and release steps live in [RELEASING.md](../RELEASING.md).

## Service levels

These are targets for this project, not measurements taken from a run.

- Search success: 99 percent of calls that reach at least one instance, excluding upstream refusals this service does not control.
- Search latency: p95 under 35 seconds. The search deadline defaults to 30 seconds and the REST timeout is 120 seconds.
- Readiness: `/readyz` answers 200 whenever the state database answers a read.
- Error budget: a month with more than 1 percent unplanned search failures blocks the next release until the cause is fixed and a smoke run passes.

## Detection signals

- `GET /healthz` answers 200 while the process is up. It reads nothing.
- `GET /startupz` runs a quick database check on every request and answers 503 while the check fails.
- `GET /readyz` runs a cheap read on every request and answers 503 during shutdown, so a monitor never waits on a whole-database scan.
- `GET /metrics` answers JSON with `calls`, `errors`, `inflight`, `perTool`, `httpCalls`, and `httpLatency`. Each `httpCalls` key is the method, the route, and the status, so a 429 row is readable directly. `httpLatency` holds `count`, `sumMs`, and `maxMs` for each method and route. Watch `errors` and the 429 rows.
- Logs go to stderr. `OWNSEARCH_LOG_FORMAT` selects `text` or `json`, and `RUST_LOG` sets the filter. Every HTTP request logs at `info` with method, path, status, and latency. A background failure logs at `warn`. A panic or a watchdog stall logs at `error`.
- A process that makes no progress for `OWNSEARCH_WATCHDOG_SECS` (default 120) logs one error and exits with code 75, so a supervisor can restart it.
- `ownsearch_session` reports the ledger head, cache state, and fleet size without a network call.

## Triage

Every entry names the symptom, the signal that identifies it, the check that confirms it, the remedy, and the check that proves the fix.

### The process exits with code 75

Signal: the log carries one error, and the supervisor restarts the process.

Check: read the last request in the log. A hang points at a blocked call, not at the fleet.

Remedy: let the supervisor restart it. `docker-compose.yml` runs the service with `restart: unless-stopped` and a 30 second stop grace period. Set `OWNSEARCH_WATCHDOG_SECS=0` only when an outside monitor already covers the process.

Verify: `/readyz` answers 200 after the restart.

### The browser does not answer

Signal: a render fails while ordinary fetches still work.

Check: the log names the missed watchdog answer or the missing Chromium.

Remedy: the watchdog closes a browser that misses two answers, and its session also ends on idle and at its maximum age, so the next render starts a fresh one. A missing executable needs `OWNSEARCH_BROWSER_PATH`. A container renders nothing by default, because the image sets `OWNSEARCH_BROWSER=false`.

Verify: one `ownsearch_fetch` call with `render: "always"` returns a page.

### A search answers `ownsearch.no_fleet`

Signal: every search fails with `ownsearch.no_fleet`.

Check: `ownsearch instances list` shows the fleet, and `ownsearch db counts` shows how many rows sit in each status.

Remedy: run `ownsearch scan`. A scan that cannot reach the network leaves the fleet empty until discovery answers again.

Verify: the next search returns results, or falls back to scanned candidates and says so in `warnings`.

### Searches are refused with `ownsearch.busy`

Signal: `/metrics` shows a rising `errors` count and 429 answers in `httpCalls`.

Check: `OWNSEARCH_SEARCH_MAX_CONCURRENT` searches run at once, up to `OWNSEARCH_SEARCH_QUEUE_LIMIT` wait for a slot, and each waits `OWNSEARCH_SEARCH_QUEUE_WAIT_SECS` before it answers `ownsearch.busy`.

Remedy: raise the queue limit or the wait when the machine has headroom, or lower the concurrency when it does not. The bound exists to keep memory and CPU flat on a small machine.

Verify: the 429 count in `httpCalls` stops growing under the same load.

### The state database is corrupted

Signal: a call answers `ownsearch.state_corrupted`, or the process refuses to start.

Check: `ownsearch db verify` reports the schema version, the integrity check, and the foreign keys.

Remedy: stop the server, then run `ownsearch db restore --from OWNSEARCH_STATE_DIR/backups/<newest>.db`. The command stages and verifies the backup, moves the live database and its WAL sidecars aside, installs the backup, and rechecks it.

Verify: `ownsearch db verify` passes and `/readyz` answers 200.

### The state database comes from a newer release

Signal: a call answers `ownsearch.state_newer`.

Check: `ownsearch --version` against the release that wrote the database.

Remedy: install that release. The alternative is to point `OWNSEARCH_STATE_DIR` at another directory, which starts a separate state.

Verify: the starting release answers `ownsearch doctor`.

### The ledger chain does not verify

Signal: `ownsearch audit verify` reports a break.

Check: the message names the row and the row's recorded hash.

Remedy: a lost `args.key` or a tampered row means the chain cannot be rebuilt. `ownsearch audit rotate --force` archives the chain and starts a fresh one on the next call.

Verify: `ownsearch audit verify` exits 0.

### A maintenance sweep logs a warning

Signal: the log carries a warning from the cleaner, and a sweep that reports a problem writes an `attention` row to `maintenance_runs`.

Check: the warning names the file it could not remove.

Remedy: run `ownsearch db maintenance` by hand and read its output. Lower `OWNSEARCH_KEEP_WINDOW` when disk is the cause.

Verify: the next sweep logs no warning.

### The process refuses to start

Signal: the process exits, and standard error names the setting or the guard that stopped it.

Check: a value outside its bounds, a non-loopback bind with no bearer token, or an unparseable profile all stop startup.

Remedy: fix the value, or remove it to take the default. `OWNSEARCH_BEARER_TOKENS` is required for any bind that is not loopback. `OWNSEARCH_ALLOWED_HOSTS` is required for a container behind a proxy that forwards a name.

Verify: `ownsearch doctor` prints every resolved setting and exits 0.

### A release is bad

Signal: `tools/smoke.sh` fails against the deployed build.

Check: the smoke run prints the endpoint and the status it received.

Remedy: `git switch --detach vX.Y.Z` for the previous tag, run `tools/reinstall.sh`, then `git switch -`. For a container, repin the image to the previous tag. The database needs no rollback step, because migrations run forward only and a pending migration takes a verified backup first.

Verify: the smoke run passes against the previous tag.

## Baselines and bounds

- Search: the search deadline is 30 seconds, and an instance that fails cools down for `OWNSEARCH_COOLDOWN_SECS` (default 3600).
- Queue: `OWNSEARCH_SEARCH_MAX_CONCURRENT` searches run, `OWNSEARCH_SEARCH_QUEUE_LIMIT` wait, and the rest answer `ownsearch.busy`.
- HTTP: 256 connections in total, 16 per client address, 128 concurrent requests, a 1 MiB body cap, a 30 second body deadline, and a 120 second REST timeout.
- Auth: 5 failed bearer checks in 60 seconds lock one address out for 30 seconds, and the refusal carries `Retry-After`.
- Shutdown: readiness flips to 503 first, then in-flight calls drain inside a 10 second grace.
- Database size: each table keeps its own fixed window, listed under "Data kept" in [docs/scanner-policy.md](scanner-policy.md). `OWNSEARCH_KEEP_WINDOW` covers ledger archives, event lines, and pre-migration backups only.

## Scheduled maintenance

- The cleaner runs at startup, after every applied scan, on `OWNSEARCH_MAINTENANCE_INTERVAL_SECS` (default 3600) while a server runs, and inside `ownsearch db maintenance`.
- One sweep is bounded at 120 seconds. A sweep that does not finish logs a warning and skips that round, and the loop keeps its next slot.
- The sweep holds `OWNSEARCH_STATE_DIR/maintenance.lock`. A second process waits 2 seconds for the lock, then skips the round with a warning.

## Deployment and rollback

Deployment is package installation per channel plus the container image. Rollback is reinstalling the previous tag or repinning the image. [RELEASING.md](../RELEASING.md) holds the ordered publish steps and the rollback commands.

## Review triggers

Review this page when a new outbound fetcher or renderer, a new stored table, a new tool, or a new deployment channel is added.
