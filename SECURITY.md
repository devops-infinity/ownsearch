# Security Policy

## Reporting a vulnerability

Write to sazzad@devops.bd with the subject line `OwnSearch security`. Include the version you run, the command or tool you called, the smallest reproducer you have, and the impact you expect.

Do not report a security problem through a public issue, pull request, or discussion. If the address above does not answer within the acknowledgement window, open a bare issue that asks for a private contact route and carries no detail.

You get an acknowledgement within three working days. A fix ships in the next release, and a published advisory names the affected versions once the fix is published.

## Supported versions

Version 0.3.x receives security fixes. A release outside that line gets a fix only when the problem can be exploited without special conditions and the fix does not break compatibility.

## What OwnSearch handles

- Public instance URLs, scores, and probe evidence in a local SQLite database, with a regenerated `instances.snapshot.json`, an `instances.events.jsonl` log, and backup copies beside it.
- A hash-chained ledger in the ledger directory: `calls.jsonl`, archived `calls-<stamp>.jsonl` files, and the 32-byte `args.key` that keys the argument hashes and the chain.
- Each ledger row records the call status, the upstream HTTP status class, the tool and operation, a keyed arguments hash, timing, byte counts, result counts, parser, cache state, and instance ids. It never records the query text.
- An HTTP bearer token list, read from the environment or the OS keychain and required for any non-loopback bind. HTTP mode has no TLS listener, so put a TLS proxy in front of any non-loopback bind.
- Text fetched from any HTTP or HTTPS URL: HTML is extracted, and a PDF answer is parsed inside the process with lopdf.
- JavaScript pages rendered in headless Chromium, on unless `OWNSEARCH_BROWSER=false`. Each session uses a per-process profile under the state directory, and `OWNSEARCH_BROWSER_NO_SANDBOX` turns off the Chromium sandbox.
- An optional CapSolver key (`OWNSEARCH_CAPSOLVER_KEY`): when set, the page URL and site key of a rendered challenge go to `api.capsolver.com`, and the key is held as a secret and never printed.
- Search results, treated as untrusted data and sanitized before they reach a client.

OwnSearch stores no credentials for the searched instances, sends no telemetry, and holds no user accounts.

At startup and on each discovery window it fetches `https://searx.space/data/instances.json`, unless offline mode or `OWNSEARCH_DISCOVERY_URL` changes that. Other requests go to an instance you scan or to a URL you pass to a fetch, and the public-address guard is off when `OWNSEARCH_ALLOW_PRIVATE_INSTANCES=true`.

## Scope

In scope:

- The MCP server, both transports.
- The scanner and its network behavior.
- The SQLite state layer and its migrations.
- The typed database change surface.
- The release scripts and packaging.

Out of scope:

- A public SearXNG instance, its operator, its engines, or its availability.
- Content returned by upstream engines.
- A client that chooses to ignore the untrusted-data framing of search results.

## Dependency and release cadence

- Run `tools/verify.sh` before every release and after every dependency change. It runs the full gate, including cargo audit, cargo deny, cargo machete, and a secret scan.
- Recheck the advisory suppressions in `deny.toml` by their stated dates. A suppression past its date fails `tools/verify.sh` on the next run, including the pre-push hook.
- Release builds use the toolchain pinned in `rust-toolchain.toml`, which is the one shipped.

## Known limits

- The operator of a public instance sees your query. This is how public instances work.
- A search snippet can still be written to persuade a model. OwnSearch sanitizes text, caps length, keeps attribution, and never treats result text as an instruction, but it cannot make the text true.
- A bot check, a CAPTCHA, or a rate limit is a control set by the operator. OwnSearch records it and stops. It never works around one.
