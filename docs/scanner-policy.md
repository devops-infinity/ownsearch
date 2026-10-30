# Scanner policy

How OwnSearch treats public SearXNG instances: when it contacts them, what it sends, how it decides an instance is healthy, and what it keeps. The complete setting catalog is in [docs/configuration.md](configuration.md).

## Schedule

- Startup: the embedded snapshot of 67 instances loads on a first run, and a discovery refresh runs when local state is older than the refresh window.
- Periodic: a scheduler refreshes discovery on `OWNSEARCH_DISCOVERY_REFRESH_SECS` and then probes the instances whose next scan time has passed.
- Serialized: one scan runs at a time per state database, and two processes on one state database take turns through a file lock.
- On demand: `ownsearch scan` and `ownsearch_scan` run a scan on request. The one-at-a-time rule and the minimum interval still apply.

A scan is skipped in offline mode, and while the database is in read-only AI mode. With `OWNSEARCH_PUBLIC_FLEET=false`, discovery is skipped, only instances added by hand are probed, and a search uses only those instances.

## Next scan after a probe

- Success: 3600 seconds.
- Failure: 900 seconds.
- Quarantined: 21600 seconds.
- Rate limited or blocked: the cooldown window.

## Request budget

- Discovery: one fetch of `https://searx.space/data/instances.json` per refresh window. The document is capped at 500 records.
- Per instance per scan, at most five HTTP requests: `/config`, the JSON search, an HTML search, and one of RSS or CSV when JSON is denied. An exact-Google search runs only when JSON succeeded and the instance advertises the engine.
- One instance receives at most `OWNSEARCH_PROBE_DAILY_CAP` probes in a rolling 24 hours. An instance at the cap is skipped, and the scan reports how many were skipped.
- One address resolution per request hop in the public-address guard. The connection is then pinned to the resolved address, so the client does not resolve the host again.
- At most one headless-browser render of the search page per probe, when the instance denies JSON, and only when the instance is neither rate limited nor blocked.
- A search requests one batch of instances: one in `fast`, three in `balanced`, and up to `OWNSEARCH_EXHAUSTIVE_MAX` in `exhaustive`. When that batch fails, failover walks the rest of the scored fleet one instance at a time until the target is reached or the deadline passes.

## Concurrency and pacing

- Probes run `OWNSEARCH_SCAN_CONCURRENCY` at a time.
- Discovery address checks resolve 8 hosts at a time.
- Each probe waits a random delay up to `OWNSEARCH_SCAN_JITTER_MS` before it starts.
- A trusted instance accepts `OWNSEARCH_INSTANCE_MAX_CONCURRENT` requests at once. Every other instance takes one at a time.
- A writing scan waits `OWNSEARCH_SCAN_MIN_INTERVAL_SECS` after the last scan on that state database. A forced scan waits for the lock, up to 120 seconds, and clears the cooldown.
- One probe step times out after `OWNSEARCH_PROBE_TIMEOUT_SECS`, and a probe runs five steps.
- A whole scan stops with a warning at `OWNSEARCH_SCAN_DEADLINE_SECS`.
- A search admits `OWNSEARCH_SEARCH_MAX_CONCURRENT` calls at once, lets `OWNSEARCH_SEARCH_QUEUE_LIMIT` wait, and answers `ownsearch.busy` to the rest.

## User agent and request headers

Every instance request sends:

```
OwnSearch (MCP fleet scanner; +https://github.com/devops-infinity/ownsearch) v<version>
```

The agent names the product and a URL where an operator can read what it is. It does not claim to be a browser. Two clients carry a different agent: the CapSolver client sends `OwnSearch captcha client`, and the headless browser sends the user agent of the Chromium build it runs.

The HTTP client sends `Accept`, `Accept-Language`, and `Accept-Encoding` (`gzip`, then brotli). The `Accept` value comes from `OWNSEARCH_ACCEPT` and defaults to `text/html,application/json;q=0.9,*/*;q=0.8`. A SearXNG limiter rejects a request whose `Accept` lacks `text/html`, whose `Accept-Encoding` lacks gzip or deflate, whose `Accept-Language` is empty, or whose User-Agent is missing. It adds no referrer and no browser user agent.

## Retry policy

- No retry after `429`, `401`, `403`, or `404`, and no retry after a challenge page.
- On the search path, a network error, a timeout, or a `5xx` is retried up to `OWNSEARCH_SEARCH_ATTEMPTS` times per instance, with a backoff that doubles from 100 milliseconds and adds up to 49 milliseconds of jitter.
- Retrying runs only while `OWNSEARCH_SEARCH_FAILOVER` is on.
- A `408` is retried as `ownsearch.upstream_failed`.
- A scan probe is never retried in the same run.
- The search reports every failed instance in `sources.failed` and walks the scored fleet in order, skipping an instance that is cooling down, until it reaches its target or the deadline passes.

## Challenge policy

A challenge page found in a fetched or parsed answer counts as a refusal. OwnSearch detects known markers (Anubis, "Verifying your browser", "Making sure you're not a bot", CAPTCHA forms, Cloudflare and DDoS-Guard pages), records the probe as blocked, quarantines the instance, and runs no proof of work.

When `OWNSEARCH_CAPSOLVER_KEY` is set, a challenge that appears only in the rendered page is sent to the CapSolver service for that one page. The key, the page URL, and the site key go with it, and `OWNSEARCH_CAPSOLVER_TIMEOUT_SECS` bounds the wait. Without the key, no challenge is ever solved.

## The SearXNG limiter

With its limiter on, SearXNG counts requests per IPv4 /32 or IPv6 /48. An HTML search allows 15 requests per 20 seconds and 150 per 600 seconds. The `json`, `csv`, and `rss` formats allow only 4 per 3600 seconds, hard-coded. `pass_ip` on the OwnSearch egress address skips every check and is the cleaner fix for an instance you run.

OwnSearch detects a limiter-protected instance when a JSON request answers `429` while the HTML request from the same client succeeds, or when the answer carries `Retry-After`. It then:

- records `limiter.json_throttled` and stores the throttle window on the row, shown as `jsonThrottledUntil`;
- keeps the instance in service and sends searches over the HTML path while JSON is throttled;
- returns to JSON when `Retry-After` or the hourly window passes, and clears the marker after the next JSON success;
- reads `Retry-After` on every `429` and never waits less than the instance asks;
- probes a trusted instance with at most one JSON request per hour and prefers the HTML probe path.

A `429` on the config request, or on JSON and HTML together, is a full rate limit: the instance cools down as before and is not marked as JSON-throttled.

## Cooldown and quarantine

- A `429` sets `cooldown_until` to now plus four times `OWNSEARCH_COOLDOWN_SECS`, because a rate limit asks for a longer rest than a plain failure.
- A challenge sets the cooldown and quarantines the instance at once.
- A `401` quarantines the instance at once.
- A failed search sets the cooldown and records an event. It changes neither the lifecycle status nor the probe counters, so repeated bad searches cannot quarantine a working instance.
- `OWNSEARCH_PROMOTE_SUCCESSES` good scans promote a candidate or probation row to active. `OWNSEARCH_PROBATION_FAILURES` failed scans demote an active row to probation, and `OWNSEARCH_QUARANTINE_FAILURES` failed scans quarantine it.
- A row that fails a hard gate moves to probation, and to quarantine after 8 failed gate checks.
- A quarantined instance keeps its evidence and its history. It is never deleted.

## Fleet selection

- A hard gate failure keeps an instance out of the active fleet.
- The fleet holds at least `OWNSEARCH_FLEET_MIN` and at most `OWNSEARCH_FLEET_MAX` instances. A fleet below the minimum is reported in the scan warnings.
- At most `OWNSEARCH_MAX_PER_NETWORK` active instances share one announced network prefix.
- Tor records stay in state and out of the normal fleet.
- When `OWNSEARCH_GOOGLE_REQUIRED` is set, an instance without exact Google support is excluded, unless it is pinned.
- A pinned instance enters the fleet from candidate, missing, or unlisted, ranks first, and skips the per-network cap and the Google gate. A failed hard gate still excludes it.
- A trusted instance is sorted ahead of every untrusted one, skips the per-network cap and the Google gate, skips the browser fallback, and accepts `OWNSEARCH_INSTANCE_MAX_CONCURRENT` requests at once. A failed hard gate still excludes it. Set it with `ownsearch instances set-trusted ID`, or `ownsearch instances add URL --trusted`.
- A manual priority, set with `ownsearch instances set-priority ID N` or the `priority` field, orders instances before the score. A higher number is tried first.
- An instance whose engine yield falls under `OWNSEARCH_ENGINE_YIELD_THRESHOLD` has its score halved and carries `quality.engine_degraded`. A pinned or trusted instance stays eligible and still shows the reason. The yield is the number of enabled engines that returned at least one result divided by the number of enabled engines in `/config`.
- With `OWNSEARCH_PUBLIC_FLEET=false`, the expected fleet size is the number of added instances, not `OWNSEARCH_FLEET_MIN`, so a self-hosted setup gets no minimum warning.
- A missing instance returns to probation after a clean probe.

A discovery record is marked missing only when the source document lists 20 or more records and at least half the known instance count, so a truncated or hostile document cannot wipe the fleet.

## Operator controls

- `deny` quarantines the instance, or marks it `excluded` when the row is Tor, and stops the scanner. `allow` clears the denial without changing the status.
- `retire` removes an instance from the fleet and keeps its rows. `restore` returns a retired, missing, unlisted, quarantined, or denied instance to candidate.
- `pin` and `unpin` set the pin. `set-type` records normal, tor, or unknown.
- An operator who does not want automated probes can be excluded in two ways: ask for the instance to be denied with `ownsearch instances deny <id> --reason "operator request"`, or publish a rate limit or a challenge, which OwnSearch records and then respects.

## Private-address guard

- A discovery record or fetch target whose host resolves to a loopback, private, link-local, documentation, or reserved address is refused. The refusals cover IPv4 and IPv6, including IPv4-compatible, site-local, NAT64 local-use, benchmarking, ORCHID, RFC 9637 documentation, and SRv6 prefixes.
- The browser is held to the same rule: every request and the address Chromium connects to are checked, and a render whose final URL is not public is refused.
- `OWNSEARCH_ALLOW_PRIVATE_INSTANCES` turns the guard off for local testing. With it on, a hostile discovery document can point the scanner at a local service. The same setting lets a public hostname that resolves to a private address pass the guard, so a production domain behind Caddy on the LAN works over HTTPS with a valid certificate.
- Plain http is refused by default, because a certificate cannot describe a url that has none. `OWNSEARCH_ALLOW_PLAIN_HTTP_PRIVATE` permits it, but only together with `OWNSEARCH_ALLOW_PRIVATE_INSTANCES` and only for a loopback, private, or local-only host. The rule is applied the same way in discovery, `instances add`, the probe, and search. A refused url records `policy.plain_http_refused`, the row is quarantined, and no TLS or certificate gate runs against it, so it never appears as `gate.tls_invalid`, `gate.cert_unknown`, or `probe.json_denied`. An allowed http url skips the TLS and certificate gates.

## Instance credentials

A protected instance can demand a header or HTTP Basic. The credential is stored as an environment variable name, resolved at request time, and applied only to the instance's own origin, measured on scheme, host, and port. A redirect to another origin drops it. An authenticated instance skips the browser fallback, because the browser cannot scope a header to one origin in this client; its HTML path carries the credential instead.

## Data kept

- Probe rows: 90 days.
- TLS observations: 90 days.
- Capability snapshots: 90 days.
- Hourly metrics: 180 days.
- Discovery runs: 180 days.
- Fleet snapshots: 30 days, capped at 200 rows.
- Lifecycle events: 365 days.
- Instance rows: kept until an operator retires them. Never deleted by the scanner.

`OWNSEARCH_KEEP_WINDOW` covers the ledger archives, the `instances.events.jsonl` lines, and the pre-migration backups. The per-table windows above are fixed.

## Data never sent

- The HTTP client keeps no cookie jar between requests. The headless browser keeps cookies for its own session and deletes them with its profile when the session closes.
- No query text is stored anywhere after the call.
- No proxy rotation and no key rotation.
