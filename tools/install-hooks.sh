#!/usr/bin/env bash
set -Eeuo pipefail

REPO="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"
source "$REPO/tools/lib.sh"
require_bash 4.4

remove=false
while (($# > 0)); do
	case "$1" in
	--remove) remove=true ;;
	--help | -h)
		printf 'usage: tools/install-hooks.sh [--remove]\n'
		exit 0
		;;
	*)
		die "unknown argument: $1"
		;;
	esac
	shift
done

if $remove; then
	git config --unset core.hooksPath || true
	log_line SUCCESS "git hooks released"
	exit 0
fi

git config core.hooksPath .githooks
chmod +x .githooks/pre-commit .githooks/pre-push
log_line SUCCESS "git hooks now run from .githooks"
