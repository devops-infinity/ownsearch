# Releasing OwnSearch

How a release of OwnSearch is prepared, built, tagged, and published. Every step runs on a machine you control, and no step publishes on its own. For incident handling and rollback, see [docs/operations.md](docs/operations.md).

## One-time setup

### Gate tools

The gate in `tools/verify.sh` needs these on `PATH`:

- `cargo`, with the Rust 1.94.1 toolchain installed through `rustup`.
- `shellcheck`, `shfmt`, `bats`, `gitleaks`, `cargo-audit`, `cargo-deny`, `cargo-machete`, `cargo-llvm-cov`, `cargo-nextest`, `cargo-semver-checks`, and `python3`.
- `musl-tools`, plus the two compiler names the musl targets look for:

```sh
sudo apt-get install -y musl-tools
sudo ln -sf /usr/bin/musl-gcc /usr/local/bin/x86_64-linux-musl-gcc
sudo ln -sf /usr/bin/musl-gcc /usr/local/bin/aarch64-linux-musl-gcc
```

Without the symlinks, the x86_64 musl archive fails at `aws-lc-sys` with `failed to find tool "x86_64-linux-musl-gcc"`.

### Packaging tools

`tools/release.sh --package` needs `cargo-dist`, `minisign`, `zig`, `cargo-zigbuild`, `zip`, and `jq`. Publishing needs `mcp-publisher`.

### Hooks

Run `tools/install-hooks.sh` once per checkout. The pre-commit hook scans the staged diff for secrets, and the pre-push hook runs `tools/verify.sh`.

### Signing key

Create the minisign pair once, and commit the public half:

```sh
minisign -G -p minisign.pub -s ~/.minisign/minisign.key
```

The secret key stays out of the repository. Its text goes into the `MINISIGN_KEY` repository secret, which the release workflow uses. A release refuses to publish a signature that has no published key behind it.

### Registry credentials

Store these outside the repository. `tools/registry-auth.sh` reports which sessions are usable and never prints a secret.

- GitHub CLI and ghcr: `GITHUB_TOKEN`, or an existing `gh auth login`.
- MCP Registry: `MCP_GITHUB_TOKEN`, else `GITHUB_PAT`, else `GITHUB_PERSONAL_ACCESS_TOKEN`, with `read:org`.
- npm: `NPM_TOKEN`, or run `tools/npm-auth-setup.sh` once.
- crates.io: `CARGO_REGISTRY_TOKEN`.

## Run the gate

```sh
tools/verify.sh
```

The gate runs these checks, in this order:

- Shell syntax over `tools/*.sh` and both hooks.
- ShellCheck, the shell tests through Bats, and `shfmt -d`.
- `cargo fmt --all -- --check`, `cargo check`, and Clippy with `-D warnings`.
- The workspace tests, then the same suite on Rust 1.94.1 for the MSRV floor.
- Doc tests, then `cargo doc` with `RUSTDOCFLAGS="-D warnings"`.
- `cargo audit`, `cargo deny check`, and the suppression recheck dates in `deny.toml`.
- `cargo machete` for unused dependencies, then line coverage against the 80 percent floor.
- The version stamped into `server.json` and `mcpb/manifest.json`.
- `cargo semver-checks check-release -p ownsearch-core` against the last tag.
- A Gitleaks scan over the tree and over the commit history.

A tool that is not installed is skipped and named. `OWNSEARCH_STRICT_GATE=1` turns a skip into a failure, which is what a release runs under.

## Prepare a version

```sh
tools/release.sh --prepare --patch
tools/release.sh --prepare --minor
tools/release.sh --prepare --major
tools/release.sh --prepare --version 1.2.3
```

The script refuses a dirty tree and a version that is not newer than the current one. It stamps the version into `Cargo.toml`, the `ownsearch-core` requirement, `Cargo.lock`, `server.json`, `mcpb/manifest.json`, and the default in the `Dockerfile`, then inserts the changelog heading and adds the compare link. A stamp that misses a file stops the script before it finishes.

Edit the changelog text, commit the preparation, and merge it into `main`.

## Package locally

```sh
tools/release.sh --package
```

`--package` needs a clean tree and changelog entries under the current version. It runs the strict gate, reads the target list from `dist plan`, and builds every target this host supports. Run it on macOS to include the two Darwin targets, and on Windows for the Windows target. A target that needs another host is skipped, and a skipped target is refused unless `OWNSEARCH_ALLOW_PARTIAL=1` is set, because the installers and the formula name every target.

Everything lands in `dist/`: one archive per target this host built, `source.tar.gz`, the shell and PowerShell installers, `ownsearch.rb`, the npm package, and the MCPB bundle built from the Linux x86_64 archive. `sha256.sum` covers every archive, the installers, the formula, the npm package, the bundle, and any bill of materials in the directory, and it is signed with minisign when `~/.minisign/minisign.key` exists and `minisign.pub` is in the repository.

The host binary is installed into `~/.local/bin`, or into the directory in `OWNSEARCH_INSTALL_DIR`. Nothing is uploaded.

## Tag

```sh
tools/release.sh --tag
```

`--tag` needs a clean tree, changelog entries under the current version heading, and a `dist/` packaged for that version. It runs the strict gate, refuses a tag that already exists, and creates `vX.Y.Z` at `HEAD`. The tag is signed when `user.signingkey` is set, and annotated with a warning when it is not. The script never pushes.

## Push the tag

```sh
git push origin vX.Y.Z
```

A `v*` tag starts the release workflow in `.github/workflows/release.yml`. That workflow is the only publisher of the GitHub release. It takes the version from the tag, builds all seven targets from `[workspace.metadata.dist]`, cross-builds both musl targets on glibc runners with `musl-tools`, and attaches every archive, both installers, `ownsearch.rb`, `ownsearch-npm-package.tar.gz`, the MCPB bundle, `sha256.sum` with its signature, and `minisign.pub`.

A manual workflow run takes the tag from its input, so the built tree is the tag and not the default branch. Every action in the workflow is pinned to a commit, so a moved tag cannot change what runs. Wait for the release to appear before publishing the npm package, because two of the publish steps download its assets.

## Publish, one registry at a time

### crates.io

```sh
cargo publish -p ownsearch-core --locked
cargo publish -p ownsearch --locked
```

### npm

```sh
gh release download vX.Y.Z --pattern 'ownsearch-npm-package.tar.gz' --dir dist
mkdir -p target/distrib/ownsearch-npm-package
tar -xzf dist/ownsearch-npm-package.tar.gz -C target/distrib/ownsearch-npm-package --strip-components=1
tools/publish-npm.sh
```

The script checks the package name and version against the workspace, injects `mcpName`, publishes, and waits for the registry to answer. `tools/publish-npm.sh --dry-run` packs and prints the file list without publishing.

### MCP Registry

```sh
mcp-publisher publish
```

Run it from the repository root, against the `server.json` the gate already checked.

### Homebrew tap

```sh
gh release download vX.Y.Z --pattern 'ownsearch.rb' --dir dist
```

The generated formula has no `test do` block, so add one with a real assertion. Then copy the file to `Formula/ownsearch.rb` in the `devops-infinity/homebrew-tap` repository, run `brew audit --strict devops-infinity/tap/ownsearch`, and commit and push that repository. The release workflow never pushes the tap.

### ghcr

```sh
docker buildx build --platform linux/amd64 \
  -t ghcr.io/devops-infinity/ownsearch:X.Y.Z \
  -t ghcr.io/devops-infinity/ownsearch:latest --push .
```

The image is amd64 only. The first push creates the package as `internal`, and no API endpoint changes that visibility, so set it once by hand in the package settings under `Change visibility`. Public is one way: a public package cannot be made private again.

## Post-release check

```sh
ownsearch doctor
ownsearch serve
ownsearch audit verify
```

Start the installed binary, call one tool over stdio, and run `ownsearch audit verify`. A failed check is a rollback. On a served instance, `tools/smoke.sh http://127.0.0.1:8774` covers the HTTP endpoints and needs a bearer token in the environment.

## Roll back

Every channel installs a versioned artifact, so a rollback repoints that channel at the previous version:

- A binary install: `git switch --detach vX.Y.Z`, then `tools/reinstall.sh`, then `git switch -`.
- A container: repin the image to the previous tag.
- npm and crates.io: publish the previous version again.

The database needs no rollback step. Migrations run forward only, and a pending migration takes a verified backup first. [docs/operations.md](docs/operations.md) holds the incident path for a bad release.

## Release policy

- Before 1.0.0, a minor release may change behavior and a patch release may fix it. The changelog records every change.
- The declared MSRV may rise in a minor release. The changelog names the new floor, and the 1.94.1 check in `tools/verify.sh` moves with it.
- Yank a crates.io release only for a broken or unsafe publish, and publish the replacement first.
- No script in this repository creates a remote tag or pushes one. The tag push is a command you type.
