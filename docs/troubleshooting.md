# Troubleshooting

Each entry names the message you see and the step that clears it. For a running service, the operator path is in [docs/operations.md](operations.md).

## `ownsearch.no_fleet`

Run `ownsearch scan`, then check `ownsearch instances list`. Before any scan has scored the fleet, a search falls back to scanned candidates that passed every hard gate and says so in `warnings`.

## A search answers with `coverage.partial` true

The search deadline passed before every selected instance answered. What arrived is kept, and `sources.failed` names each instance that missed it with `ownsearch.timeout`. Raise the mode deadline, or use `mode=fast`, when the extra sources matter.

## Every search returns no results and `sources.failed` says `ownsearch.policy_refused`

The selected instances deny JSON and HTML. Run from a network the instances accept, or check `ownsearch instances list` for quarantined records.

## `policy refused the request: plain http is refused because allow_plain_http_private is off`

OwnSearch refuses `http://` by default. For an instance on your own machine or network, set `OWNSEARCH_ALLOW_PRIVATE_INSTANCES=true` and `OWNSEARCH_ALLOW_PLAIN_HTTP_PRIVATE=true`, then scan again. Both are needed. A public host over plain http is always refused, and the instance records `policy.plain_http_refused` until it is reachable over https or both settings are on. `ownsearch instances add` prints the decision as soon as the row is written.

## `limiter.json_throttled`

A SearXNG limiter allows only 4 JSON requests per IP per hour. The instance answered a JSON request with `429` while its HTML page worked, so OwnSearch marked the JSON path throttled and now searches it over HTML. It returns to JSON when `Retry-After` or the hourly window passes. The cleaner fix is to add OwnSearch's egress address to the instance's `pass_ip` list. `ownsearch_instance_health` shows `jsonThrottledUntil`.

## `policy.auth_unavailable`, or a message naming an environment variable

The instance has credentials stored, and the named environment variable is unset or empty in this process. Export it for the server and for the CLI, then run `ownsearch health --instance ID` to confirm. `ownsearch instances show ID` prints which variable the instance needs.

## The profile is refused with `unknown field search_deadline_secs`

That key was replaced by `search_deadline_fast_secs`, `search_deadline_balanced_secs`, and `search_deadline_exhaustive_secs`. Delete the old line or replace it with the three keys.

## `ownsearch.state_corrupted`

Run `ownsearch db verify`, then restore the newest backup with `ownsearch db restore --from <path>`.

## `ownsearch.state_newer`

The state database was written by a newer release. Install that release, or point `OWNSEARCH_STATE_DIR` at another directory.

## `ownsearch.busy`

Too many searches are already waiting. Raise `OWNSEARCH_SEARCH_QUEUE_LIMIT` or `OWNSEARCH_SEARCH_QUEUE_WAIT_SECS` when the machine has headroom, or send fewer concurrent calls.

## The MCP client lists no tools, or marks the server as failed

Use the absolute path to the binary, then run the same command in a terminal to read its error. A server that cannot open its state database exits with the reason on standard error.

## `ownsearch audit verify` says the ledger cannot be read

On a fresh install the file appears after the first tool call. Run a search once and verify again. A chain that cannot be rebuilt is archived with `ownsearch audit rotate --force`.

## `ownsearch.write_disabled`

The database is in read-only AI mode. Set `ai_db_mode = "read-write"` in the profile, or `OWNSEARCH_AI_DB_MODE=read-write`.

## `ownsearch.confirmation_expired`

The one-use token outlived its lifetime. Call the tool again with `dry_run: true`.

## Disk keeps filling up

Lower `OWNSEARCH_KEEP_WINDOW` for the ledger archives, event lines, and pre-migration backups, then run `ownsearch db maintenance` and read the warnings it prints. SQLite rows follow the fixed per-table windows listed in [docs/database.md](database.md).

## A startup error names a setting and its bounds

The value is outside its allowed range. Fix it, or remove it to take the default.

## The process exits with code 75

It stopped making progress for `OWNSEARCH_WATCHDOG_SECS`. Check the logs for the last request, and let the supervisor start it again.

## The browser does not render a page

Set `OWNSEARCH_BROWSER_PATH` when auto-detection misses the Chrome or Chromium executable. Inside a container, the image sets `OWNSEARCH_BROWSER=false`, so no page is rendered there.
