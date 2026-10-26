#!/usr/bin/env bash
set -Eeuo pipefail

timestamp() {
	date -u +%H:%M:%S
}

log_line() {
	local level="$1"
	shift
	printf '[%s] %-7s %s\n' "$(timestamp)" "$level" "$*" >&2
}

warn() {
	log_line WARNING "$@"
}

die() {
	log_line FAILED "$@"
	exit 1
}

version_at_least() {
	local have="$1" want="$2" index left right
	local -a have_parts=() want_parts=()
	[[ "$have" =~ ^[0-9]+(\.[0-9]+)*$ && "$want" =~ ^[0-9]+(\.[0-9]+)*$ ]] || return 1
	IFS=. read -r -a have_parts <<<"$have"
	IFS=. read -r -a want_parts <<<"$want"
	for index in 0 1 2; do
		left="${have_parts[index]:-0}"
		right="${want_parts[index]:-0}"
		if ((10#$left > 10#$right)); then
			return 0
		fi
		if ((10#$left < 10#$right)); then
			return 1
		fi
	done
	return 0
}

require_bash() {
	local minimum="$1"
	local current="${BASH_VERSINFO[0]:-0}.${BASH_VERSINFO[1]:-0}"
	if ! version_at_least "$current" "$minimum"; then
		die "Bash $minimum or newer is required"
	fi
}

require_tool() {
	local tool="$1"
	local hint="$2"
	if ! command -v "$tool" >/dev/null 2>&1; then
		die "$tool is missing; $hint"
	fi
}

workspace_version() {
	local manifest="$1/Cargo.toml"
	awk '
		/^\[workspace.package\]/ { in_package = 1; next }
		/^\[/ { in_package = 0 }
		in_package && /^version[[:space:]]*=/ {
			gsub(/[",]/, "", $3)
			print $3
			exit
		}
	' "$manifest"
}

sha256_file() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | awk '{print $1}'
	elif command -v shasum >/dev/null 2>&1; then
		shasum -a 256 "$1" | awk '{print $1}'
	else
		die "no sha256 tool found"
	fi
}

check_shell_syntax() {
	local file
	for file in "$@"; do
		bash -n "$file"
	done
}
