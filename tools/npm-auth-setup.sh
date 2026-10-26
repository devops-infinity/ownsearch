#!/usr/bin/env bash
set -Eeuo pipefail

REPO="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"
source "$REPO/tools/lib.sh"
require_bash 4.4
require_tool npm "install Node.js and npm"

NPMRC="${HOME}/.npmrc"

printf 'npm token (input is hidden): '
IFS= read -rs TOKEN
printf '\n'
[[ -n "$TOKEN" ]] || die "no token was entered"

umask 077
if [[ -f "$NPMRC" ]]; then
	cp "$NPMRC" "$NPMRC.bak"
fi
printf '//registry.npmjs.org/:_authToken=%s\n' "$TOKEN" >"$NPMRC"
chmod 600 "$NPMRC"
unset TOKEN

if npm whoami >/dev/null 2>&1; then
	log_line SUCCESS "npm is authenticated as $(npm whoami)"
else
	die "npm could not verify the token"
fi
