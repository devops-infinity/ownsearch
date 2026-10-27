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

sha256_of() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | awk '{print $1}'
	elif command -v shasum >/dev/null 2>&1; then
		shasum -a 256 "$1" | awk '{print $1}'
	else
		die "no sha256 tool found"
	fi
}

sha256_line_of() {
	local file="$1"
	printf '%s  %s\n' "$(sha256_of "$file")" "$(basename "$file")"
}

stamp_version() {
	local root="$1" from="$2" to="$3"
	local manifest="$root/Cargo.toml"
	[[ -f "$manifest" ]] || die "missing $manifest"
	sed -i.bak "s/^version = \"$from\"$/version = \"$to\"/" "$manifest"
	sed -i.bak -E "s|^(ownsearch-core = \{ path = \"crates/ownsearch-core\", version = \")[0-9][0-9.]*(\" \})$|\1$to\2|" "$manifest"
	rm -f "$manifest.bak"
}

verify_version_stamps() {
	local root="$1" want="$2" mismatch=0 file actual package
	actual="$(workspace_version "$root")"
	if [[ "$actual" != "$want" ]]; then
		warn "Cargo.toml reports $actual"
		mismatch=1
	fi
	for file in server.json mcpb/manifest.json; do
		[[ -f "$root/$file" ]] || continue
		actual="$(jq -r .version "$root/$file")"
		if [[ "$actual" != "$want" ]]; then
			warn "$file reports $actual"
			mismatch=1
		fi
	done
	if [[ -f "$root/Dockerfile" ]] && ! grep -qF "ARG OWNSEARCH_VERSION=$want" "$root/Dockerfile"; then
		warn "Dockerfile does not default OWNSEARCH_VERSION to $want"
		mismatch=1
	fi
	if ! grep -qF "ownsearch-core = { path = \"crates/ownsearch-core\", version = \"$want\" }" "$root/Cargo.toml"; then
		warn "Cargo.toml does not require ownsearch-core $want"
		mismatch=1
	fi
	if [[ -f "$root/Cargo.lock" ]]; then
		for package in ownsearch ownsearch-core; do
			if ! grep -A1 -F "name = \"$package\"" "$root/Cargo.lock" | grep -qF "version = \"$want\""; then
				warn "Cargo.lock does not report $want for $package"
				mismatch=1
			fi
		done
	fi
	[[ "$mismatch" -eq 0 ]] || die "the version stamp did not reach every file for $want"
}

insert_changelog_heading() {
	local version="$1" date="$2" tmp heading
	heading="## [$version] - $date"
	tmp="$(mktemp "$(dirname -- CHANGELOG.md)/.stamp.XXXXXX")"
	awk -v heading="$heading" '
		!done && /^## \[Unreleased\]/ {
			print "## [Unreleased]"
			print ""
			print heading
			done=1
			next
		}
		!done && /^## \[[0-9]/ { print heading; print ""; done=1 }
		{ print }
		END { if (!done) { print ""; print heading; print "" } }
	' CHANGELOG.md >"$tmp" || {
		rm -f "$tmp"
		die "could not rewrite CHANGELOG.md"
	}
	mv "$tmp" CHANGELOG.md
}

add_changelog_link() {
	local new="$1" previous="$2" url
	url="https://github.com/devops-infinity/ownsearch/compare/v$previous...v$new"
	if grep -qF "[$new]: $url" CHANGELOG.md; then
		return 0
	fi
	printf '\n[%s]: %s\n' "$new" "$url" >>CHANGELOG.md
}

require_packaged_dist() {
	local version="$1" dir="$2"
	[[ -f "$dir/sha256.sum" ]] || die "missing $dir/sha256.sum; run tools/release.sh --package first"
	[[ -f "$dir/ownsearch-$version.mcpb" ]] || die "missing $dir/ownsearch-$version.mcpb; the packaged files do not belong to $version"
}

host_target() {
	rustc -vV | awk '/^host:/{print $2}'
}

check_shell_syntax() {
	local file
	for file in "$@"; do
		bash -n "$file" || die "syntax error in $file"
	done
}
