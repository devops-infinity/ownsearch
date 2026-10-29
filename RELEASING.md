# Releasing OwnSearch

How a release of OwnSearch is prepared, built, tagged, and published. The version is prepared on your machine, and the release build runs on GitHub's native runners. For incident handling and rollback, see [docs/operations.md](docs/operations.md).

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

### Publishing and optional packaging tools

Publishing needs `mcp-publisher`. The optional local build with `tools/release.sh --package` also needs `cargo-dist`, `minisign`, `zig`, `cargo-zigbuild`, `zip`, and `jq`. A normal release built by the workflow does not need them.

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

## Cut a release

The release is prepared on this machine, and the release build runs on GitHub's native runners. Pushing a `v*` tag starts `.github/workflows/release.yml`, which builds every target on its own runner and creates the GitHub release. Building the archives locally is optional.

### 1. Prepare the version on a branch

```sh
git switch main && git pull
git switch -c release/prepare-X-Y-Z
tools/release.sh --prepare --minor
```

Use `--patch`, `--major`, or `--version X.Y.Z` instead of `--minor` as needed. The script refuses a dirty tree and a version that is not newer than the current one. It stamps the version into `Cargo.toml`, the `ownsearch-core` requirement, `Cargo.lock`, `server.json`, `mcpb/manifest.json`, the `Dockerfile`, and the README status line, moves the `[Unreleased]` notes under the new `## [X.Y.Z] - DATE` heading, and adds the compare link while updating the `[Unreleased]` link. A stamp that misses a file stops the script before it finishes.

### 2. Write the changelog, then commit and merge

Edit the changelog text under the new heading, then commit on the release branch, push it, and open a pull request into `main`.

```sh
git add -A && git commit -m "chore(release): prepare X.Y.Z"
git push -u origin release/prepare-X-Y-Z
gh pr create --fill --base main
```

Merge the pull request before tagging.

### 3. Tag the merged commit and push the tag

```sh
git switch main && git pull
git tag -a vX.Y.Z -m "OwnSearch X.Y.Z"
git push origin vX.Y.Z
```

A `v*` tag starts the release workflow in `.github/workflows/release.yml`. That workflow is the only publisher of the GitHub release. It takes the version from the tag, builds all seven targets from `[workspace.metadata.dist]`, cross-builds both musl targets on glibc runners with `musl-tools`, and attaches every archive, both installers, `ownsearch.rb`, `ownsearch-npm-package.tar.gz`, the MCPB bundle, `sha256.sum` with its signature, and `minisign.pub`. Each target builds on its own native runner, so the build runs in parallel and needs no cross linker here.

A manual workflow run takes the tag from its input, so the built tree is the tag and not the default branch. Every action in the workflow is pinned to a commit, so a moved tag cannot change what runs. Wait for the release to appear before publishing the npm package, because two of the publish steps download its assets.

### Optional: build the archives on this machine

`tools/release.sh --package` builds the archives, installers, formula, npm package, and MCPB bundle under `dist/`, and installs the host binary into `~/.local/bin`. Run it on macOS to include the two Darwin targets, and on Windows for the Windows target; a Linux host skips them and refuses a partial set unless `OWNSEARCH_ALLOW_PARTIAL=1` is set. `tools/release.sh --tag` then creates the tag from that `dist/`. This path is slower than the tag and push above, because one machine builds every target in turn, so a normal release does not use it. `sha256.sum` covers every artifact, and it is signed with minisign when `~/.minisign/minisign.key` exists and `minisign.pub` is in the repository.

## Publish after the release exists

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

## Update host installs

Every machine that runs an older `ownsearch` needs updating to the new version before it reports or serves on the new build. On a machine with a source checkout, check out the tag and reinstall:

```sh
git switch --detach vX.Y.Z
tools/reinstall.sh
git switch -
```

`tools/reinstall.sh` builds the release binary, runs the gate, and installs into `~/.local/bin`, or into the directory from `--dir PATH` or `OWNSEARCH_INSTALL_DIR`. Add `--skip-gate` to install without the gate, and `--doctor` to run the doctor command right after.

On a channel install, update through that channel instead: `brew upgrade ownsearch`, `npm install -g @devops-infinity/ownsearch`, `cargo install --locked ownsearch`, or rerun the shell installer from the release page. A machine with no checkout can also install the published artifact directly and check it against the release `.sha256` before moving it onto `PATH`.

## Post-release check

On a host updated to the new version, run these:

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
