#!/usr/bin/env bash
set -Eeuo pipefail

REPO="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${1:-http://127.0.0.1:8774}"
source "$REPO/tools/lib.sh"
require_bash 4.4
require_tool curl "install curl to run the smoke test"

TOKEN_HEADER=()
if [[ -n "${OWNSEARCH_BEARER_TOKENS:-}" ]]; then
	TOKEN_HEADER=(-H "Authorization: Bearer ${OWNSEARCH_BEARER_TOKENS%%,*}")
elif [[ -n "${OWNSEARCH_BEARER_TOKEN:-}" ]]; then
	TOKEN_HEADER=(-H "Authorization: Bearer ${OWNSEARCH_BEARER_TOKEN}")
fi

check() {
	local name="$1"
	shift
	if "$@" >/dev/null 2>&1; then
		log_line SUCCESS "$name"
	else
		die "$name failed against $BASE"
	fi
}

check "healthz" curl -fsS "$BASE/healthz"
check "startupz" curl -fsS "${TOKEN_HEADER[@]}" "$BASE/startupz"
check "readyz" curl -fsS "${TOKEN_HEADER[@]}" "$BASE/readyz"
check "metrics" curl -fsS "${TOKEN_HEADER[@]}" "$BASE/metrics"
search_body="$(mktemp)"
trap 'rm -f "$search_body"' EXIT
curl -sS -X POST "$BASE/v1/search" \
	"${TOKEN_HEADER[@]}" \
	-H 'content-type: application/json' \
	-d '{"query":"ownsearch smoke","mode":"fast","timeout_secs":5}' >"$search_body"
python3 - "$search_body" <<'PYJSON'
import json
import sys
with open(sys.argv[1]) as handle:
    body = json.load(handle)
if "results" not in body and "code" not in body:
    raise SystemExit("the search answer is neither a result nor a refusal")
PYJSON
log_line SUCCESS "search"
log_line SUCCESS "smoke test passed against $BASE"
