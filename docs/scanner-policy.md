# Scanner policy

This document states how OwnSearch treats public SearXNG instances. The code follows it, and a change to the code needs a change to this file.

## Schedule

- Startup: the embedded snapshot loads on a first run, and a discovery refresh runs when local state is older than the refresh window.
- Periodic: a scheduler refreshes discovery on `OWNSEARCH_DISCOVERY_REFRESH_SECS` and then probes the instances whose next scan time has passed, up to the fleet size in one pass.
- Serialized: one scan runs at a time per process, with the minimum interval between writing scans, and each probe waits a random jitter delay.
- On demand: `ownsearch scan` and `ownsearch_scan` run a scan on request. The one-at-a-time rule and the minimum interval still apply.

## Request budget

- Discovery: one fetch of `https://searx.space/data/instances.json` per refresh window. The window is `OWNSEARCH_DISCOVERY_REFRESH_SECS`, default 21600 seconds.
- Per instance per scan: one TLS resolution, one TLS handshake, at most four HTTP requests (`/config`, the JSON search, at most one HTML search, and at most one exact-Google search), and two address resolutions per HTTP request, one in the public-address guard and one in the HTTP client.
- Probes run four at a time by default, with jitter between instances. The jitter is random from zero to `OWNSEARCH_SCAN_JITTER_MS`, default 250 ms.
- `OWNSEARCH_SCAN_CONCURRENCY` sets the probe concurrency from 1 to 8 for any run, scheduled or on demand. Discovery and probing are never unbounded.
- A search request uses at most `exhaustive_max` instances, default 16, and stops waiting at the search deadline, default 30 seconds. A bounded queue admits 8 searches at once, lets 64 wait, and refuses the rest with `ownsearch.rate_limited`.

## User agent

Every request sends:

```
OwnSearch (MCP fleet scanner; +https://github.com/devops-infinity/ownsearch) v<version>
```

The agent names the product and a URL where an operator can read what it is. It does not claim to be a browser.

## Retry policy

- No retry after `429`, `401`, `403`, or `404`, and no retry after a challenge page.
- A network error, a timeout, or a `5xx` on the search path is retried up to `OWNSEARCH_SEARCH_ATTEMPTS` times, default 3. After the last attempt the instance cools down and the search moves to the next instance.
- A scan probe is never retried in the same run.
- The search reports every failed instance in `sources.failed` and walks the whole scored fleet in order, skipping an instance that is cooling down, until it reaches its target or the deadline, so an answer low in the list is still found.

## Challenge policy

OwnSearch treats a challenge page as a refusal. It detects known markers (Anubis, "Verifying your browser", "Making sure you're not a bot", CAPTCHA forms, Cloudflare and DDoS-Guard pages), records the probe as blocked, and quarantines the instance. It never solves a challenge, never runs a proof of work, and never drives a browser.

## Cooldown and quarantine

- Any 429 sets `cooldown_until` to now plus `OWNSEARCH_COOLDOWN_SECS`, default 3600 seconds.
- A failed search sets the cooldown and records an event. It changes neither the lifecycle status nor the probe counters, so four bad searches cannot quarantine a working instance. Only a scan probe or an operator command moves an instance between active, probation, and quarantine.
- Repeated probe failures move an instance to probation, then to quarantine.
- A challenge page quarantines at once.
- A quarantined instance keeps its evidence and its history. It is never deleted.

## Fleet selection

- A hard gate failure keeps an instance out of the active fleet.
- At most `OWNSEARCH_FLEET_MAX` instances are active, default 12.
- At most `OWNSEARCH_MAX_PER_NETWORK` active instances share one announced network prefix, default 2.
- Tor records stay in state and out of the normal fleet.
- Hysteresis needs repeated good scans to promote and repeated bad scans to demote.

## Operator opt-out

An operator who does not want automated probes can be excluded in two ways:

- Ask for the instance to be denied. The operator of this tool records a policy override with `ownsearch instances deny <id> --reason "operator request"`. The instance is quarantined and no further probes are sent.
- Publish a rate limit or a challenge for the client. OwnSearch records the refusal and stops.

## Data kept

- Probe rows: 90 days.
- TLS observations: 90 days.
- Capability snapshots: 90 days.
- Hourly metrics: 180 days.
- Discovery runs: 180 days.
- Fleet snapshots: 30 days.
- Lifecycle events: 365 days.
- Instance rows: kept until an operator retires them. Never deleted by the scanner.

## What is never sent

- No cookie jar is kept between requests.
- No query text is stored anywhere after the call.
- No header is added to spoof a browser or a referrer.
- No proxy rotation, no key rotation, no IP-block evasion.
