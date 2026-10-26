#!/usr/bin/env bash
set -Eeuo pipefail

REPO="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"
source "$REPO/tools/lib.sh"
require_bash 4.4

MODE=""
VERSION=""
BUMP=""

usage() {
	printf 'usage: tools/release.sh --prepare (--patch|--minor|--major|--version X.Y.Z) | --package | --tag\n'
}

while (($# > 0)); do
	case "$1" in
	--prepare)
		MODE="prepare"
		;;
	--package)
		MODE="package"
		;;
	--tag)
		MODE="tag"
		;;
	--patch | --minor | --major)
		BUMP="${1#--}"
		;;
	--version)
		shift
		VERSION="${1:?--version needs a number}"
		;;
	--help | -h)
		usage
		exit 0
		;;
	*) die "unknown argument: $1" ;;
	esac
	shift
done

[[ -n "$MODE" ]] || {
	usage
	exit 1
}
require_tool git "install git"
[[ -z "$(git status --porcelain)" ]] || die "the working tree is not clean"

CURRENT="$(workspace_version "$REPO")"
[[ -n "$CURRENT" ]] || die "cannot read the workspace version"

next_version() {
	if [[ -n "$VERSION" ]]; then
		printf '%s' "$VERSION"
		return
	fi
	local major minor patch
	IFS=. read -r major minor patch <<<"$CURRENT"
	case "$BUMP" in
	patch) patch=$((patch + 1)) ;;
	minor)
		minor=$((minor + 1))
		patch=0
		;;
	major)
		major=$((major + 1))
		minor=0
		patch=0
		;;
	esac
	printf '%s.%s.%s' "$major" "$minor" "$patch"
}

stamp_version() {
	local version="$1"
	sed -i.bak -E "s/^version = \"[0-9]+\.[0-9]+\.[0-9]+\"/version = \"$version\"/" Cargo.toml
	sed -i.bak -E "s|ownsearch-core = \{ path = \"../ownsearch-core\", version = \"[0-9]+\.[0-9]+\.[0-9]+\" \}|ownsearch-core = { path = \"../ownsearch-core\", version = \"$version\" }|" crates/ownsearch/Cargo.toml
	sed -i.bak -E "s/\"version\": \"[0-9]+\.[0-9]+\.[0-9]+\"/\"version\": \"$version\"/g" server.json
	sed -i.bak -E "s/\"version\": \"[0-9]+\.[0-9]+\.[0-9]+\"/\"version\": \"$version\"/g" mcpb/manifest.json
	sed -i.bak -E "s/^ARG OWNSEARCH_VERSION=.*/ARG OWNSEARCH_VERSION=$version/" Dockerfile
	rm -f Cargo.toml.bak crates/ownsearch/Cargo.toml.bak server.json.bak mcpb/manifest.json.bak Dockerfile.bak
	cargo update --workspace --offline --quiet
}

insert_changelog() {
	local version="$1"
	local date_today
	date_today="$(date -u +%Y-%m-%d)"
	if grep -q "^## \\[$version\\]" CHANGELOG.md; then
		warn "CHANGELOG.md already holds a heading for $version"
		return
	fi
	local heading
	heading="## [$version] - $date_today"
	awk -v heading="$heading" '
		BEGIN { inserted = 0 }
		/^## \[Unreleased\]/ && !inserted {
			print
			print ""
			print heading
			print ""
			inserted = 1
			next
		}
		{ print }
	' CHANGELOG.md >CHANGELOG.md.next
	mv CHANGELOG.md.next CHANGELOG.md
}

case "$MODE" in
prepare)
	NEW="$(next_version)"
	[[ "$NEW" != "$CURRENT" ]] || die "the new version is the current version"
	stamp_version "$NEW"
	insert_changelog "$NEW"
	log_line SUCCESS "prepared version $NEW; edit CHANGELOG.md, then commit the change"
	;;
package)
	require_tool dist "run: cargo install cargo-dist --locked"
	OWNSEARCH_STRICT_GATE=1 "$REPO/tools/verify.sh"
	dist build --artifacts=local
	if command -v minisign >/dev/null 2>&1 && [[ -f "$HOME/.minisign/ownsearch.key" ]]; then
		(
			cd dist
			shasum -a 256 ./*.tar.gz >sha256.sum 2>/dev/null || true
			minisign -S -s "$HOME/.minisign/ownsearch.key" -m sha256.sum
		)
		log_line SUCCESS "signed sha256.sum"
	else
		warn "minisign key or tool is missing, so checksums were not signed"
	fi
	log_line SUCCESS "local packages built under dist/"
	;;
tag)
	OWNSEARCH_STRICT_GATE=1 "$REPO/tools/verify.sh"
	git tag -s "v$CURRENT" -m "OwnSearch $CURRENT" || die "a signed tag is required; configure a GPG key and retry"
	log_line SUCCESS "created tag v$CURRENT locally; it is not pushed"
	;;
esac
