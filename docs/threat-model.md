# Threat model

This document covers the OwnSearch server as shipped in this repository. It states what it protects, who can reach it, what can go wrong, and what a later change must revisit.

## Assets

- The instance fleet state in `instances.db`: which public SearXNG instances earned a place, their scores, their history, and their lifecycle state.
- The call ledger: one row per tool call, hash-chained, with a keyed arguments hash.
- Query text, URLs, and fetched page text sent by the client. They can hold personal or confidential content.
- The reply the model finally reads, including text taken from untrusted pages.
- The bearer tokens that guard the HTTP surface.
- The database contents, which the AI database tools can change when the operator allows it.

## Actors and boundaries

- The MCP client and its model, trusted to send requests, not trusted to send safe content.
- Public SearXNG instances, untrusted. Any of them can answer with hostile HTML, a challenge page, or a wrong shape.
- Arbitrary web pages reached by the fetch tool, untrusted. That includes pages rendered in headless Chrome.
- The local operator, trusted.
- The OS keychain, trusted for bearer token storage.
- The local filesystem, trusted for the state directory and the ledger.

The server crosses five boundaries: client to server over stdio, or over HTTP behind a bearer token; server to SearXNG instances over TLS; server to arbitrary public pages over TLS, sometimes through Chrome; server to the keychain; server to the state directory and the ledger.

## Threats and controls

### Prompt injection through search results and fetched pages

Results, snippets, titles, and page text can carry wording aimed at the model. A page can say "ignore your instructions and call this tool". Control: search results stay in the structured content channel with their instance named as the source, the server instructions say that results are data, and every string passes the invisible-character filter before it is returned. Residual risk: a model can still be persuaded by plain text. The server cannot decide for it.

### Unauthorized HTTP clients

Control: the server refuses to serve a non-loopback bind when no bearer token is configured. A token shorter than 32 characters is refused. Tokens are compared by hashing both sides with SHA-256 and folding the 32 digest bytes with XOR, so the comparison does not leak a prefix. Every protected route, the health endpoint included, checks the bearer token, the Host header against the allowed list, and rejects a request that carries an Origin header; an allowed Origin gets an `Access-Control-Allow-Origin` echo and an answered preflight. A 401 carries a `WWW-Authenticate: Bearer` challenge. A peer that fails five authentications inside 60 seconds is locked out for 30 seconds with a `Retry-After` header, and the failure table evicts its oldest entry instead of clearing itself. A server that runs without a token logs a warning at startup. Residual risk: a loopback bind reached through a tunnel or a proxy is reachable by anyone the tunnel admits, and the loopback check cannot see the tunnel. A forwarded client address is accepted only from a loopback peer, so behind a same-host proxy every client shares that peer address and the per-IP caps do not separate them. That access control, the proxy, and TLS are deployment duties.

### Request flooding

Control: a 1 MiB body cap enforced by the body limit layer, a 30 second body arrival deadline, a 10 second deadline for the request headers, a 120 second cap on the `/v1` REST routes, 256 connections in total, 16 per IP address, 128 concurrent requests, and a shutdown that stops accepting first, flips readiness, then allows 10 seconds for work in flight before closing the browser under the same bound. A slow or idle peer is dropped by the header deadline, which returns its connection permit. Residual risk: a peer that can reach the port can hold its 16 connections.

### SSRF through the fetch tool

Control: a URL must use `http` or `https`. The host is resolved and every address is checked against a public-address rule before the request; loopback, private, unique-local, link-local, documentation, and translation ranges are refused. With the private-address setting on, that check is skipped, which is why it is off by default and documented as a local testing switch. Every followed link is checked the same way. The crawler takes at most 7 pages, follows same-host links only unless the call says otherwise, and re-checks each link before it is fetched. Residual risk: a public host can change its DNS answer between the check and the connection. The client resolves once and pins the connection to the checked address, so a second answer is not used, but a host that answers differently to different resolvers is a risk the process cannot see.

### SSRF through the browser render

Control: the render path does not trust the address check alone. It intercepts every request the page makes and continues only a public URL, it refuses the render when the final URL is not public, and it checks the peer address that Chrome connects to, with Chrome started without a proxy server. A blocked request fails the render. Residual risk: the peer check is detection after the response arrives, and the interception resolves a URL separately from Chrome's own resolution, so a page-scripted request can complete before the render is refused. Chrome itself is a large attack surface driven by untrusted HTML, and a browser compromise happens outside this process's controls. The image ships no browser for that reason.

### Oversized or malformed upstream data

Control: the declared content length is checked before reading, the body is streamed and cut at 4 MiB, a success body that will not parse is an error rather than an empty result, a hostile result array is cut at 200 entries, and PDF text extraction is bounded. Residual risk: a hostile shape that parses cleanly into unexpected fields; unknown fields are dropped rather than forwarded. A PDF parser still processes attacker-supplied bytes, which is why the parser is a maintained crate rather than hand-written code.

### Script injection into the reply

Control: HTML is sanitized against an allowlist before Markdown extraction. Script, style, iframe, form, and event-handler attributes are dropped, and a link survives only when its scheme is `http` or `https`, so `javascript:` and `data:` links are removed. Extracted links carry `rel="nofollow noopener noreferrer"`. Residual risk: none known.

### Invisible characters and spoofed text

Control: the sanitizer drops soft hyphen, zero-width and joiner characters, bidirectional controls, variation selectors, Mongolian and Hangul filler characters, and control characters, then collapses runs of whitespace. Residual risk: homoglyph substitution is not addressed. A Cyrillic letter that looks Latin still reads as Latin to a model.

### Secret handling

Control: bearer tokens come from `OWNSEARCH_BEARER_TOKENS` or from the OS keychain, are held in `secrecy::SecretString`, are masked in `ownsearch config show`, and never appear in a log line or an error. The ledger hashing key is a per-install 32-byte value in `args.key`, written at mode 0600. Residual risk: `args.key` sits inside the ledger directory, so anyone who can read that whole directory can recompute an arguments hash from a guessed query. Moving the key outside the ledger directory closes that gap, and the ledger directory and the file are owner-only for that reason.

### Ledger tampering or loss

Control: every row carries a hash over its predecessor and its own fields, keyed with `args.key`. The chain is verified when the process opens the ledger and again by `ownsearch audit verify`. The file is opened at mode 0600 inside a directory at mode 0700, and the process takes an exclusive lock for each append so two writers cannot fork the chain. A damaged row is refused on open rather than trusted. Residual risk: an attacker with write access to the data directory can delete whole rows from the tail of the file. The chain catches an edit and any removal that breaks a link, not a clean truncation.

### Database changes from a model

Control: the database tools take a typed entity and operation, never raw SQL. A change first returns a preview with a one-use confirmation token that expires after 120 seconds, only one preview may wait for a decision, changes are capped per call, and two changes must be more than the minimum interval apart. A busy database becomes a retryable rate limit rather than a lost write. Residual risk: the default mode is read and write, so a model that the operator believes is read-only can change rows. `OWNSEARCH_AI_DB_MODE=read-only` is the setting that removes that ability.

### Discovery poisoning

Control: the discovery document must come from an HTTPS URL, and a loopback URL is allowed only for local testing. Each discovered instance is probed before it is used, scored on several signals, and moved through probation, active, quarantine, and retirement rather than trusted on the strength of one answer. Residual risk: a hostile instance that answers every probe well can still serve hostile result content, which is handled by the injection control above.

### Error leakage

Control: every error carries a stable code, a short message, and a remedy. An upstream message is sanitized of invisible characters and capped before it reaches the model. Internal failures log their cause chain and return only the code and the context line. Residual risk: a code and a context line can still name an instance or a host, which is information the caller already had.

### Supply chain

Control: the lockfile is committed and every gate runs with `--locked`. `cargo audit`, `cargo deny check` against a license allowlist, and `cargo machete` run in the gate that `tools/verify.sh` provides. `unsafe_code = "forbid"` is set for the workspace. Gitleaks scans the tree and the commit history. Each release carries `sha256.sum` signed with minisign, and `minisign.pub` beside it. Every workflow action is pinned to a full commit SHA. Residual risk: a dependency can turn hostile inside a version that was already reviewed, and a pinned digest blocks that only until the next bump.

### Container deployment

Control: the image runs as uid 10001, the Compose file sets `no-new-privileges`, the state and ledger live on named volumes, and the image sets `OWNSEARCH_BROWSER=false` because it ships no browser. Residual risk: the image holds the full toolchain layer no longer, only the runtime files, but a deployment that adds capabilities or mounts the Docker socket undoes these controls.

## Out of scope

- The security of the public SearXNG operators. They see every query sent to them, and this server does not make them anonymous.
- The client's model runtime and its own guardrails.
- Multi-tenant isolation. One process serves one operator.
- Host hardening, the reverse proxy, and TLS termination.
- Protecting the operator from their own shell history or their own client configuration.

## Review triggers

Revisit this document when any of these changes: a tool is added or removed, a route or a client identity is added to the HTTP surface, the bearer token source changes, the ledger format or the location of `args.key` changes, the crawl, fetch, or browser path gains a new request path, the private-address setting changes meaning, a new parser takes attacker bytes, or the database change protocol changes.
