# Database and local files

What OwnSearch stores on disk, the commands that manage it, how an instance moves through its lifecycle, and how a typed change reaches the database.

## Local files

- Database: `${XDG_STATE_HOME}/ownsearch/instances.db`, or `OWNSEARCH_STATE_DIR/instances.db`.
- Snapshot: `instances.snapshot.json` beside it, regenerated after each discovery run, each scan, each AI change, and the bootstrap import.
- Discovery history: `instances.events.jsonl`, trimmed to the keep window.
- Backups: `backups/instances-<stamp>.db`, written with `VACUUM INTO`, verified with a quick check and a foreign key check, and removed after the keep window like every other temporary file.
- Pre-migration backups: `pre-migration-<stamp>.db`, removed after the keep window.
- Browser profiles: `browser/profile-<pid>` under the state directory, owner-only, removed when the session closes, and reaped at startup when the process that owned one is gone.
- Ledger: `calls.jsonl`, archived to `calls-<stamp>.jsonl` at the keep boundary, and archives older than the window are removed.
- Arguments key: `args.key` beside the ledger. It keys the argument hashes and the ledger chain, so it is kept.

The state directory is created with mode 0700 and files with mode 0600. WAL mode adds `instances.db-wal` and `instances.db-shm`.

## Schema

The database is SQLite in WAL mode with foreign keys on, STRICT tables, and a checksummed migration ledger. The schema is built from embedded migrations that run forward at open, in order, from `0001_initial.sql` to the newest file in `crates/ownsearch-core/src/db/migrations/`. A pending migration takes a verified `VACUUM INTO` backup first. A database written by a newer release is refused with `ownsearch.state_newer` rather than read partially, and the remedy is to run that newer release.

Tables: `schema_migrations`, `application_metadata`, `discovery_sources`, `discovery_runs`, `instances`, `capability_snapshots`, `engines`, `categories`, `capability_engines`, `capability_categories`, `probes`, `tls_observations`, `hourly_probe_metrics`, `instance_quality`, `engine_stats`, `instance_auth`, `instance_events`, `fleet_snapshots`, `instance_policy_overrides`, and `maintenance_runs`.

Row keep windows, applied by `ownsearch db maintenance`: probes 90 days, TLS observations 90 days, capability snapshots 90 days, hourly metrics 180 days, fleet snapshots 30 days, discovery runs 180 days, lifecycle events 365 days. Instance rows are never deleted by the scanner.

## Commands

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

## Integrity and recovery

`ownsearch db verify` checks the file's integrity and foreign keys and prints the schema version. The store also checks each migration checksum when it opens. A backup is written with `VACUUM INTO` and verified before it is used. `ownsearch db restore --from <backup>` stages and verifies the backup, moves the live database and its WAL sidecars aside, installs the backup, and rechecks it, so a corrupt store has a working recovery command.

## Your own instance

`instance_auth` holds one credential per instance as a kind, an optional header name or basic user, and the environment variable name that holds the value. The value is never stored. `instances.manual_priority` orders an instance before its score, and `instances.json_throttled_until` holds the window in which a limiter refuses JSON.

- Run a self-hosted instance with `ownsearch instances add http://192.168.110.26:8080/ --pin --trusted`, with `OWNSEARCH_ALLOW_PRIVATE_INSTANCES=true` and `OWNSEARCH_ALLOW_PLAIN_HTTP_PRIVATE=true` set for every command and for the server. The instance starts as a candidate. A scan probes it like any other instance, and two passing probes promote it to active, after which searches use it. A loopback or private host skips the TLS and certificate gate, because those checks describe a public host. The address rule still applies: keep the instance on a network you control, since the same setting lets a hostile discovery document point the scanner at a local service.

## Instance lifecycle

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

Remove it from the fleet, which retires rather than deletes the row:

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
4. An expired token, a changed preview, a replayed token, a raw SQL field, an unknown field, or read-only mode all return a stable error with a remedy, and every refusal is written to the event history.

The tool can create or remove a policy override, update lifecycle and policy fields, retire an instance, or restore a retired or missing instance. It cannot delete an instance row, rewrite probes, or forge events. Setting `status` to `active` requires a passing hard gate.
