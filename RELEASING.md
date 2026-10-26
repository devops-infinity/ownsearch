# Releasing OwnSearch

Every step below runs on your machine. Nothing here publishes on its own.

## One-time setup

1. Install the tools the gate needs: `shellcheck`, `shfmt`, `bats`, `gitleaks`, `cargo-audit`, `cargo-deny`, `cargo-machete`, `cargo-llvm-cov`, `dist` (cargo-dist), `minisign`, `zig`, and `mcp-publisher`.
2. Run `tools/install-hooks.sh` once per checkout so the pre-commit and pre-push hooks use `.githooks`.
3. Create a minisign key if you sign releases: `minisign -G -p minisign.pub -s ~/.minisign/ownsearch.key`. Keep the secret key out of the repository.
4. Store registry credentials with your usual tools. `tools/registry-auth.sh` reports which sessions are usable without printing any secret.

## Local checks

```sh
tools/verify.sh
```

The gate runs shell syntax, ShellCheck, shfmt, Bats (when installed), formatting, check, Clippy, tests, doc tests, docs, cargo audit, cargo deny, cargo machete, coverage, and a Gitleaks scan over the tree and history. A skipped check is reported. `OWNSEARCH_STRICT_GATE=1` turns a skip into a failure.

## Prepare a version

```sh
tools/release.sh --prepare --patch
tools/release.sh --prepare --minor
tools/release.sh --prepare --major
tools/release.sh --prepare --version 1.2.3
```

The script refuses a dirty tree, stamps the version into the workspace manifest, the binary dependency, every version field in the MCP Registry manifest, the bundle manifest, and the Dockerfile label, and inserts a changelog heading. Edit the changelog text, then commit the preparation on `main`.

The version number is chosen by the Principal Architect. Do not invent one.

## Package locally

```sh
tools/release.sh --package
```

This runs the strict gate, builds the cargo-dist artifacts for the host, and signs `sha256.sum` with minisign when the key exists. It never uploads anything. The MCPB bundle is built with the `mcpb` tool from the Linux x86_64 binary, and the local binary is installed with `tools/reinstall.sh`.

## Tag

```sh
tools/release.sh --tag
```

This runs the strict gate again and creates a signed tag locally. It fails when no GPG key is configured, because an unsigned release tag is refused. It never pushes. Push the tag only when the Principal Architect orders it. A `v*` tag starts the release workflow in `.github/workflows/release.yml`.

## Edition policy

A new Rust edition moves through `cargo update`, `cargo fix --edition`, the edition field in the workspace manifest, the full gate in `tools/verify.sh`, and `cargo fmt --all`, in one change. Staying on the current edition is a recorded decision, not an omission.

## Semver checks

Install `cargo-semver-checks` once, alongside the other gate tools. Before a tagged release, `tools/verify.sh` runs `cargo semver-checks check-release -p ownsearch-core --baseline-rev <previous tag>`. The checker has three blind spots that a reviewer covers by hand: a changed field or parameter type, a change in generics or lifetimes, and a change visible only under a non-default feature set.

## Deployment and rollback

Every channel installs a versioned artifact: the archives and installers, the crates, the container image, and the npm loader. A bad release is rolled back by reinstalling the previous tag with `tools/reinstall.sh --dir`, repinning the image to the previous tag, or publishing the previous npm and crate versions again. The database needs no rollback step: migrations run forward only and the pre-migration backup is verified before it is used. Run `tools/smoke.sh` after every deploy; a failure is the trigger to roll back.

## Version, MSRV, yank, and rollback policy

- Before 1.0.0, a minor release may change behavior and a patch release may fix it. The changelog records every change.
- The declared MSRV may rise in a minor release. The changelog names the new floor, and the 1.94.1 gate in `tools/verify.sh` moves with it.
- Yank a crates.io release only for a broken or unsafe publish, and publish the replacement first.
- Roll back a bad binary by reinstalling the previous release with `tools/reinstall.sh` against the older tag. Roll back state from the newest verified backup in the state `backups/` directory, then run `ownsearch db verify`.

## Publish, one registry at a time

1. crates.io: `cargo publish -p ownsearch-core --locked`, then `cargo publish -p ownsearch --locked`.
2. npm: `tools/publish-npm.sh` after `tools/npm-auth-setup.sh`. The script checks the package name and version, injects `mcpName`, publishes, and waits for the registry to answer with the version.
3. MCP Registry: `mcp-publisher publish` from a schema-valid `server.json`.
4. Homebrew: build the formula from the release artifacts, run `brew style`, `brew audit --strict`, and `brew test`, then update the tap.
5. ghcr: build the image from the released tag and push both the version tag and `latest`.
6. Post-release smoke: start the installed binary, call `ownsearch doctor`, call one tool over stdio, and check `ownsearch audit verify`. A failed smoke check is a rollback.

Credentials live outside the repository. crates.io accepts an API token in `CARGO_REGISTRY_TOKEN`, or an OIDC trusted publisher set up once in the account settings for `ownsearch-core` and `ownsearch`. `tools/registry-auth.sh` reports which registry sessions are usable without printing a secret. Nothing is published from the local machine without the command being typed.

The release workflow builds the five native targets. The two musl archives in `[workspace.metadata.dist]` are built locally with cargo-zigbuild and added to the release by hand.

## Boundaries

- No command in this repository commits, pushes, tags a remote, or publishes by itself.
- No GitHub workflow other than the release workflow is added without an explicit order.
- A release does not proceed while any gate fails. A skipped gate needs a written reason and the maintainer's approval.
