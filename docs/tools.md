# Tools and search behavior

Reference for the eight MCP tools, the arguments each accepts, how a search picks its instances, and how an instance earns its score.

## Tools

- `ownsearch_search`: federated web search. Arguments: `query` (required), `categories`, `engines`, `language`, `page`, `time_range` (`day`, `week`, `month`, `year`), `safesearch` (`off`, `moderate`, `strict`), `limit`, `fields` (`minimal`, `standard`, `full`), `mode` (`fast`, `balanced`, `exhaustive`), `timeout_secs`, and `result_text` (`summary`, `full`). It may refresh local fleet state before answering.
- `ownsearch_fetch`: fetch a public page as `markdown` (default), `text`, or `html`, with `url` required and `max_chars` from 1000 to 200000. `max_pages` runs from 1 to 8 and defaults to 3; beyond 1 it follows links from the first page and returns them under `pages`, staying on the same host unless `same_host_only` is false. An HTTP redirect is followed up to three hops, each hop passes the same public-address check as the first request, and the answer reports `finalUrl` plus the `redirects` chain so a caller never has to guess where a link ended up. An `application/pdf` answer is parsed under an 8 MiB decompressed-text cap and shaped into Markdown paragraphs. `render` chooses the browser: `auto` (the default) renders only a page that looks like an empty JavaScript shell, `always` renders every call, and `never` stays on HTTP. Markdown is extracted from the main content: navigation, sidebars, cookie banners, share blocks, comment sections, and footers are pruned, the densest content root wins over link lists, and code fences carry their language. Private and loopback addresses are refused, and every page is capped and sanitized.
- `ownsearch_instance_info`: lifecycle and capability facts for one instance or a list, including its `slug`, status, score, rank, version, JSON and HTML access, engines, categories, and exact-Google evidence. Every result source and every scan status change carries the slug; a failed source names its instance id and url. The `/config` instance name comes from `ownsearch_db_query` with `entity: "capabilities"`.
- `ownsearch_instance_health`: hard gate result, certificate days left, TLS protocol and grade, median search time, cooldown, consecutive failures, and the last lifecycle events.
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

Failover works the same way in every mode. The selected instances answer in parallel. A network error, a timeout, or a `5xx` is retried up to `OWNSEARCH_SEARCH_ATTEMPTS` times per instance. A `4xx` other than `408` and `429`, a `403`, or a challenge page is never retried. When an instance still fails, OwnSearch sets its `cooldown_until`, records a `search_failed` event, and tries the next instance in score order until it reaches its target or the search deadline (`OWNSEARCH_SEARCH_DEADLINE_SECS`).

A failed search changes neither the status nor the probe counters. A scan probe or an operator command moves an instance between active, probation, and quarantined, and only a probe resets the success and failure counts. Failover keeps walking the scored fleet in order until it reaches its target or the deadline, so an answer low in the list is still found.

A bounded queue sits in front: `OWNSEARCH_SEARCH_MAX_CONCURRENT` searches run at once, up to `OWNSEARCH_SEARCH_QUEUE_LIMIT` wait for a slot for `OWNSEARCH_SEARCH_QUEUE_WAIT_SECS`, and the rest answer `ownsearch.busy` without stacking work. That bound is what keeps memory and CPU flat on a small machine.

An answer with zero results is a success, not a failure. Only a page whose results carry no usable title and URL counts as a parser failure.

Round-robin rotates the starting point inside the top healthy window, so two searches in a row do not always begin with the same instance. Turn it off with `OWNSEARCH_ROUND_ROBIN=false` when you prefer a strict score order.

Before any scan has scored the fleet, or when every scored instance is cooling, a search falls back to the highest ranked discovered instances that have not failed a hard gate, rather than failing with `ownsearch.no_fleet`, and the response says so in `warnings`. The response never invents a result: when every attempt is refused or rate limited, `sources.failed` names each instance, its code, and what it answered. `sources.fallbacks` names each instance whose answer came from the HTML parser instead of JSON.

Field presets control payload size. `minimal` keeps title, URL, rank, score, and source id. `standard` adds the snippet, engines, published date, and the full source list. `full` keeps everything.

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

The rules behind these numbers are in [docs/scanner-policy.md](scanner-policy.md).
