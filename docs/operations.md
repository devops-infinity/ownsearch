# Operations

## Service levels

- Search success rate, excluding upstream refusals this service cannot control: target 99 percent of calls that reach at least one instance.
- Search latency: p95 under 35 seconds against the default 30 second deadline plus discovery allowance.
- Readiness: `/readyz` answers 200 whenever the state database answers a read, measured by the smoke script on a timer.
- Error budget: a month with more than 1 percent unplanned search failures blocks the next release until the cause is fixed and a smoke run passes.

## Detection

- `/metrics` reports calls, errors, in-flight requests, and per-tool counts. Watch the error rate and the search queue refusals.
- Logs go to stderr as JSON in the container and text on a terminal. Internal failures log at `error`; client-correctable refusals log at `warn`.
- `ownsearch session` reports the ledger head, cache state, and fleet size without a network call.

## Self-healing

- A supervisor restarts the process. `OWNSEARCH_WATCHDOG_SECS` (default 120) sets how long the process may go without making progress. Past that window it logs one error and exits with code 75, and `docker-compose.yml` restarts it with `restart: unless-stopped` and a 30 second stop grace period. Set it to 0 to turn the exit off.
- The browser is watched while it is open. Every `OWNSEARCH_BROWSER_WATCHDOG_SECS` the browser this process launched must answer a version call; two missed answers close it, and a new session starts on the next render. A session also closes itself after `OWNSEARCH_BROWSER_IDLE_SECS` idle and after `OWNSEARCH_BROWSER_MAX_LIFETIME_SECS` alive.
- Each process renders under its own profile at `OWNSEARCH_STATE_DIR/browser/profile-<pid>`, created with owner-only access and removed on close. Startup reaps a profile whose owning process is gone, and kills only a process whose command line names that profile and a Chrome binary, so a Chrome the user started is never touched.
- Maintenance runs are bounded and serialized. A sweep that does not finish in 120 seconds logs a warning and skips that round, and the loop keeps its next slot. The sweep holds `OWNSEARCH_STATE_DIR/maintenance.lock`, so two processes on one state database take turns instead of sweeping twice.

## Triage and remediation

1. Run `tools/smoke.sh http://127.0.0.1:8774`. A failure there is the first fact.
2. `ownsearch doctor` prints every resolved setting and its source.
3. `ownsearch instances list --status active` shows the fleet; `ownsearch db counts` shows row growth.
4. A corrupt store: `ownsearch db verify`, then restore the newest verified backup from the backups folder, then start again.
5. A bad release: reinstall the previous tag with `tools/reinstall.sh --dir`, or repin the container image to the previous tag, then run the smoke script.

## Baselines

- Search against a responding fleet: p50 near 1 second per instance, the deadline is 30 seconds.
- Idle process: under 100 MB resident with no browser running; a rendered page adds the browser process while it is in use, and the browser closes after the idle window.
- Database size: driven by the 500-row discovery cap and the keep windows; probes and TLS rows leave after 90 days, metrics after 180, events after 365.
- Queue under load: 8 searches run, 64 wait, the rest answer `ownsearch.rate_limited`.

## Maintenance

- Corrective: fix defects found by the gate, a smoke failure, or an incident.
- Adaptive: dependency and toolchain refresh, plus a new edition through `cargo fix --edition`, the edition bump, the full gate, and `cargo fmt`.
- Perfective: keep-window tuning and query-plan review at each release.
- Preventive: the advisory scan, the deny policy, and the smoke timer.

## Deployment

Deployment is package installation per channel plus the container image. Rollback is reinstalling the previous tag or repinning the image; the database needs no step because migrations are forward-only and the pre-migration backup is verified before it is used.

## Review triggers

Review this page when a new outbound fetcher or renderer, a new stored table, a new tool, or a new deployment channel is added.

## Local enforcement

The pre-commit hook scans staged files for secrets and the pre-push hook runs the full gate; `tools/install-hooks.sh` installs both. Merge-time enforcement is these local hooks plus the release gate in `RELEASING.md`. The one hosted workflow is `.github/workflows/release.yml`, which runs on a `v*` tag, builds the seven targets, and publishes the release; the whole gate (`tools/verify.sh` with shellcheck, shfmt, bats, formatting, check, clippy, tests, the MSRV check, doc tests, docs, audit, deny, the suppression dates, machete, coverage, cargo-semver-checks, and both gitleaks scans) runs on the machine. A penetration test before exposing the port publicly is recorded as not performed: the service is loopback by default, single operator, and holds no accounts. Reopen that decision the moment it is exposed beyond loopback or a second party receives a token.
