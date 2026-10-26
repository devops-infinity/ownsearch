#!/usr/bin/env bash
set -Eeuo pipefail

REPO="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"
source "$REPO/tools/lib.sh"
require_bash 4.4

cargo_credentials_present() {
	local file="${CARGO_HOME:-$HOME/.cargo}/credentials.toml"
	[[ -s "$file" ]] && grep -q 'token' "$file"
}

check() {
	local name="$1"
	shift
	if "$@" >/dev/null 2>&1; then
		log_line SUCCESS "$name"
	else
		warn "$name is not ready"
	fi
}

check "GitHub CLI" gh auth status
check "npm" npm whoami
check "crates.io" cargo_credentials_present
check "MCP Registry" mcp-publisher --help
check "Docker" docker info
check "Homebrew" brew --version
