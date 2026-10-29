# Troubleshooting

Each entry names the message you see and the step that clears it. For a running service, the operator path is in [docs/operations.md](operations.md).

## `ownsearch.no_fleet`

Run `ownsearch scan`, then check `ownsearch instances list`. Before any scan has scored the fleet, a search falls back to scanned candidates that passed every hard gate and says so in `warnings`.

## Every search returns no results and `sources.failed` says `ownsearch.policy_refused`

The selected instances deny JSON and HTML. Run from a network the instances accept, or check `ownsearch instances list` for quarantined records.

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
