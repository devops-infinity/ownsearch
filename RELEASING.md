# Releasing OwnSearch

Every step below runs on your machine. Nothing here publishes on its own.

## One-time setup

1. Install the tools the gate needs: `shellcheck`, `shfmt`, `bats`, `gitleaks`, `cargo-audit`, `cargo-deny`, `cargo-machete`, `cargo-llvm-cov`, `dist` (cargo-dist 0.32.0), `minisign`, `zig`, `cargo-zigbuild`, `zip`, `jq`, and `mcp-publisher`.
2. For a local Linux package build, install `musl-tools` and link the compiler name the build looks for: `sudo apt-get install -y musl-tools`, then `sudo ln -sf /usr/bin/musl-gcc /usr/local/bin/x86_64-linux-musl-gcc` and `sudo ln -sf /usr/bin/musl-gcc /usr/local/bin/aarch64-linux-musl-gcc`. Without them the x86_64 musl archive fails at `aws-lc-sys` with `failed to find tool "x86_64-linux-musl-gcc"`.
3. Run `tools/install-hooks.sh` once per checkout so the pre-commit and pre-push hooks use `.githooks`. The pre-commit hook scans the staged diff for secrets, and the pre-push hook runs `tools/verify.sh`.
4. Create the minisign pair once with `minisign -G -p minisign.pub -s ~/.minisign/minisign.key`, then commit `minisign.pub`. The secret key stays out of the repository, and its text goes into the `MINISIGN_KEY` repository secret. Both are required: a release refuses to publish a signature that has no published key behind it.
5. Store registry credentials with your usual tools. `tools/registry-auth.sh` refreshes the GitHub, npm, MCP Registry, and ghcr sessions from the environment and reports which ones are usable, without printing a secret.

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

The script refuses a dirty tree and a version that is not newer than the current one. It stamps the version into the workspace manifest, the binary dependency, `Cargo.lock`, every version field in the MCP Registry manifest, the bundle manifest, and the default in the Dockerfile, then inserts the changelog heading, adds the compare link, and checks every stamped file. A stamp that missed a file stops the script before it finishes. Edit the changelog text, then commit the preparation.

The version number is chosen by the Principal Architect. Do not invent one.

## Package locally

```sh
tools/release.sh --package
```

This runs the strict gate, reads the target list from `dist plan`, and builds every target this host can build. It then builds the global artifacts: the shell and PowerShell installers, `ownsearch.rb` for Homebrew, and the npm package. Everything lands in `dist/`: one archive for each target this host built, `source.tar.gz`, the installers, the formula, the npm package, and the MCPB bundle built from the Linux x86_64 archive. `sha256.sum` covers every archive, the installers, the formula, the npm package, the bundle, and any bill of materials in the directory. It is signed with minisign when `~/.minisign/minisign.key` exists and `minisign.pub` is in the repository. A run that skips a target is refused unless `OWNSEARCH_ALLOW_PARTIAL=1` is set, because the installers and the formula name every target. The host binary is installed into `~/.local/bin`, or into the directory in `OWNSEARCH_INSTALL_DIR`. Nothing is uploaded.

## Tag

```sh
tools/release.sh --tag
```

This requires a clean tree, changelog entries under the current version heading, and a `dist/` packaged for that version. It runs the strict gate, refuses a tag that already exists, and creates `vX.Y.Z` locally: signed when `user.signingkey` is set, and annotated with a warning when it is not. It never pushes. Push the tag only when the Principal Architect orders it. A `v*` tag starts the release workflow in `.github/workflows/release.yml`.

## Edition policy

A new Rust edition moves through `cargo update`, `cargo fix --edition`, the edition field in the workspace manifest, the full gate in `tools/verify.sh`, and `cargo fmt --all`, in one change. Staying on the current edition is a recorded decision, not an omission.

## Semver checks

Install `cargo-semver-checks` once, alongside the other gate tools. Before a tagged release, `tools/verify.sh` runs `cargo semver-checks check-release -p ownsearch-core --baseline-rev <previous tag>`. The checker has three blind spots that a reviewer covers by hand: a changed field or parameter type, a change in generics or lifetimes, and a change visible only under a non-default feature set.

## Deployment and rollback

Every channel installs a versioned artifact: the archives and installers, the crates, the container image, and the npm loader. A bad release is rolled back by `git switch --detach vX.Y.Z` and running `tools/reinstall.sh`, repinning the image to the previous tag, or publishing the previous npm and crate versions again. Switch back with `git switch -` afterward. The database needs no rollback step: migrations run forward only and the pre-migration backup is verified before it is used. Run `tools/smoke.sh` after every deploy; a failure is the trigger to roll back.

## Version, MSRV, yank, and rollback policy

- Before 1.0.0, a minor release may change behavior and a patch release may fix it. The changelog records every change.
- The declared MSRV may rise in a minor release. The changelog names the new floor, and the 1.94.1 gate in `tools/verify.sh` moves with it.
- Yank a crates.io release only for a broken or unsafe publish, and publish the replacement first.
- Roll back a bad binary with `git switch --detach vX.Y.Z`, `tools/reinstall.sh`, then `git switch -`. Roll back state with `ownsearch db restore --from <backup>` against the newest verified backup, then run `ownsearch db verify`.

## Publish, one registry at a time

1. crates.io: `cargo publish -p ownsearch-core --locked`, then `cargo publish -p ownsearch --locked`.
2. npm: `gh release download vX.Y.Z --pattern 'ownsearch-npm-package.tar.gz' --dir dist`, then `mkdir -p target/distrib/ownsearch-npm-package`, then `tar -xzf dist/ownsearch-npm-package.tar.gz -C target/distrib/ownsearch-npm-package --strip-components=1`. Run `tools/npm-auth-setup.sh` once, then `tools/publish-npm.sh`. The script checks the package name and the version against the workspace, injects `mcpName`, publishes, and waits for the registry to answer. `tools/publish-npm.sh --dry-run` packs and prints the file list without publishing.
3. MCP Registry: `mcp-publisher publish` from a schema-valid `server.json`.
4. Homebrew: `gh release download vX.Y.Z --pattern 'ownsearch.rb' --dir dist`. Two fixes are needed before the audit: drop the `version` line, because the URL already carries the version, and give `test` a real assertion, because the generated one is empty. Copy the result to `Formula/ownsearch.rb` in `devops-infinity/homebrew-tap`, run `brew audit --strict devops-infinity/tap/ownsearch`, then commit and push the tap. The release workflow never pushes the tap.
5. ghcr: `docker buildx build --platform linux/amd64 -t ghcr.io/devops-infinity/ownsearch:X.Y.Z -t ghcr.io/devops-infinity/ownsearch:latest --push .`. The image is amd64 only. The first push creates the package as `internal`, and the REST API has no endpoint that changes package visibility, so make it public once by hand: open `https://github.com/orgs/devops-infinity/packages/container/package/ownsearch/settings`, then `Change visibility` under Danger Zone, choose `Public`, and confirm the package name. Public is one way; a public package cannot be made private again.
6. Post-release smoke: start the installed binary, call `ownsearch doctor`, call one tool over stdio, and check `ownsearch audit verify`. A failed smoke check is a rollback.

Credentials live outside the repository. crates.io accepts an API token in `CARGO_REGISTRY_TOKEN`, or an OIDC trusted publisher set up once in the account settings for `ownsearch-core` and `ownsearch`. `tools/registry-auth.sh` reports which registry sessions are usable without printing a secret. Nothing is published from the local machine without the command being typed.

The release workflow builds all seven targets from `[workspace.metadata.dist]` on native runners, including the two musl archives. It attaches every archive, both installers, `ownsearch.rb`, `ownsearch-npm-package.tar.gz`, the MCPB bundle, `sha256.sum` with its signature, and `minisign.pub`. The gate runs on the machine through `tools/release.sh --tag`, which refuses to create a tag it cannot pass. A manual run takes the tag from the workflow input, so the built tree is the tag and not the default branch.

## Boundaries

- No command in this repository commits, pushes, tags a remote, or publishes by itself.
- No GitHub workflow other than the release workflow is added without an explicit order.
- A release does not proceed while any gate fails. A skipped gate needs a written reason and the maintainer's approval.
