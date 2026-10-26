#!/usr/bin/env bash
set -Eeuo pipefail

REPO="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"
source "$REPO/tools/lib.sh"
require_bash 4.4
shopt -s inherit_errexit

PROFILE="release"
INSTALL_DIR="${OWNSEARCH_INSTALL_DIR:-$HOME/.local/bin}"
SKIP_GATE=0
RUN_DOCTOR=0

usage() {
	cat <<'USAGE'
Usage: tools/reinstall.sh [options]

Build OwnSearch and install the binary into your own bin directory.

  --debug          build the debug profile instead of release
  --dir <path>     install into this directory (default ~/.local/bin)
  --skip-gate      install without running tools/verify.sh first
  --doctor         run the installed binary's doctor command afterwards
  -h, --help       show this

Environment:
  OWNSEARCH_INSTALL_DIR   same as --dir
USAGE
}

while [[ $# -gt 0 ]]; do
	case "$1" in
	--debug)
		PROFILE="debug"
		shift
		;;
	--dir)
		[[ $# -ge 2 ]] || die "--dir needs a path"
		INSTALL_DIR="$2"
		shift 2
		;;
	--skip-gate)
		SKIP_GATE=1
		shift
		;;
	--doctor)
		RUN_DOCTOR=1
		shift
		;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		usage
		die "unknown option: $1"
		;;
	esac
done

[[ -n "$INSTALL_DIR" ]] || die "the install directory is empty"
require_tool cargo "install Rust from https://rustup.rs"

if [[ "$SKIP_GATE" -eq 0 ]]; then
	log_line INFO "running the verification gate"
	"$REPO/tools/verify.sh"
fi

log_line INFO "building the $PROFILE binary"
if [[ "$PROFILE" == "release" ]]; then
	cargo build --release --locked --bin ownsearch
	BINARY="$REPO/target/release/ownsearch"
else
	cargo build --locked --bin ownsearch
	BINARY="$REPO/target/debug/ownsearch"
fi

[[ -x "$BINARY" ]] || die "the build did not produce $BINARY"
mkdir -p "$INSTALL_DIR"
INSTALL_TMP="$(mktemp "$INSTALL_DIR/.ownsearch.new.XXXXXX")"
trap 'rm -f -- "${INSTALL_TMP:-}"' EXIT
install -m 0755 "$BINARY" "$INSTALL_TMP"
mv -f -- "$INSTALL_TMP" "$INSTALL_DIR/ownsearch"
log_line SUCCESS "installed $("$INSTALL_DIR/ownsearch" --version) to $INSTALL_DIR/ownsearch"

case ":$PATH:" in
*":$INSTALL_DIR:"*) ;;
*) warn "$INSTALL_DIR is not on PATH; add it to your shell profile" ;;
esac

if [[ "$RUN_DOCTOR" -eq 1 ]]; then
	"$INSTALL_DIR/ownsearch" doctor
fi
