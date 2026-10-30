# OwnSearch

Self-hosted MCP and HTTP search over a scored fleet of public SearXNG instances.

Status: released, version 0.5.0.

`mcp-name: io.github.devops-infinity/ownsearch` is the identifier the MCP Registry reads when it verifies this repository.

## Contents

- Getting started: [What OwnSearch is](#what-ownsearch-is), [Who it is for](#who-it-is-for), [What you get](#what-you-get), [What it does not do](#what-it-does-not-do), [Requirements](#requirements), [Install](#install), [Quick start](#quick-start), [Connect an MCP client](#connect-an-mcp-client), [Your first search](#your-first-search)
- Using it: [Fetch a page](#fetch-a-page), [HTTP mode](#http-mode), [Container](#container), [Configuration](#configuration), [Tools](#tools)
- About: [Documentation](#documentation), [Privacy](#privacy), [Security](#security), [Support](#support), [Contributing](#contributing), [License](#license)

## What OwnSearch is

OwnSearch gives an AI client web search without a paid search API. It finds public SearXNG instances, tests them, ranks them, and answers each query from the healthiest ones. When an instance refuses or fails, the same search continues on the next instance, and every result keeps the name of the instance that supplied it.

Two terms come up throughout this page.

- SearXNG is a free meta-search engine that queries other search engines and merges their answers. Volunteers run public instances of it.
- The Model Context Protocol (MCP) is the interface an AI client uses to call a tool. OwnSearch is one such tool server.

OwnSearch runs on your machine. It stores its fleet in a local SQLite database, keeps a hash-chained ledger of calls, and sends nothing to a service of ours, because there is no such service.

## Who it is for

- Developers building an AI assistant or agent that needs web search, and who do not want to buy a search API key.
- Teams that want search results with a named source for each result, so an answer can be traced back to the instance that produced it.
- Operators who need page fetching, JavaScript rendering, and page-to-Markdown extraction in the same local server.
- Anyone running a self-hosted setup who wants the fleet health, the scoring, and the call ledger to stay under their own control.

## What you get

- A fleet of public SearXNG instances, discovered from `searx.space`, probed, scored, and kept in local state.
- Search with failover: a failing instance cools down and the search moves to the next one in score order.
- Result attribution: each result names its source instance, and, when known, the engine.
- Page fetch as Markdown, text, or cleaned HTML. An optional headless browser covers JavaScript pages, and the fetch follows links on the same host.
- Eight MCP tools, two REST endpoints, and a command line for setup, scanning, fleet changes, and database work.
- A local SQLite database with a typed change path, a verified backup and restore, and a hash-chained ledger of every call.

## What it does not do

- It does not host or operate a SearXNG instance.
- It does not call a paid search API.
- It does not make your queries anonymous. The operator of each public instance sees them.
- It does not run JavaScript on a search answer. The headless browser runs for the fetch tool, on the page you name. It also runs for a search when an instance denies JSON, as a fallback that proves the instance can still answer.
- It passes the header checks a SearXNG limiter runs, because every request carries a browser-like `Accept` header. A CAPTCHA or a challenge page is still not solved, and OwnSearch records it; with `OWNSEARCH_CAPSOLVER_KEY` set, a challenge that appears only in a rendered page can be sent to the CapSolver service.

## Requirements

- Rust 1.94.1 or newer to build. The repository pins Rust 1.99.1 in `rust-toolchain.toml`, and the gate builds against 1.94.1 as the minimum.
- Linux or macOS. A Windows binary is part of the release target list, and the MCP bundle ships for macOS and Linux.
- Outbound HTTPS for discovery, probes, and searches.
- A C compiler for the bundled SQLite build: `build-essential` on Debian and Ubuntu, or the Command Line Tools on macOS.
- A Chrome or Chromium executable for JavaScript rendering. Set `OWNSEARCH_BROWSER_PATH` when auto-detection misses it.

## Install

Pick one channel. Each installs the same `ownsearch` binary.

Homebrew, on Linux or macOS:

```sh
brew trust devops-infinity/tap
brew install devops-infinity/tap/ownsearch
```

npm, which installs the platform binary through a small loader:

```sh
npm install -g @devops-infinity/ownsearch
```

Cargo:

```sh
cargo install --locked ownsearch
```

The shell installer from the release page:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://github.com/devops-infinity/ownsearch/releases/latest/download/ownsearch-installer.sh | sh
```

Check the result:

```sh
ownsearch --version
```

Keep one channel only. Two installs of the same binary on your `PATH` make it unclear which one your MCP client starts.

### Update an install

```sh
ownsearch update --check     # report whether a newer release is published
ownsearch update             # download, verify, and install it
```

The updater fetches the release from GitHub, verifies `sha256.sum` against the minisign key built into the binary, checks the archive against the digest in that signed file, and replaces the running binary in place. It runs only when you call it. There is no background check and no automatic replacement.

`--check` works on every install and reports the published version, plus the command that installs it. A full update refuses when another tool owns the install and names that command: `brew upgrade ownsearch` for Homebrew, `cargo install --locked ownsearch` for cargo, `npm install -g @devops-infinity/ownsearch` for npm, and pulling the newer image for a container. Only an install that came from the shell installer or a plain binary copy updates itself.

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

Five steps take you from a fresh install to a search answered from the fleet.

1. Check the resolved settings and the state.

   ```sh
   ownsearch doctor
   ```

2. Create the state and score the fleet. The first command that reads the database creates it, applies the schema, and seeds an embedded fleet of 67 instances.

   ```sh
   ownsearch scan
   ```

   A scan probes each candidate over the network. `--limit` caps how many instances a run probes, and `--dry-run` reports what would change without writing state.

3. Look at what the scan found.

   ```sh
   ownsearch instances list
   ```

   Instances start as candidates. Two passing probes promote one to active, so a second scan after `OWNSEARCH_SCAN_MIN_INTERVAL_SECS` (30 seconds by default) moves the repeat winners into the fleet.

   ```sh
   ownsearch scan
   ownsearch instances list
   ```

   A scan inside that interval is refused with the remaining wait. `ownsearch scan --force` runs it now.

4. Start the MCP server. Your client starts it for you once it is configured, and running it by hand shows its errors.

   ```sh
   ownsearch serve
   ```

5. Connect a client and run a search. The next section covers the client entry.

Search has no terminal command. It runs through an MCP client, through the two REST endpoints, or through the tools a client calls. The CLI covers setup, scanning, fleet changes, and database work.

The state lives in `${XDG_STATE_HOME}/ownsearch` by default, which is `~/.local/state/ownsearch` on most machines. Point every command and every MCP client at the same directory if you move it, with `OWNSEARCH_STATE_DIR` or `--state-dir`.

A dry run changes nothing and prints what would happen:

```sh
ownsearch scan --dry-run --limit 12
```

Searches use the active fleet. When no instance is active, OwnSearch falls back to scanned candidates that passed every hard gate and says so in `warnings`.

## Connect an MCP client

Every MCP client starts a server process. Point it at the installed binary with `serve` as the only argument.

Claude Code:

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

After you add the entry, the client lists eight tools: `ownsearch_search`, `ownsearch_fetch`, `ownsearch_instance_info`, `ownsearch_instance_health`, `ownsearch_scan`, `ownsearch_session`, `ownsearch_db_query`, and `ownsearch_db_crud`.

OwnSearch holds three secrets at most: the HTTP bearer tokens, used only when you serve Streamable HTTP; an optional CapSolver key, used only when you turn on the paid captcha service; and an optional outbound proxy URL. None is read from a flag or the profile. All three come from the environment, and the bearer tokens can also live in the OS keychain.

Over Streamable HTTP, point the client at `http://127.0.0.1:8774/mcp`, or at the same host with no path. Both serve the MCP endpoint, and any other path answers 404 with a problem body. The port also answers `/healthz`, `/startupz`, `/readyz`, and `/metrics`. Send `Authorization: Bearer <token>` once `OWNSEARCH_BEARER_TOKENS` is set. The REST calls `POST /v1/search` and `POST /v1/fetch` take the tool arguments as the JSON body.

## Your first search

Ask your client to call `ownsearch_search` with a query:

```json
{ "query": "rust programming language", "limit": 5 }
```

The answer carries the results, the mode that was used, and a `sources` block. Each source names the instance that supplied results, and `sources.failed` names any instance that refused. A result keeps its title, URL, snippet, engine, and published date according to the `fields` preset.

`fast` targets one instance, `balanced` targets three, and `exhaustive` targets `OWNSEARCH_EXHAUSTIVE_MAX`. The default mode is `balanced`.

## Fetch a page

`ownsearch_fetch` returns a public page as Markdown by default:

```json
{ "url": "https://example.org/article", "max_pages": 3 }
```

It refuses private and loopback addresses, follows same-host links up to `max_pages`, and reports `finalUrl` plus the redirect chain. `render: "always"` sends the page through headless Chrome, and `render: "auto"` does that only for a page that looks like an empty JavaScript shell.

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
- `GET /healthz` is liveness, `GET /startupz` checks the database, `GET /readyz` reports readiness, and `GET /metrics` reports call counters, per-tool counts, and per-route HTTP counts.
- Shutdown flips readiness first, then drains in-flight calls under a deadline.

A REST call:

```sh
curl -s http://127.0.0.1:8774/v1/search \
  -H 'content-type: application/json' \
  -d '{"query":"rust programming language","limit":5}'
```

A successful call answers `200` with the structured content the MCP tool returns. A refused call answers a status between `400` and `504` with an RFC 9457 `application/problem+json` body that carries `type`, `title`, `status`, `detail`, `instance`, and the stable `code` and `remedy` as extension members. Every REST call is recorded in the ledger and counted in `/metrics` exactly like an MCP call.

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

`docker-compose.yml` holds the same setup with `restart: unless-stopped`, both state volumes, and a healthcheck. The image sets `OWNSEARCH_BIND=0.0.0.0:8774`, a ledger directory under `/home/ownsearch/.local/share/ownsearch/ledger`, and `OWNSEARCH_BROWSER=false`, so a container renders no page. `OWNSEARCH_BEARER_TOKENS` is required at run time and the server refuses to start without it. Add `OWNSEARCH_ALLOWED_HOSTS` with the names clients use, or the Host check refuses every request that does not name `localhost` or the bind address.

## Configuration

Settings resolve in this order: CLI flags, `OWNSEARCH_*` environment variables, a TOML profile at `${XDG_CONFIG_HOME}/ownsearch/profile.toml`, then built-in defaults. Most environment variables map to a lowercase profile key, so `OWNSEARCH_FLEET_MAX` is `fleet_max`.

[docs/configuration.md](docs/configuration.md) lists every setting with its default and its bounds, shows a profile example, and covers the paths, the keep windows, and the AI database mode.

## Tools

Eight tools, described in full in [docs/tools.md](docs/tools.md):

- `ownsearch_search`: federated web search across the fleet.
- `ownsearch_fetch`: one page, or a few same-host pages, as Markdown, text, or HTML.
- `ownsearch_instance_info` and `ownsearch_instance_health`: what one instance can do and how it is doing.
- `ownsearch_scan`: refresh discovery, probe, rescore, and reselect the fleet. It defaults to a dry run.
- `ownsearch_session`: local counts, cache state, ledger head, and fleet size.
- `ownsearch_db_query` and `ownsearch_db_crud`: bounded reads and typed changes over the local database.

## Documentation

- [docs/configuration.md](docs/configuration.md): every setting, its default, and its bounds.
- [docs/tools.md](docs/tools.md): the tool arguments, search behavior, and the quality model.
- [docs/database.md](docs/database.md): the schema, the commands, the instance lifecycle, and the local files.
- [docs/scanner-policy.md](docs/scanner-policy.md): how instances are contacted, scored, cooled down, and kept.
- [docs/operations.md](docs/operations.md): service levels, detection signals, and the triage path for each failure.
- [docs/troubleshooting.md](docs/troubleshooting.md): the messages you are likely to hit and the step that clears each one.
- [RELEASING.md](RELEASING.md): the release procedure.
- [SECURITY.md](SECURITY.md): how to report a vulnerability.

## Privacy

Queries go to the public instances you selected and to the engines behind them. OwnSearch stores no raw query text. The ledger records a keyed hash of the arguments, the tool name, the status, the upstream HTTP status class, timing, byte counts, parser, and instance ids. There is no telemetry.

One more destination exists, and only when you set it: with `OWNSEARCH_CAPSOLVER_KEY` set, a detected captcha sends the page URL and the site key to `https://api.capsolver.com` to solve the challenge. With `OWNSEARCH_PROXY` set, every outbound HTTP request goes through the proxy you name. A per-instance credential is stored as an environment variable name and sent only to that instance's own origin, never to another host. Nothing else leaves the machine.

[SECURITY.md](SECURITY.md) lists everything OwnSearch stores and every host it contacts.

## Security

- No `unsafe` code anywhere in the workspace.
- Discovery records and fetch targets are limited to public addresses, so a hostile document cannot point the scanner at a local or internal service.
- rustls with strict certificate validation on every outbound connection.
- Bounded response reads, connection limits, and request deadlines.
- Search results are data. Every string passes a sanitizer that removes invisible and control characters, URLs must be HTTP or HTTPS, and long values are cut with a visible marker.
- No secret is accepted on a command line, written to the profile, or printed by any command.

Report a vulnerability privately; [SECURITY.md](SECURITY.md) states how.

## Support

Write to sazzad@devops.bd for a question, a bug report, or an install problem. Include the version, the command you ran, and the output. Security problems follow [SECURITY.md](SECURITY.md), not this address's normal queue.

Most failures have a written remedy in [docs/troubleshooting.md](docs/troubleshooting.md), and a running service has an operator path in [docs/operations.md](docs/operations.md).

## Contributing

OwnSearch is maintained by Md. Sazzad Hossain Sharkar. Send a patch or a proposal to sazzad@devops.bd before starting large work, and run `tools/verify.sh` before sending a change. A change lands when the full gate passes; [RELEASING.md](RELEASING.md) lists what the gate runs.

## License

MIT OR Apache-2.0, at your choice. The full texts are in [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE). The HTML parser (`scraper`) brings MPL-2.0 components (`cssparser`, `selectors`, `dtoa-short`), used as unmodified dependencies; MPL-2.0 permits that use inside a larger work under another license.
