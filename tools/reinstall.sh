#!/usr/bin/env bash
set -Eeuo pipefail

REPO="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"
source "$REPO/tools/lib.sh"
require_bash 4.4

PROFILE="release"
RUN_GATE=true
RUN_DOCTOR=true
TARGET_DIR="${OWNSEARCH_INSTALL_DIR:-$HOME/.local/bin}"

usage() {
	printf 'usage: tools/reinstall.sh [--debug] [--no-gate] [--no-doctor] [--dir DIR]\n'
}

while (($# > 0)); do
	case "$1" in
	--debug) PROFILE="debug" ;;
	--no-gate) RUN_GATE=false ;;
	--no-doctor) RUN_DOCTOR=false ;;
	--dir)
		shift
		TARGET_DIR="${1:?--dir needs a path}"
		;;
	--help | -h)
		usage
		exit 0
		;;
	*) die "unknown argument: $1" ;;
	esac
	shift
done

if $RUN_GATE; then
	"$REPO/tools/verify.sh"
fi

if [[ "$PROFILE" == "release" ]]; then
	cargo build --release --locked --bin ownsearch
	BINARY="$REPO/target/release/ownsearch"
else
	cargo build --locked --bin ownsearch
	BINARY="$REPO/target/debug/ownsearch"
fi

mkdir -p "$TARGET_DIR"
STAGED="$TARGET_DIR/.ownsearch.new"
install -m 0755 "$BINARY" "$STAGED"
mv -f "$STAGED" "$TARGET_DIR/ownsearch"
log_line SUCCESS "installed $TARGET_DIR/ownsearch"

case ":$PATH:" in
*":$TARGET_DIR:"*) ;;
*) warn "$TARGET_DIR is not on PATH; add it to your shell profile" ;;
esac

if $RUN_DOCTOR; then
	"$TARGET_DIR/ownsearch" doctor
fi
