# Configuration

Every setting OwnSearch reads, with its default and its bounds.

Settings resolve in this order: CLI flags, `OWNSEARCH_*` environment variables, a TOML profile at `${XDG_CONFIG_HOME}/ownsearch/profile.toml`, then built-in defaults.

Most environment variables have a matching profile key: drop the `OWNSEARCH_` prefix and lowercase the rest. For example `OWNSEARCH_FLEET_MAX` is `fleet_max`, and `OWNSEARCH_STATE_DIR` is `state_dir`. Three keys do not follow the pattern: `OWNSEARCH_BROWSER_PATH` is `browser_executable`, and `OWNSEARCH_BEARER_TOKENS` and `OWNSEARCH_PROFILE` have no profile key. Two keys exist only in the profile: `cache_max_bytes` and `preset`.

`OWNSEARCH_BEARER_TOKENS`, `OWNSEARCH_CAPSOLVER_KEY`, `OWNSEARCH_PROXY`, and `OWNSEARCH_PROXIES` are secrets. They come from the environment only, a profile that names one is refused when it loads, and no command prints them.

## Paths and transports

- `OWNSEARCH_PROFILE`: path to the TOML profile.
- `OWNSEARCH_STATE_DIR`: directory for `instances.db`, the snapshot, events, and backups. Default `${XDG_STATE_HOME}/ownsearch`.
- `OWNSEARCH_LEDGER_DIR`: directory for `calls.jsonl`, its archives, and `args.key`. Default `${XDG_DATA_HOME}/ownsearch/ledger`.
- `OWNSEARCH_HTTP`: serve Streamable HTTP instead of stdio. Default false.
- `OWNSEARCH_BIND`: HTTP bind address. Default `127.0.0.1:8774`.
- `OWNSEARCH_BEARER_TOKENS`: comma-separated bearer tokens, each at least 32 characters.
- `OWNSEARCH_ALLOWED_HOSTS`: comma-separated `Host` values for HTTP mode.
- `OWNSEARCH_ALLOWED_ORIGINS`: comma-separated `Origin` values for HTTP mode.
- `OWNSEARCH_LOG_FORMAT`: `text` or `json`. Default `text`. Logs go to stderr only.
- `RUST_LOG`: the log filter, read by the tracing subscriber.
- `OWNSEARCH_PROXY`: an outbound proxy URL for every HTTP request the client makes, for example `socks5h://127.0.0.1:1080`. Unset by default.
- `OWNSEARCH_PROXIES`: a comma-separated proxy list that replaces `OWNSEARCH_PROXY` when it is set. One request goes through each entry in turn, and an entry that fails three times in a row leaves the rotation for five minutes before it is tried again. A proxy URL carries its own credentials, so a list is often a single rotating gateway rather than many hosts. A value without a scheme gets `http://` in front of it.
- `OWNSEARCH_ALLOW_PRIVATE_INSTANCES`: accept loopback and private addresses for discovery records, probes, search, and fetch. Default false, and it exists for local testing only. Leave it off in any deployment that can reach the internet, because with it on a hostile discovery document can point the scanner at a local service.
- `OWNSEARCH_ACCEPT`: the `Accept` header sent on every instance request. Default `text/html,application/json;q=0.9,*/*;q=0.8`. It must contain `text/html`, because a SearXNG limiter rejects a request without it.
- `OWNSEARCH_ALLOW_PLAIN_HTTP_PRIVATE`: permit `http://` for a loopback, private, or local-only host, such as `http://192.168.110.26:8080/`. Default false. Both this setting and `OWNSEARCH_ALLOW_PRIVATE_INSTANCES` must be on; plain http to a public host is always refused. A refused url records `policy.plain_http_refused` and never enters the active fleet. TLS and certificate gates do not apply to an allowed http url.

## Discovery and scanning

- `OWNSEARCH_DISCOVERY_URL`: discovery source. Default `https://searx.space/data/instances.json`. The URL must use HTTPS, except a loopback host.
- `OWNSEARCH_DISCOVERY_REFRESH_SECS`: refresh window. Default 21600.
- `OWNSEARCH_OFFLINE`: skip discovery and use local state only. Default false.
- `OWNSEARCH_PUBLIC_FLEET`: discover from searx.space and probe public instances. Default true. When false, discovery stops, public rows are never probed, and every fleet and search uses only instances added with `ownsearch instances add`.
- `OWNSEARCH_SCAN_CONCURRENCY`: parallel probes, 1 to 8. Default 4.
- `OWNSEARCH_SCAN_JITTER_MS`: random delay between probes, up to this many milliseconds. Default 750.
- `OWNSEARCH_PROBE_DAILY_CAP`: most probes one instance receives in a rolling 24 hours. Default 96, from 1 to 10000. An instance at the cap is skipped and the scan says how many were skipped.
- `OWNSEARCH_INSTANCE_MAX_CONCURRENT`: requests in flight to one trusted instance. Default 4, from 1 to 64. An untrusted instance always allows one at a time.
- `OWNSEARCH_PROBE_TIMEOUT_SECS`: per-request timeout for each probe step. Default 15. One probe stops after five times this value.
- `OWNSEARCH_SCAN_DEADLINE_SECS`: whole-scan deadline. Default 600, from 30 to 7200. The scan stops with a warning when it passes.
- `OWNSEARCH_SCAN_MIN_INTERVAL_SECS`: minimum gap between two writing scans on the same state database. Default 30, from 0 to 3600, and 0 turns the gap off. One scan at a time within a process always applies.
- `OWNSEARCH_FETCH_DEADLINE_SECS`: whole-fetch deadline, including followed links and a browser render. Default 60, from 5 to 600.
- `OWNSEARCH_BROWSER`: render JavaScript pages in headless Chrome. Default true.
- `OWNSEARCH_BROWSER_PATH`: Chrome or Chromium executable, profile key `browser_executable`. Empty uses auto-detection.
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

## Search

- `OWNSEARCH_SEARCH_DEADLINE_FAST_SECS`, `OWNSEARCH_SEARCH_DEADLINE_BALANCED_SECS`, `OWNSEARCH_SEARCH_DEADLINE_EXHAUSTIVE_SECS`: the whole-search deadline for each mode. Defaults 10, 30, and 60 seconds, each from 1 to 300. The `timeout_secs` argument overrides the mode deadline for one call, up to 120 seconds.
- `OWNSEARCH_EXHAUSTIVE_MAX`: instances in `exhaustive` mode, default 16.
- `OWNSEARCH_SEARCH_ATTEMPTS`: attempts per instance, 1 to 5. Default 3.
- `OWNSEARCH_SEARCH_FAILOVER`: fail over to the next instance. Default true.
- `OWNSEARCH_ROUND_ROBIN`: rotate the starting instance. Default true.
- `OWNSEARCH_RESULT_CAP`: default result limit and the ceiling on a call's `limit`. Default 25, from 1 to 100.
- `OWNSEARCH_SEARCH_MAX_CONCURRENT`: searches that run at once, default 8.
- `OWNSEARCH_SEARCH_QUEUE_LIMIT`: searches allowed to wait for a slot, default 64.
- `OWNSEARCH_SEARCH_QUEUE_WAIT_SECS`: how long a queued search waits before it answers `ownsearch.busy`, default 30.
- `OWNSEARCH_CACHE_TTL_SECS`: how long a cached answer stays fresh. Default 300, and 0 turns the cache off.
- Profile key `cache_max_bytes`: cache byte cap. Default 16 MiB.
- Profile key `preset`: default field preset, `minimal`, `standard`, or `full`. Default `standard`.
- Profile key `min_engines`: fewest distinct engines a result needs to survive the merge, from 1 to 16. Default 1. A call can raise it with `min_engines`.
- `OWNSEARCH_FRESHNESS_DAYS`: the freshness window for search results. Default 365, from 0 to 3650, and 0 turns it off. When a call sends no `time_range`, a window of 365 days or fewer also asks SearXNG for the closest covering range (`day`, `week`, `month`, or `year`), and a result whose `publishedDate` is known and older than the window is dropped. A result with no date is kept, because it cannot be proven stale.
- `OWNSEARCH_ENGINE_YIELD_THRESHOLD`: the share of enabled engines that must contribute results before an instance counts as healthy. Default 0.5, from 0 to 1. Under it, the score is halved and the reason `quality.engine_degraded` is recorded; a pinned or trusted instance stays eligible but carries the reason.
- Profile key `search_presets`: named intent presets. Each entry sets `categories`, `engines_allow`, `engines_deny`, `engine_weights`, and `min_engines`, and replaces the built-in value for every key it sets. The built-in names are `web`, `code`, `news`, and `images`; `code` asks for `it` and `general`, drops `wiby`, `bing`, and `yep`, and boosts `github`, `stackoverflow`, `mdn`, `docker hub`, `pypi`, and `crates.io`.
- Profile key `engine_weights`: table of engine name to weight, each from 0 to 10, used by the merge ranking. Built-in weights already rank wiby at 0.25 and other small-web engines at 0.4, and every other engine at 1.0. An entry here replaces the built-in weight for that engine.
- Profile key `exclude_engines`: engines whose results are dropped before the merge.
- Profile key `exclude_domains`: domains dropped from results, matching the host and every subdomain.

The merge rank is reciprocal rank fusion over each engine position with the engine weight, plus a bonus for each extra agreeing engine and instance, plus SearXNG's own score. The full formula is in [docs/tools.md](tools.md).

## Keep windows and maintenance

- `OWNSEARCH_KEEP_WINDOW`: how long to keep ledger archives, event lines, and pre-migration backups. Accepts `90s`, `15m`, `2h`, `3d`, or plain seconds. Default `3d`. Minimum 1 hour, maximum 90 days, and a negative or malformed value is refused at startup.
- `OWNSEARCH_MAINTENANCE_INTERVAL_SECS`: how often the running server runs the cleaner. Default 3600, minimum 60, maximum 86400.
- `OWNSEARCH_WATCHDOG_SECS`: how long the process may go without progress before it exits with code 75 so a supervisor restarts it. Default 120, from 0 to 3600, and 0 turns it off.

The cleaner runs at startup, after every applied scan, on that interval while a server runs, and inside `ownsearch db maintenance`. It rotates the live ledger when its oldest row passes the keep age, deletes ledger archives and `pre-migration-*.db` files older than the window, and trims old lines from `instances.events.jsonl`. The trim holds a lock file, so an event written at the same moment is never lost. The same interval also expires old rows in SQLite: probes, TLS observations, capability snapshots, metrics, snapshots, events, and discovery runs each keep their own window, and `ownsearch db maintenance` reports what it removed. Two processes on one state database take turns through a lock file, so a sweep never runs twice. If a file cannot be removed, the run logs a warning that names `ownsearch db maintenance` as the manual repair, and a sweep that reports a problem writes an `attention` row to `maintenance_runs`.

## AI database changes

- `OWNSEARCH_AI_DB_MODE`: `read-only` or `read-write`. Default `read-write`, because every change still needs a preview and a one-use token; set `read-only` to refuse all typed database changes.
- `OWNSEARCH_AI_DB_MAX_CHANGES_PER_CALL`: field changes per call, default 5.
- `OWNSEARCH_AI_DB_CONFIRM_TTL_SECS`: confirmation lifetime, default 120.
- `OWNSEARCH_AI_DB_MIN_INTERVAL_SECS`: minimum gap between applies, default 2.

## CapSolver

- `OWNSEARCH_CAPSOLVER_KEY`: the CapSolver API key. Unset by default, and no challenge is solved without it.
- `OWNSEARCH_CAPSOLVER_TIMEOUT_SECS`: how long to wait for a solved challenge, profile key `capsolver_timeout_secs`. Default 45, from 10 to 180. The client polls every 3 seconds.

## Profile example

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
public_fleet = true
min_engines = 1
engine_yield_threshold = 0.5
exclude_domains = ["example.com"]
search_deadline_fast_secs = 10
search_deadline_balanced_secs = 30
search_deadline_exhaustive_secs = 60

[engine_weights]
wiby = 0.2
google = 1.0

[search_presets.code]
categories = ["it", "general"]
engines_deny = ["wiby", "bing", "yep"]
[search_presets.code.engine_weights]
github = 2.0
```

## Private hosts over HTTPS

`OWNSEARCH_ALLOW_PRIVATE_INSTANCES=true` also covers a public hostname that resolves to a private address, such as a production domain behind Caddy on the LAN or inside a tunnel. With a valid public certificate, the TLS and certificate gates pass and the private-address guard stands down. With the setting off, the probe refuses the address and records `gate.tls_invalid` with a message naming the non-public address.
