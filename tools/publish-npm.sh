#!/usr/bin/env bash
set -Eeuo pipefail

REPO="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"
source "$REPO/tools/lib.sh"
require_bash 4.4
require_tool npm "install Node.js and npm"
require_tool jq "brew install jq"

PACKAGE_DIR="${1:-}"
if [[ -z "$PACKAGE_DIR" ]]; then
	PACKAGE_DIR="$(find dist -type f -name package.json -path '*npm*' 2>/dev/null | head -n 1)"
fi
[[ -n "$PACKAGE_DIR" ]] || die "no npm package found; run tools/release.sh --package first or pass a directory"
PACKAGE_DIR="$(cd -- "$(dirname -- "$PACKAGE_DIR")" && pwd)"

NAME="$(jq -r '.name' "$PACKAGE_DIR/package.json")"
VERSION="$(jq -r '.version' "$PACKAGE_DIR/package.json")"
[[ "$NAME" == "@devops-infinity/ownsearch" ]] || die "unexpected package name: $NAME"
[[ -n "$VERSION" && "$VERSION" != "null" ]] || die "the package has no version"

[[ -z "$(git status --porcelain)" ]] || die "the working tree is not clean"

jq --arg name "io.github.devops-infinity/ownsearch" '.mcpName = $name' \
	"$PACKAGE_DIR/package.json" >"$PACKAGE_DIR/package.json.next"
mv "$PACKAGE_DIR/package.json.next" "$PACKAGE_DIR/package.json"

(
	cd "$PACKAGE_DIR"
	npm publish --access public
)

for _ in $(seq 1 12); do
	published="$(npm view "$NAME@$VERSION" version 2>/dev/null || true)"
	if [[ "$published" == "$VERSION" ]]; then
		log_line SUCCESS "$NAME@$VERSION is live"
		exit 0
	fi
	sleep 5
done
die "$NAME@$VERSION did not appear in the registry"
