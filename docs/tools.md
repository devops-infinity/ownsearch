# Tools and search behavior

Reference for the eight MCP tools, the arguments each accepts, how a search picks its instances, and how an instance earns its score.

Every search and fetch answer carries `outcome`, one of `success`, `partial`, `empty`, or `failed`, and `messages`, a list of notices. A notice has `level` (`info`, `warning`, or `error`), a stable `code`, a plain `message`, and a `nextStep` that names the action available to the caller. A client reads `outcome` first, then the notices, so it can decide its next call without parsing prose. A hard failure still returns the error body with `code`, `message`, and `remedy`.

A search that loses a source adds a `search.sources_failed` warning with the count and the retry step, and every entry in `sources.failed` names the `parser` that was serving that source when it failed.

## Tools

- `ownsearch_search`: federated web search. Arguments: `query` (required), `categories`, `engines`, `language`, `page`, `time_range` (`day`, `week`, `month`, `year`), `safesearch` (`off`, `moderate`, `strict`), `limit`, `fields` (`minimal`, `standard`, `full`), `mode` (`fast`, `balanced`, `exhaustive`), `timeout_secs` (up to 120), `result_text` (`summary`, `full`), `dedupe` (default true), `min_engines` (1 to 16), `preset` (`web`, `code`, `news`, `images`), and `freshness_days` (0 to 3650). It may refresh local fleet state in the background and never waits on that refresh.

An intent preset adds categories, engine weights, and an engine deny list for one call. `code` asks for the `it` and `general` categories, drops the small-web engines, and boosts `github`, `stackoverflow`, `mdn`, `docker hub`, `pypi`, and `crates.io`. An explicit `categories` or `engines` argument replaces the preset's value. The profile can redefine each preset under `search_presets`.

Results are held to a freshness window. The default is `OWNSEARCH_FRESHNESS_DAYS=365`, so a result whose `publishedDate` is known and older than a year is dropped, and the upstream request asks for `time_range=year` when the call sends no `time_range`. A result with no date is kept. A call sets its own window with `freshness_days`, `0` turns it off for that call, and an explicit `time_range` argument wins over the mapped value.

Each mode has its own deadline: `fast` 10 seconds, `balanced` 30, `exhaustive` 60 at the defaults. When the deadline passes with at least one answer in hand, the call returns what arrived with `coverage.partial: true`, and every instance that did not answer is listed in `sources.failed` with `ownsearch.timeout`. A slow instance never turns a partial answer into an error.
- `ownsearch_fetch`: fetch a public page as `markdown` (default), `text`, `html`, or `json`, with `url` required and `max_chars` from 1000 to 200000. `max_pages` runs from 1 to 8 and defaults to 3; beyond 1 it follows links from the first page and returns them under `pages`, staying on the same host unless `same_host_only` is false. A link that does not answer or does not parse is reported as a `fetch.links_skipped` notice, and a walk that stops early is reported as `fetch.links_capped`. An HTTP redirect is followed up to three hops, each hop passes the same public-address check as the first request, and the answer reports `finalUrl` plus the `redirects` chain so a caller never has to guess where a link ended up. `statusSource` says where `status` came from: `http` when the host answered it, and `browser` when the page was rendered and no HTTP status was observed. An `application/pdf` answer is parsed under an 8 MiB decompressed-text cap and shaped into Markdown paragraphs. `render` chooses the browser: `auto` (the default) renders a page that looks like an empty JavaScript shell or whose extraction found no text, `always` renders every call, and `never` stays on HTTP. Extraction runs a chain and stops at the first pass that yields at least 200 characters: the densest content root first, then the whole body with boilerplate pruned, then the raw document. The answer reports the `parser` that produced the text and the attempts it made, and `json` adds a `document` with the title, description, canonical URL, language, publication date, author, site name, content, headings, links, and JSON-LD blocks. Navigation, sidebars, cookie banners, share blocks, comment sections, and footers are pruned, and code fences carry their language. Private and loopback addresses are refused, and every page is capped and sanitized.
- `ownsearch_instance_info`: lifecycle and capability facts for one instance or a list, including its `slug`, status, score, rank, version, priority, JSON and HTML access, the enabled engines and categories read from `/config`, rolling per-engine success and failure counts, the stored credential as a description (never a value), and exact-Google evidence. Every result source and every scan status change carries the slug; a failed source names its instance id and url. `ownsearch instances engines ID` prints the same engine list from a terminal.
- `ownsearch_instance_health`: hard gate result, certificate days left, TLS protocol and grade, median search time, cooldown, the JSON throttle window, engine yield with its degraded flag, trust, consecutive failures, and the last lifecycle events.
- `ownsearch_scan`: refresh discovery, probe instances, rescore, and reselect the fleet. It defaults to `dry_run: true`, and a dry run writes nothing: it probes and reports the status changes and the fleet a real run would write, in `summary`. Only `dry_run: false` writes lifecycle status, quality, and fleet ranks, and it requires a `reason`. Within one process, a writing scan runs one at a time per state database, waits `OWNSEARCH_SCAN_MIN_INTERVAL_SECS` between runs, and refuses a named instance that is not yet due unless the call sets `force`.
- `ownsearch_session`: local call counts, cache state, ledger length and head, and fleet size. No network call.
- `ownsearch_db_query`: bounded read-only queries over instances, quality, policy overrides, capability snapshots, events, snapshots, discovery runs, and hourly probe metrics. Each entity applies its own filters, and a filter that the selected entity cannot apply is refused instead of ignored. Events page in SQL up to a 50000 row offset, and a larger offset is refused rather than repeated. It never accepts SQL.
- `ownsearch_db_crud`: one typed lifecycle or policy change. It needs write mode, a reason, and a one-use confirmation token from a dry-run preview. On an `instance` it creates from a URL and updates every field: `url` or `domain`, `status`, `type`, `priority`, `scanEnabled`, `htmlFallbackAllowed`, `pinned`, `trusted`, `denied`, and `auth`. It also retires, restores, and deletes an instance with its probes, capabilities, events, counters, credentials, and policy rows. On a `policy_override` it updates or deletes the override. A `status` of `active` still needs a passing hard gate, and `excluded` still belongs to a Tor row.

Every tool returns `structuredContent` plus a one-line text summary. Every input schema sets `additionalProperties: false`. Every field a preset may drop is optional in the published output schema.

## Search behavior

Modes control how many instances answer one query.

- `fast` targets one instance.
- `balanced`, the default, targets three.
- `exhaustive` targets `OWNSEARCH_EXHAUSTIVE_MAX`.

Failover works the same way in every mode. The selected instances answer in parallel. A network error, a timeout, or a `5xx` is retried up to `OWNSEARCH_SEARCH_ATTEMPTS` times per instance. A `4xx` other than `408` and `429`, a `403`, or a challenge page is never retried. When an instance still fails, OwnSearch sets its `cooldown_until`, records a `search_failed` event, and tries the next instance in score order until it reaches its target or the mode's deadline (`OWNSEARCH_SEARCH_DEADLINE_FAST_SECS`, `_BALANCED_`, or `_EXHAUSTIVE_`).

A failed search changes neither the status nor the probe counters. A scan probe or an operator command moves an instance between active, probation, and quarantined, and only a probe resets the success and failure counts. Failover keeps walking the scored fleet in order until it reaches its target or the deadline, so an answer low in the list is still found.

A bounded queue sits in front: `OWNSEARCH_SEARCH_MAX_CONCURRENT` searches run at once, up to `OWNSEARCH_SEARCH_QUEUE_LIMIT` wait for a slot for `OWNSEARCH_SEARCH_QUEUE_WAIT_SECS`, and the rest answer `ownsearch.busy` without stacking work. That bound is what keeps memory and CPU flat on a small machine.

An answer with zero results is a success, not a failure. Only a page whose results carry no usable title and URL counts as a parser failure.

Round-robin rotates the starting point inside the top healthy window, so two searches in a row do not always begin with the same instance. Turn it off with `OWNSEARCH_ROUND_ROBIN=false` when you prefer a strict score order.

Before any scan has scored the fleet, or when every scored instance is cooling, a search falls back to the highest ranked discovered instances that have not failed a hard gate, rather than failing with `ownsearch.no_fleet`, and the response says so in a `search.fleet_fallback` notice. The response never invents a result: when every attempt is refused or rate limited, `sources.failed` names each instance, its code, and what it answered. `sources.fallbacks` names each instance whose answer came from a parser other than JSON: HTML, RSS, CSV, or the browser.

Field presets control payload size. `minimal` keeps title, URL, rank, score, and source id. `standard` adds the snippet, engines, published date, merged-from count, and the full source list. `full` keeps everything, including the result category and `sources.unresponsiveEngines`, which names every engine an instance reported as unresponsive with its reason.

Choose `limit` by the job. Five results answers one fact, ten suits a normal search, and twenty-five is a sweep. A result costs about 150 tokens at `standard` and about 50 at `minimal`.

## Merge, duplicates, and rank

Each upstream result keeps its `engines`, `positions`, `score`, `category`, and `publishedDate`. A search merges every result with the same normalized url across engines and instances. Normalization lowercases the scheme and host, drops a leading `www.`, drops the default port and the fragment, drops tracking parameters (`utm_*`, `fbclid`, `gclid`, and the other click ids), sorts the remaining parameters, strips the trailing slash, treats http and https of one host and path as one, and unwraps AMP links and the common redirect links from Google, DuckDuckGo, Facebook, and YouTube. A merged entry keeps the best title, the longest clean snippet, the union of engines and sources, the best position (`bestPosition`), and a `mergedFrom` count.

The rank is one documented formula, computed as a pure function:

- Reciprocal rank fusion: for every engine position, `weight / (60 + position)`, summed over every source. A built-in weight table ranks wiby at 0.25 and other small-web engines at 0.4, and every other engine at 1.0; `engine_weights` replaces any entry.
- The fusion sum is divided by the highest fusion sum in the answer, giving 0 to 1.
- Engine agreement adds up to 0.20: one quarter for each extra engine, capped at four.
- Instance agreement adds up to 0.10: one third for each extra instance, capped at three.
- SearXNG's own score adds up to 0.15, averaged over the sources.
- The final score is `100 * (0.55 * fusion + 0.20 * engines + 0.10 * instances + 0.15 * searxng)`, rounded to two places.

A result backed by three or more engines therefore ranks above every single-engine novelty result at the default weights, and the fixture test in `tests/merge_acceptance.rs` proves it on a recorded answer. `exclude_engines`, `exclude_domains`, and `min_engines` drop results before ranking. With `dedupe: false`, every observation stays its own result.

## Timing and rehearsal commands

Every search answer carries `timing`: `queueMs` waiting for a search slot, `selectionMs` resolving and ordering the fleet, `upstreamMs` in the instance requests, `parseMs` shaping merged results into items, `mergeMs` in the merge, and `elapsedMs` for the whole call.

- `ownsearch bench --queries FILE --count N --concurrency C --duration D --instance ID --json` runs repeated searches and reports p50, p95, and p99 latency, the mean, the error rate by code, results per query, unresponsive engine counts, the cache hit rate, and this process's RSS and CPU. Without `--queries` it uses ten built-in queries. With `--duration` it keeps going until the time passes. `--instance` sends every query to the named instance.
- `ownsearch health --instance ID` probes one instance and exits zero when it answers and non-zero when it does not, which fits a container health check. `--json` prints the report.

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
