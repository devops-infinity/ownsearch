# OwnSearch

Self-hosted MCP and HTTP search over a scored fleet of public SearXNG instances.

mcp-name: io.github.devops-infinity/ownsearch

Status: released, version 0.3.0.

OwnSearch discovers public SearXNG instances from searx.space, probes them at a low rate, scores them, keeps the fleet in a local SQLite database, rotates the starting instance, retries a failing one, and answers from the next healthy instance. Each result carries the sources that supplied it, with the instance and, when known, the engine. It runs as a Model Context Protocol (MCP) server over stdio or Streamable HTTP, and the HTTP port also answers two REST calls.

Owner: Md. Sazzad Hossain Sharkar.

Reference material: `docs/scanner-policy.md` for instance policy, `docs/operations.md` for the runbook, `RELEASING.md` for the release procedure, `SECURITY.md` for the reporting policy.

## Contents

- [What it does](#what-it-does)
- [What is enabled by default](#what-is-enabled-by-default)
- [What it does not do](#what-it-does-not-do)
- [Requirements](#requirements)
- [Build and install](#build-and-install)
- [Quick start](#quick-start)
- [Connect an MCP client](#connect-an-mcp-client)
- [Tools](#tools)
- [Search behavior](#search-behavior)
- [Scanner policy](#scanner-policy)
- [Quality model](#quality-model)
- [Configuration](#configuration)
- [HTTP mode](#http-mode)
- [Container](#container)
- [Local files and keep windows](#local-files-and-keep-windows)
- [Database](#database)
- [Database commands](#database-commands)
- [AI-assisted database changes](#ai-assisted-database-changes)
- [Development](#development)
- [Troubleshooting](#troubleshooting)
- [Privacy](#privacy)
- [Security](#security)
- [Support](#support)
- [Contributing](#contributing)
- [Distribution](#distribution)
- [License](#license)

## What it does

- Discovers public HTTPS SearXNG instances from `https://searx.space/data/instances.json`, and imports an embedded snapshot of 67 instances on the first run, before any network call.
- Probes each candidate once per scan: it resolves the host and validates the TLS chain and certificate dates, reads `/config`, and makes one JSON search. It makes an HTML search only when JSON is denied, and an exact-Google search only when the instance advertises that engine and JSON worked.
- Applies hard gates before scoring, so a certificate near expiry, an analytics tracker, or an unreadable `/config` keeps an instance out of the active fleet.
- Selects an active fleet with a size limit, a per-network limit, and hysteresis, so one bad scan does not remove a working instance.
- Rotates the starting instance on every search, makes up to three attempts per instance on a network error or `5xx`, cools an instance down when it keeps failing, and fails over to the next instance until the search succeeds or the deadline passes.
- Merges results from several instances, keeps every source observation, and marks partial coverage.
- Never retries a `429`, never runs a proof of work, and never tries to look like a human browser. With an operator-set `OWNSEARCH_CAPSOLVER_KEY`, a challenge that appears only in a rendered page can be solved by the CapSolver service for that one page.

## What is enabled by default

These capabilities are on by default. Each one is controlled by the setting named in [Configuration](#configuration).

- HTML search fallback: on. When an instance refuses JSON with a `403`, another `4xx` or `5xx`, or a non-JSON body, OwnSearch reads the ordinary HTML result page instead. A `401`, a `429`, and a challenge page are not retried through HTML; those follow the quarantine and cooldown rules.
- Round-robin selection: on. Each search starts with a different healthy instance.
- Failover: on. A failing instance gets its configured attempts, then it cools down and the search moves to the next instance.
- Every search engine the selected instance enables: on. OwnSearch sends no engine filter by default, so Google, Bing, DuckDuckGo, Brave, and every other engine the instance runs are available. Set `engines` on a call to narrow it.
- All SearXNG categories: on. OwnSearch sends no category filter by default.
- Exact-Google probing: on, whenever the instance advertises the `google` engine.
- Page extraction: on. `ownsearch_fetch` fetches a public page and returns it as Markdown, plain text, or cleaned HTML, with the title and a size cap.
- JavaScript rendering: on. When a page is an empty shell or the call sets `render: "always"`, a headless Chrome renders it, the browser stays open while it is used, and it closes on idle, on two missed health answers, or at its maximum age.
- Same-host crawling: on. `ownsearch_fetch` follows up to `max_pages - 1` links from the page you name, at most seven, and returns each one under `pages`. `same_host_only` defaults to true, so a call never wanders to another site unless you allow it.
- Result attribution, field presets, the response cache, session statistics, and typed database changes (with a preview and a one-use token) are all available without extra flags.

## What it does not do

- It does not host or operate a SearXNG instance.
- It does not call a paid search API.
- It does not make your queries anonymous. The operator of each public instance sees them.
- It does not run JavaScript on an ordinary search answer. The headless browser runs for the fetch tool on the page you name, and, as a last resort, for a search page or a probe from an instance that denies every HTTP format.
- It does not bypass a bot check. Many public instances answer automated clients with `429`, a challenge page, or a CAPTCHA. OwnSearch records that and moves on, unless `OWNSEARCH_CAPSOLVER_KEY` is set, which lets a challenge that appears only in a rendered page be solved by the CapSolver service.

## Requirements

- Rust 1.94.1 or newer to build. The pinned toolchain is in `rust-toolchain.toml` (1.99.1).
- A Linux or macOS machine. Windows builds are part of the release target list.
- Outbound HTTPS for discovery, probes, and searches.
- A C compiler for the bundled SQLite build: `build-essential` on Debian and Ubuntu, or the Command Line Tools on macOS.
- A Chrome or Chromium executable for JavaScript rendering. Set `OWNSEARCH_BROWSER_PATH` when auto-detection misses it.

## Build and install

### Install a release

Pick one channel. Each installs the same `ownsearch` binary.

- Homebrew, on Linux or macOS:

  ```sh
  brew trust devops-infinity/tap
  brew install devops-infinity/tap/ownsearch
  ```

- npm, which installs the platform binary through a small loader:

  ```sh
  npm install -g @devops-infinity/ownsearch
  ```

- Cargo:

  ```sh
  cargo install --locked ownsearch
  ```

- The shell installer from the latest release:

  ```sh
  curl --proto '=https' --tlsv1.2 -sSf https://github.com/devops-infinity/ownsearch/releases/latest/download/ownsearch-installer.sh | sh
  ```

Check the result:

```sh
ownsearch --version
```

Keep one channel only. Two installs of the same binary on your PATH make it unclear which one your MCP client starts.

### Update

```sh
ownsearch update --check     # report whether a newer release exists
ownsearch update             # download, verify, and install it
```

The updater fetches the latest release from GitHub, verifies `sha256.sum` against the minisign key built into the binary, checks the archive against the digest in that signed file, and replaces the running binary in place. It runs only when you call it. There is no background check and no automatic replacement.

`--check` works on every install and reports the latest version, plus the command that installs it. A full update refuses when another tool owns the install and names that command: `brew upgrade ownsearch` for Homebrew, `cargo install --locked ownsearch` for cargo, `npm install -g @devops-infinity/ownsearch` for npm, and pulling the newer image for a container. Only an install that came from the shell installer or a plain binary copy updates itself.

Set `GH_TOKEN` or `GITHUB_TOKEN` to raise the GitHub API limit from 60 requests an hour. `--json` prints the result as data for a script.

### Build from source

```sh
git clone https://github.com/devops-infinity/ownsearch
cd ownsearch
cargo build --release --locked
```

The binary lands at `target/release/ownsearch`. To install it into `~/.local/bin` after a full gate:

```sh
tools/reinstall.sh
```

## Quick start

The first command that reads the database creates it, applies the schema, and seeds an embedded fleet of 67 instances: `instances list`, `db counts`, `scan`, or `serve`. `doctor` and `db path` only report, they do not create.

```sh
ownsearch doctor              # resolved settings, state health, ledger state
ownsearch instances list      # the seeded fleet
ownsearch scan                # discover, probe, and score the fleet
ownsearch scan                # after OWNSEARCH_SCAN_MIN_INTERVAL_SECS (30 by default), a second pass promotes repeat successes to active
ownsearch instances list      # the fleet after scoring
ownsearch serve               # MCP over stdio, for a client to start
```

Search itself has no terminal command. It runs through an MCP client, through the two REST endpoints, or through the tools a client calls. The CLI covers setup, scanning, fleet changes, and database work.

The state lives in `${XDG_STATE_HOME}/ownsearch` by default, which is `~/.local/state/ownsearch` on most machines. Point every command and every MCP client at the same directory if you move it, with `OWNSEARCH_STATE_DIR` or `--state-dir`.

A dry run changes nothing and prints what would happen:

```sh
ownsearch scan --dry-run --limit 12
```

Searches use the active fleet. When no instance is active, OwnSearch falls back to scanned candidates that passed every hard gate and says so in `warnings`.

## Connect an MCP client

Every MCP client starts a server process. Point it at the installed binary with `serve` as the only argument.

Claude Code, available in every project:

```sh
claude mcp add --scope user ownsearch -- /home/you/.local/bin/ownsearch serve
```

Any client that reads a JSON config, such as Claude Desktop:

```json
{
  "mcpServers": {
    "ownsearch": {
      "command": "/home/you/.local/bin/ownsearch",
      "args": ["serve"]
    }
  }
}
```

Codex reads a TOML config:

```toml
[mcp_servers.ownsearch]
command = "/home/you/.local/bin/ownsearch"
args = ["serve"]
```

Use the absolute path to the binary. A desktop client usually does not load your shell profile, so a bare `ownsearch` may not resolve, and a Homebrew or npm install puts the binary somewhere else.

The server reads the same settings as the CLI. With nothing set, both use the default state directory, so one `ownsearch scan` in a terminal and the client see the same fleet. Set `OWNSEARCH_STATE_DIR` on both only when you moved the state.

Check the connection after adding it. The client should list eight tools: `ownsearch_search`, `ownsearch_fetch`, `ownsearch_instance_info`, `ownsearch_instance_health`, `ownsearch_scan`, `ownsearch_session`, `ownsearch_db_query`, and `ownsearch_db_crud`. Ask the client to call `ownsearch_session` to confirm it answers, then ask it to search.

No API key is needed to search. OwnSearch holds two secrets at most: an HTTP bearer token, used only when you serve Streamable HTTP, and an optional CapSolver key, used only when you turn on the paid captcha service. Neither is read from a flag or the profile; both come from the environment, and the bearer token can also live in the OS keychain.

Over Streamable HTTP, point the client at `http://127.0.0.1:8774/mcp`, or at the same host with no path. Both serve the MCP endpoint, and any other path answers 404 with a problem body. The port also answers `/healthz`, `/startupz`, `/readyz`, and `/metrics`. Send `Authorization: Bearer <token>` once `OWNSEARCH_BEARER_TOKENS` is set. The REST calls `POST /v1/search` and `POST /v1/fetch` take the tool arguments as the JSON body.

## Tools

- `ownsearch_search`: federated web search. Arguments: `query` (required), `categories`, `engines`, `language`, `page`, `time_range` (`day`, `week`, `month`, `year`), `safesearch` (`off`, `moderate`, `strict`), `limit`, `fields` (`minimal`, `standard`, `full`), `mode` (`fast`, `balanced`, `exhaustive`), `timeout_secs`, and `result_text` (`summary`, `full`). It may refresh local fleet state before answering.
- `ownsearch_fetch`: fetch a public page as `markdown` (default), `text`, or `html`, with `url` required and `max_chars` from 1000 to 200000. `max_pages` runs from 1 to 8 and defaults to 3; beyond 1 it follows links from the first page and returns them under `pages`, staying on the same host unless `same_host_only` is false. An HTTP redirect is followed up to three hops, each hop passes the same public-address check as the first request, and the answer reports `finalUrl` plus the `redirects` chain so a caller never has to guess where a link ended up. An `application/pdf` answer is parsed under an 8 MiB decompressed-text cap and shaped into Markdown paragraphs. `render` chooses the browser: `auto` (the default) renders only a page that looks like an empty JavaScript shell, `always` renders every call, and `never` stays on HTTP. Markdown is extracted from the main content: navigation, sidebars, cookie banners, share blocks, comment sections, and footers are pruned, the densest content root wins over link lists, and code fences carry their language. Private and loopback addresses are refused, and every page is capped and sanitized.
- `ownsearch_instance_info`: lifecycle and capability facts for one instance or a list, including its `slug`, status, score, rank, version, JSON and HTML access, engines, categories, and exact-Google evidence. Every result source and every scan status change carries the slug; a failed source names its instance id and url. The `/config` instance name comes from `ownsearch_db_query` with `entity: "capabilities"`.
- `ownsearch_instance_health`: hard gate result, certificate days left, TLS protocol and grade, median search time, cooldown, consecutive failures, and the most recent lifecycle events.
- `ownsearch_scan`: refresh discovery, probe instances, rescore, and reselect the fleet. It defaults to `dry_run: true`. Within one process, a writing scan runs one at a time per state database, waits `OWNSEARCH_SCAN_MIN_INTERVAL_SECS` between runs, and refuses a named instance that is not yet due unless the call sets `force`.
- `ownsearch_session`: local call counts, cache state, ledger length and head, and fleet size. No network call.
- `ownsearch_db_query`: bounded read-only queries over instances, quality, policy overrides, capability snapshots, events, snapshots, discovery runs, and hourly probe metrics. Each entity applies its own filters, and a filter that the selected entity cannot apply is refused instead of ignored. Events page in SQL up to a 50000 row offset, and a larger offset is refused rather than repeated. It never accepts SQL.
- `ownsearch_db_crud`: one typed lifecycle or policy change. It needs write mode, a reason, and a one-use confirmation token from a dry-run preview. It cannot delete an instance row.

Every tool returns `structuredContent` plus a one-line text summary. Every input schema sets `additionalProperties: false`. Every field a preset may drop is optional in the published output schema.

## Search behavior

Modes control how many instances answer one query.

- `fast` targets one instance.
- `balanced`, the default, targets three.
- `exhaustive` targets `OWNSEARCH_EXHAUSTIVE_MAX`.

Failover works the same way in every mode. The selected instances answer in parallel. A network error, a timeout, or a `5xx` is retried up to `OWNSEARCH_SEARCH_ATTEMPTS` times per instance. A `4xx` other than `408` and `429`, a `403`, or a challenge page is never retried. When an instance still fails, OwnSearch sets its `cooldown_until`, records a `search_failed` event, and tries the next instance in score order until it reaches its target or the search deadline (`OWNSEARCH_SEARCH_DEADLINE_SECS`). A failed search changes neither the status nor the probe counters. A scan probe or an operator command moves an instance between active, probation, and quarantined, and only a probe resets the success and failure counts. Failover keeps walking the scored fleet in order until it reaches its target or the deadline, so an answer low in the list is still found. A bounded queue sits in front: `OWNSEARCH_SEARCH_MAX_CONCURRENT` searches run at once, up to `OWNSEARCH_SEARCH_QUEUE_LIMIT` wait for a slot for `OWNSEARCH_SEARCH_QUEUE_WAIT_SECS`, and the rest answer `ownsearch.rate_limited` without stacking work, which is what keeps memory and CPU bounded on a small machine. An answer with zero results is a success, not a failure; only a page whose results carry no usable title and URL counts as a parser failure.

Round-robin rotates the starting point inside the top healthy window, so two searches in a row do not always begin with the same instance. Turn it off with `OWNSEARCH_ROUND_ROBIN=false` when you prefer a strict score order.

Before any scan has scored the fleet, or when every scored instance is cooling, a search falls back to the highest ranked discovered instances that have not failed a hard gate, rather than failing with `ownsearch.no_fleet`, and the response says so in `warnings`. The response never invents a result: when every attempt is refused or rate limited, `sources.failed` names each instance, its code, and what it answered. `sources.fallbacks` names each instance whose answer came from the HTML parser instead of JSON.

Field presets control payload size. `minimal` keeps title, URL, rank, score, and source id. `standard` adds the snippet, engines, published date, and the full source list. `full` keeps everything.

## Scanner policy

- A built-in scheduler refreshes discovery at startup and on each discovery window (`OWNSEARCH_DISCOVERY_REFRESH_SECS`), with a document capped at 500 records. It then probes due instances at the scan interval, one scan at a time, after a jitter delay.
- A pinned instance stays in the selection even when its score would drop it. A pin overrides the score order, the hysteresis, the per-network cap, and a Google requirement, and it lets a candidate or missing record enter the fleet. A failed hard gate still excludes it.
- A discovery record whose host resolves to a loopback, private, link-local, documentation, or reserved address is skipped, and an explicit scan of a denied, disabled, retired, or cooling instance is refused.
- One probe per kind per instance per run.
- Probes run four at a time by default, with jitter. `OWNSEARCH_SCAN_CONCURRENCY` changes the count for any run, scheduled or on demand.
- A `429` starts a cooldown and keeps the instance's status. A `401` or a challenge page quarantines it at once. A `403` counts as a failed scan: it reschedules the scan and moves the instance to probation or quarantine once the failure count passes its threshold. Repeated failures also lower its score.
- The user agent names the product: `OwnSearch (MCP fleet scanner; +https://github.com/devops-infinity/ownsearch) v<version>`.
- No proxy rotation and no header spoofing. A browser touches an instance page only as a last-resort capability check after every HTTP format is denied, and never to get past a detected challenge.

The full policy is in `docs/scanner-policy.md`.

## Quality model

Hard gates, in order. A failure keeps the instance out of the active fleet.

- HTTPS only.
- A valid certificate chain with a matching hostname.
- At least `OWNSEARCH_CERT_MIN_DAYS` days left on the certificate.
- No analytics tracker.
- A parseable `/config`.
- Exact Google present, but only when `OWNSEARCH_GOOGLE_REQUIRED=true`.

Score out of 100:

- Monthly uptime, 20 points.
- Yearly uptime, 10 points.
- Search success rate, 20 points.
- Median search time, 15 points.
- Initial response success and time, 10 points.
- Exact Google success and timing, 10 points.
- TLS grade, protocol, and certificate lifetime, 10 points.
- SearXNG version recency, 5 points.
- Bounded modifiers, up to 5 points, for engine coverage, category coverage, repeated successes, repeated failures, and parser drift.

Fleet selection sorts by score, takes at most `OWNSEARCH_FLEET_MAX` instances, allows at most `OWNSEARCH_MAX_PER_NETWORK` instances from one announced network prefix, excludes Tor records, and excludes an instance without exact Google support when `OWNSEARCH_GOOGLE_REQUIRED` is set, unless the instance is pinned. Hysteresis needs `OWNSEARCH_PROMOTE_SUCCESSES` good scans to promote, `OWNSEARCH_PROBATION_FAILURES` failed scans to demote, and `OWNSEARCH_QUARANTINE_FAILURES` failed scans to quarantine. A recovered missing instance returns through probation.

## Configuration

Settings resolve in this order: CLI flags, `OWNSEARCH_*` environment variables, a TOML profile at `${XDG_CONFIG_HOME}/ownsearch/profile.toml`, then built-in defaults.

Most environment variables have a matching profile key: drop the `OWNSEARCH_` prefix and lowercase the rest. For example `OWNSEARCH_FLEET_MAX` is `fleet_max`, and `OWNSEARCH_STATE_DIR` is `state_dir`. Three keys do not follow the pattern: `OWNSEARCH_BROWSER_PATH` is `browser_executable`, and `OWNSEARCH_BEARER_TOKENS` and `OWNSEARCH_PROFILE` have no profile key. Two keys exist only in the profile: `cache_max_bytes` and `preset`. The secrets `OWNSEARCH_BEARER_TOKENS`, `OWNSEARCH_CAPSOLVER_KEY`, and `OWNSEARCH_PROXY` have no profile key: a profile that names one is refused when it loads.

### Paths and transports

- `OWNSEARCH_PROFILE`: path to the TOML profile.
- `OWNSEARCH_STATE_DIR`: directory for `instances.db`, the snapshot, events, and backups. Default `${XDG_STATE_HOME}/ownsearch`.
- `OWNSEARCH_LEDGER_DIR`: directory for `calls.jsonl`, its archives, and `args.key`. Default `${XDG_DATA_HOME}/ownsearch/ledger`.
- `OWNSEARCH_HTTP`: serve Streamable HTTP instead of stdio. Default false.
- `OWNSEARCH_BIND`: HTTP bind address. Default `127.0.0.1:8774`.
- `OWNSEARCH_BEARER_TOKENS`: comma-separated bearer tokens, each at least 32 characters.
- `OWNSEARCH_ALLOWED_HOSTS`: comma-separated `Host` values for HTTP mode.
- `OWNSEARCH_ALLOWED_ORIGINS`: comma-separated `Origin` values for HTTP mode.
- `OWNSEARCH_LOG_FORMAT`: `text` or `json`. Logs go to stderr only.
- `OWNSEARCH_ALLOW_PRIVATE_INSTANCES`: accept loopback and private addresses for discovery records, probes, search, and fetch. Default false, and it exists for local testing only. Leave it off in any deployment that can reach the internet, because with it on a hostile discovery document can point the scanner at a local service.

### Discovery and scanning

- `OWNSEARCH_DISCOVERY_URL`: discovery source. Default `https://searx.space/data/instances.json`.
- `OWNSEARCH_DISCOVERY_REFRESH_SECS`: refresh window. Default 21600.
- `OWNSEARCH_OFFLINE`: skip discovery and use local state only. Default false.
- `OWNSEARCH_SCAN_CONCURRENCY`: parallel probes, 1 to 8. Default 4.
- `OWNSEARCH_SCAN_JITTER_MS`: random delay between probes, up to this many milliseconds. Default 250.
- `OWNSEARCH_PROBE_TIMEOUT_SECS`: per-request timeout for each probe step. Default 15; one probe stops after five times this value.
- `OWNSEARCH_SCAN_DEADLINE_SECS`: whole-scan deadline. Default 600, from 30 to 7200. The scan stops with a warning when it passes.
- `OWNSEARCH_SCAN_MIN_INTERVAL_SECS`: minimum gap between two writing scans on the same state database. Default 30, from 0 to 3600, and 0 turns the gap off. One scan at a time within a process always applies.
- `OWNSEARCH_FETCH_DEADLINE_SECS`: whole-fetch deadline, including followed links and a browser render. Default 60, from 5 to 600.
- `OWNSEARCH_BROWSER`: render JavaScript pages in headless Chrome. Default true.
- `OWNSEARCH_BROWSER_PATH`: Chrome or Chromium executable. Empty uses auto-detection.
- `OWNSEARCH_BROWSER_IDLE_SECS`: how long an idle browser stays open. Default 120, from 30 to 3600.
- `OWNSEARCH_BROWSER_NO_SANDBOX`: pass `--no-sandbox`, required inside a container that cannot set the sandbox. Default false.
- `OWNSEARCH_BROWSER_WATCHDOG_SECS`: how often the browser this process launched is checked for an answer. Default 30, from 5 to 600. Two missed answers close it.
- `OWNSEARCH_BROWSER_MAX_LIFETIME_SECS`: how long one browser session may live before it is replaced. Default 1800, from 60 to 86400.
- `OWNSEARCH_REQUEST_TIMEOUT_SECS`: HTTP client timeout. Default 30.
- `OWNSEARCH_CONNECT_TIMEOUT_SECS`: connect timeout. Default 5.
- `OWNSEARCH_MAX_UPSTREAM_BYTES`: response body cap. Default 4194304.
- `OWNSEARCH_COOLDOWN_SECS`: cooldown after a rate limit, a challenge, or a search failure. Default 3600.
- `OWNSEARCH_PROMOTE_SUCCESSES`, `OWNSEARCH_PROBATION_FAILURES`, `OWNSEARCH_QUARANTINE_FAILURES`: hysteresis. Defaults 2, 2, 4.
- `OWNSEARCH_FLEET_MIN`, `OWNSEARCH_FLEET_MAX`: fleet size, defaults 6 and 12.
- `OWNSEARCH_MAX_PER_NETWORK`: instances per announced network prefix, default 2.
- `OWNSEARCH_CERT_MIN_DAYS`: certificate lifetime gate, default 14.
- `OWNSEARCH_HTML_FALLBACK`: HTML probe and fallback. Default true.
- `OWNSEARCH_GOOGLE_REQUIRED`: exact Google as a hard gate. Default false.

### Search

- `OWNSEARCH_SEARCH_DEADLINE_SECS`: whole-search deadline, default 30.
- `OWNSEARCH_EXHAUSTIVE_MAX`: instances in `exhaustive` mode, default 16.
- `OWNSEARCH_SEARCH_ATTEMPTS`: attempts per instance, 1 to 5. Default 3.
- `OWNSEARCH_SEARCH_FAILOVER`: fail over to the next instance. Default true.
- `OWNSEARCH_ROUND_ROBIN`: rotate the starting instance. Default true.
- `OWNSEARCH_RESULT_CAP`: default result limit and the ceiling on a call's `limit`. Default 25, from 1 to 100.
- `OWNSEARCH_SEARCH_MAX_CONCURRENT`: searches that run at once, default 8.
- `OWNSEARCH_SEARCH_QUEUE_LIMIT`: searches allowed to wait for a slot, default 64.
- `OWNSEARCH_SEARCH_QUEUE_WAIT_SECS`: how long a queued search waits before it answers `ownsearch.rate_limited`, default 30.
- `OWNSEARCH_CACHE_TTL_SECS`: how long a cached answer stays fresh. Default 300, and 0 turns the cache off.
- Profile key `cache_max_bytes`: cache byte cap. Default 16 MiB.
- Profile key `preset`: default field preset, `minimal`, `standard`, or `full`. Default `standard`.

### Keep windows and maintenance

- `OWNSEARCH_KEEP_WINDOW`: how long to keep ledger archives, event lines, and pre-migration backups. Accepts `90s`, `15m`, `2h`, `3d`, or plain seconds. Default `3d`. Minimum 1 hour, maximum 90 days, and a negative or malformed value is refused at startup.
- `OWNSEARCH_MAINTENANCE_INTERVAL_SECS`: how often the running server runs the cleaner. Default 3600, minimum 60, maximum 86400.
- `OWNSEARCH_WATCHDOG_SECS`: how long the process may go without progress before it exits with code 75 so a supervisor restarts it. Default 120, from 0 to 3600, and 0 turns it off.

The cleaner runs at startup, after every applied scan, on that interval while a server runs, and inside `ownsearch db maintenance`. It rotates the live ledger when its oldest row passes the keep age, deletes ledger archives and `pre-migration-*.db` files older than the window, and trims old lines from `instances.events.jsonl`. The trim holds a lock file, so an event written at the same moment is never lost. The same interval also expires old rows in SQLite: probes, TLS observations, capability snapshots, metrics, snapshots, events, and discovery runs each keep their own window, and `ownsearch db maintenance` reports what it removed. Two processes on one state database take turns through a lock file, so a sweep never runs twice. If a file cannot be removed, the run logs a warning that names `ownsearch db maintenance` as the manual repair, and a sweep that reports a problem writes an `attention` row to `maintenance_runs`.

### AI database changes

- `OWNSEARCH_AI_DB_MODE`: `read-only` or `read-write`. Default `read-write`, because every change still needs a preview and a one-use token; set `read-only` to refuse all typed database changes.
- `OWNSEARCH_AI_DB_MAX_CHANGES_PER_CALL`: field changes per call, default 5.
- `OWNSEARCH_AI_DB_CONFIRM_TTL_SECS`: confirmation lifetime, default 120.
- `OWNSEARCH_AI_DB_MIN_INTERVAL_SECS`: minimum gap between applies, default 2.

### Profile example

```toml
version = 1
fleet_min = 6
fleet_max = 12
html_fallback = true
google_required = false
result_cap = 10
search_attempts = 3
round_robin = true
keep_window = "3d"
ai_db_mode = "read-only"
```

## HTTP mode

```sh
OWNSEARCH_BEARER_TOKENS="$(openssl rand -hex 24)" ownsearch serve --http
```

Any random value of at least 32 characters works; `openssl rand -hex 24` is one way to make one.

- The default bind is `127.0.0.1:8774`.
- A non-loopback bind without a bearer token is refused at startup.
- Tokens need at least 32 characters and are compared in constant time.
- `Host` is checked against a loopback list or `OWNSEARCH_ALLOWED_HOSTS`.
- An `Origin` header is rejected unless it matches `OWNSEARCH_ALLOWED_ORIGINS`.
- Request bodies are capped at 1 MiB, headers must arrive within 10 seconds, and concurrent requests are capped.
- `GET /healthz` is liveness. `GET /startupz` runs a quick database check on every request and answers 503 while the check fails. `GET /readyz` runs a cheap liveness read on every request and fails during shutdown, so a monitor never waits on a whole-database scan. `GET /metrics` reports call counters, per-tool counts, and per-route HTTP counts with a latency sum and maximum.
- Middleware runs outermost first: body deadline, request id, origin check, host check, bearer check, concurrency gate, body limit, trace layer, metrics, route. Security headers wrap the whole router. The trace layer logs method, path, status, and latency.
- Shutdown flips readiness first, then drains in-flight calls under a deadline.
- Under a supervisor, a hung process is replaced. `OWNSEARCH_WATCHDOG_SECS` bounds how long it may stall before it exits with code 75, and `docker-compose.yml` sets `restart: unless-stopped` with a 30 second stop grace period.

### REST endpoints

The same engine answers two JSON endpoints on the HTTP port, behind the same bearer check, origin check, host check, and body limit as the MCP stream.

- `POST /v1/search` takes the fields of `ownsearch_search`, with `query` required.
- `POST /v1/fetch` takes the fields of `ownsearch_fetch`, with `url` required.

```sh
curl -s http://127.0.0.1:8774/v1/search \
  -H 'content-type: application/json' \
  -d '{"query":"rust programming language","limit":5}'
```

A successful call answers `200` with the structured content the MCP tool returns. A refused call answers `400` for bad arguments, `401` for a missing or wrong token, `403` for a policy refusal, `404` for a missing route or resource, `405` for a wrong method, `409` for an expired confirmation, `429` for rate limiting, `502` for an upstream failure, `503` for no fleet or a shutdown, and `504` for a timeout, with an RFC 9457 `application/problem+json` body that carries `type`, `title`, `status`, `detail`, `instance`, and the stable `code` and `remedy` as extension members, and no internal detail. Every REST call is recorded in the ledger and counted in `/metrics` exactly like an MCP call.

Put Caddy in front for any remote use. Never expose an anonymous scanner.

Store the token list in the OS keychain instead of the environment:

```sh
printf '%s' "$TOKEN" | ownsearch config set-bearer-token
```

## Container

```sh
docker build -t ownsearch .
docker run --rm -p 127.0.0.1:8774:8774 \
  -e OWNSEARCH_BEARER_TOKENS="$(openssl rand -hex 24)" \
  -v ownsearch-state:/home/ownsearch/.local/state/ownsearch \
  ownsearch
```

`docker-compose.yml` holds the same setup with `restart: unless-stopped`, both state volumes, and a healthcheck. The image sets `OWNSEARCH_BIND=0.0.0.0:8774`, a ledger directory under `/home/ownsearch/.local/share/ownsearch/ledger`, and `OWNSEARCH_BROWSER=false`, so a container renders no page. `OWNSEARCH_BEARER_TOKENS` is required at run time and the server refuses to start without it. Add `OWNSEARCH_ALLOWED_HOSTS` with the names clients use, or the Host check refuses every request that does not name `localhost` or the bind address. The state volume keeps the database and snapshot across restarts; mount the ledger directory too when the ledger and `args.key` must survive a restart.

## Local files and keep windows

- Database: `${XDG_STATE_HOME}/ownsearch/instances.db`, or `OWNSEARCH_STATE_DIR/instances.db`.
- Snapshot: `instances.snapshot.json` beside it, regenerated after each discovery run, each scan, each AI change, and the bootstrap import.
- Discovery history: `instances.events.jsonl`, trimmed to the keep window.
- Backups you create: `backups/instances-<stamp>.db`, written with `VACUUM INTO`, verified with a quick check and a foreign key check, and removed after the keep window like every other temporary file.
- Pre-migration backups: `pre-migration-<stamp>.db`, removed after the keep window.
- Browser profiles: `browser/profile-<pid>` under the state directory, owner-only, removed when the session closes, and reaped at startup when the process that owned one is gone.
- Ledger: `calls.jsonl`, archived to `calls-<stamp>.jsonl` at the keep boundary, and archives older than the window are removed.
- Arguments key: `args.key` beside the ledger. It keys the argument hashes and the ledger chain, so it is kept.

The state directory is created with mode 0700 and files with mode 0600. WAL mode adds `instances.db-wal` and `instances.db-shm`. Database rows have their own windows, applied by `ownsearch db maintenance`: probes 90 days, TLS observations 90 days, capability snapshots 90 days, hourly metrics 180 days, fleet snapshots 30 days, discovery runs 180 days, lifecycle events 365 days. Instance rows are never deleted by the scanner.

## Database

The database is SQLite in WAL mode with foreign keys on, STRICT tables, and a checksummed migration ledger. The schema is built from embedded migrations that run forward at open, in order, from `0001_initial.sql` to the newest file in `crates/ownsearch-core/src/db/migrations/`. A pending migration takes a verified `VACUUM INTO` backup first. A database written by a newer release is refused with `ownsearch.state_newer` rather than read partially, and the remedy is to run that newer release. This page does not repeat the column-level schema.

Tables: `schema_migrations`, `application_metadata`, `discovery_sources`, `discovery_runs`, `instances`, `capability_snapshots`, `engines`, `categories`, `capability_engines`, `capability_categories`, `probes`, `tls_observations`, `hourly_probe_metrics`, `instance_quality`, `instance_events`, `fleet_snapshots`, `instance_policy_overrides`, and `maintenance_runs`.

`ownsearch db verify` checks the file's integrity and foreign keys and prints the schema version. The store also checks each migration checksum when it opens. A backup is written with `VACUUM INTO` and verified before it is used. `ownsearch db restore --from <backup>` stages and verifies the backup, moves the live database and its WAL sidecars aside, installs the backup, and rechecks it, so a corrupt store has a working recovery command.

## Database commands

- `ownsearch db path`
- `ownsearch db verify`
- `ownsearch db backup [--out PATH]`
- `ownsearch db restore --from PATH`
- `ownsearch db export [--out PATH]`
- `ownsearch db maintenance` (WAL checkpoint, vacuum, data expiry, and the cleanup pass)
- `ownsearch db counts [--json]`
- `ownsearch instances list [--status S] [--limit N] [--json]`
- `ownsearch instances show [ID] [--url URL] [--json]`
- `ownsearch instances add URL [--pin] [--json]`
- `ownsearch instances pin ID`, `unpin ID`, `deny ID [--reason TEXT]`, `allow ID [--reason TEXT]`, `reset-status ID`
- `ownsearch instances retire ID [--reason TEXT]`, `restore ID`, `set-type ID normal|tor|unknown`
- `ownsearch instances events [ID] [--limit N] [--json]`
- `ownsearch config show [--json]`, `set-bearer-token`, `delete-bearer-token`
- `ownsearch audit verify [--file PATH]`, `rotate [--file PATH]`
- `ownsearch update [--check] [--json]`

`--profile`, `--state-dir`, and `--ledger-dir` work at any position, before or after a subcommand.

### Your own instance

`ownsearch instances add http://localhost:8888/ --pin` stores a SearXNG instance you run yourself, whether it listens on a localhost port, a Docker network address, or any other private address. The instance starts as a candidate. A scan probes it like any other instance, and two passing probes promote it to active, after which searches use it. A loopback or private host skips the TLS and certificate gate, because those checks describe a public host. The address rule still applies: set `OWNSEARCH_ALLOW_PRIVATE_INSTANCES=true`, and keep the instance on a network you control, since the same setting lets a hostile discovery document point the scanner at a local service.

### Instance CRUD examples

Create an instance:

```bash
ownsearch instances add http://127.0.0.1:8080/ --pin
```

Read it back:

```bash
ownsearch instances list --limit 10
ownsearch instances show 1
ownsearch instances show --url http://127.0.0.1:8080/
```

Update its policy:

```bash
ownsearch instances pin 1
ownsearch instances unpin 1
ownsearch instances deny 1 --reason "host is gone"
ownsearch instances reset-status 1
```

Delete it from the fleet, which retires rather than removes the row:

```bash
ownsearch instances retire 1 --reason "replaced by another host"
```

Bring it back:

```bash
ownsearch instances restore 1
```

Apply the change to the fleet with a scan:

```bash
ownsearch scan --instance 1 --force
```

An instance is never deleted, so its probe history and events stay readable. `retire` is the delete step of the lifecycle. `restore` returns a retired, missing, unlisted, quarantined, or denied instance to candidate, clears the denial, and re-enables scanning. `allow` clears a denial without changing the status. `reset-status` clears every override and resets the row to candidate. `deny` quarantines the row, stops the scanner, and says how to undo itself. A Tor row sits in `excluded` and is never probed or served; set its type to normal first if you believe it is reachable. The same operations exist over MCP through `ownsearch_db_crud`, with the preview and one-use token flow.

## AI-assisted database changes

Write mode is on by default. Every change still needs a preview, then a separate apply call that carries the one-use token, and `OWNSEARCH_AI_DB_MODE=read-only` refuses all of them.

1. The client calls `ownsearch_db_crud` with `dry_run: true`. OwnSearch validates the entity, the field allowlist, the value types, and the lifecycle transition, then returns a before-and-after preview and a one-use token with a lifetime. The minimum interval between applies is enforced when the token is used.
2. The client calls again with `dry_run: false` and the token. OwnSearch applies exactly the previewed change in one transaction, appends a lifecycle event, regenerates the snapshot, and returns the committed record.
3. To add an instance, call the same tool with `operation: create`, `entity: instance`, and `url` set to the instance address. The preview and token flow is identical, and the instance is stored exactly as `ownsearch instances add` stores it.
3. An expired token, a changed preview, a replayed token, a raw SQL field, an unknown field, or read-only mode all return a stable error with a remedy, and every refusal is written to the event history.

The tool can create or remove a policy override, update lifecycle and policy fields, retire an instance, or restore a retired or missing instance. It cannot delete an instance row, rewrite probes, or forge events. Setting `status` to `active` requires a passing hard gate.

## Development

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo test --workspace --all-features --locked --doc
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --locked
cargo llvm-cov --workspace --all-features --locked --fail-under-lines 80
cargo run -p ownsearch -- sbom
tools/verify.sh
tools/smoke.sh http://127.0.0.1:8774
```

The live test is ignored by default because it needs the public network:

```sh
cargo test -p ownsearch-core --test live -- --ignored --nocapture
```

## Troubleshooting

- `ownsearch.no_fleet`: run `ownsearch scan`, then check `ownsearch instances list`.
- Every search returns no results and `sources.failed` says `ownsearch.policy_refused`: the selected instances deny JSON and HTML. Run from a network the instances accept, or check `ownsearch instances list` for quarantined records.
- `ownsearch.state_corrupted`: run `ownsearch db verify` and restore the newest backup.
- `ownsearch.state_newer`: the state database was written by a newer release. Install that release, or point `OWNSEARCH_STATE_DIR` at another directory.
- The MCP client lists no tools or marks the server as failed: use the absolute path to the binary, then run the same command in a terminal to see its error. A server that cannot open its state database exits with the reason on standard error.
- `ownsearch audit verify` says the ledger cannot be read: on a fresh install the file appears after the first tool call. Run a search once and verify again.
- `ownsearch.write_disabled`: set `ai_db_mode = "read-write"`.
- `ownsearch.confirmation_expired`: call the tool again with `dry_run: true`.
- Disk filling up: lower `OWNSEARCH_KEEP_WINDOW` for the ledger archives, event lines, and pre-migration backups, then run `ownsearch db maintenance` and read the warnings it prints. SQLite rows follow the fixed per-table windows listed under Local files and keep windows.
- A startup error names the setting and its bounds. Fix the value, or remove it to take the default.
- The process exits with code 75: it stopped making progress for `OWNSEARCH_WATCHDOG_SECS`. Check the logs for the last request, and let the supervisor start it again.

## Privacy

Queries go to the public instances you selected and to the engines behind them. OwnSearch stores no raw query text. The ledger records a keyed hash of the arguments, the tool name, the status, the upstream HTTP status class, timing, byte counts, parser, and instance ids. There is no telemetry.

One more destination exists, and only when you set it: with `OWNSEARCH_CAPSOLVER_KEY` set, a detected captcha sends the page URL and the site key to `https://api.capsolver.com` to solve the challenge. Nothing else leaves the machine for that call, and the service is off without a key.

## Security

- No `unsafe` code anywhere in the workspace.
- Discovery records are limited to public addresses, so a hostile document cannot point the scanner at a local or internal service.
- rustls with strict certificate validation on every outbound connection.
- Bounded response reads, connection limits, and request deadlines.
- Search results are data. Every string passes a sanitizer that removes invisible and control characters, URLs must be HTTP or HTTPS, and long values are cut with a visible marker.
- No secret is accepted on a command line, written to the profile, or printed by any command.
- The browser renders under a per-process profile with owner-only permissions and is closed when it hangs, goes idle, or outlives its maximum age. Only a browser OwnSearch launched is ever closed.

Report a vulnerability privately; `SECURITY.md` states how.

## Support

Write to sazzad@devops.bd for a question, a bug report, or an install problem. Include the version, the command you ran, and the output. Security problems follow `SECURITY.md`, not this address's normal queue.

The runbook is `docs/operations.md`: service levels, detection, triage, self-healing behavior, and baselines.

## Contributing

The repository has a single maintainer. Send a patch or a proposal to sazzad@devops.bd before starting large work, and run `tools/verify.sh` before sending a change. A change lands when the full gate passes; `RELEASING.md` lists what the gate runs.

## Distribution

Each release carries `sha256.sum`, which covers every archive, the installers, the formula, and the MCPB bundle, and is signed with minisign. `ownsearch sbom` generates a CycloneDX 1.7 bill of materials on demand from `Cargo.lock`.

The release channels:

- GitHub release archives and checksums, signed with minisign.
- Shell and PowerShell installers.
- A Homebrew formula built from the release archives.
- The npm loader `@devops-infinity/ownsearch`.
- crates.io packages `ownsearch-core` and `ownsearch`.
- The MCP Registry entry `io.github.devops-infinity/ownsearch`.
- An MCP bundle (MCPB) for desktop clients.
- A `ghcr.io/devops-infinity/ownsearch` image.

An install from the shell installer or a plain binary copy updates itself with `ownsearch update`. Every other channel updates through its own command, and the updater refuses and names that command when it detects one.

A `v*` tag starts the release workflow, which builds the seven targets and then creates and publishes the GitHub release. The gate runs on the machine through `tools/verify.sh` before the tag exists, so the workflow repeats none of it. Every action in that workflow is pinned to a commit, so a moved tag cannot change what runs. `RELEASING.md` holds the ordered publish steps and the credentials each one needs.

## License

MIT OR Apache-2.0, at your choice. The full texts are in [`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE). The HTML parser (`scraper`) brings MPL-2.0 components (`cssparser`, `selectors`, `dtoa-short`), used as unmodified dependencies; MPL-2.0 permits that use inside a larger work under another license.
