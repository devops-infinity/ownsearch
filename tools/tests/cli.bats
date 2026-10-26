#!/usr/bin/env bats

setup_file() {
	REPO="$(cd -- "$(dirname -- "$BATS_TEST_FILENAME")/../.." && pwd)"
	export REPO
	export BIN="$REPO/target/release/ownsearch"
	if [[ ! -x "$BIN" ]]; then
		export BIN="$REPO/target/debug/ownsearch"
	fi
	WORK="$(mktemp -d)"
	export WORK
	export OWNSEARCH_STATE_DIR="$WORK/state"
	export OWNSEARCH_LEDGER_DIR="$WORK/ledger"
}

teardown_file() {
	rm -rf "$WORK"
}

setup() {
	if [[ ! -x "$BIN" ]]; then
		skip "build the binary first with cargo build"
	fi
}

@test "the command tree lists every documented command" {
	run "$BIN" --help
	[ "$status" -eq 0 ]
	[[ "$output" == *serve* ]]
	[[ "$output" == *doctor* ]]
	[[ "$output" == *scan* ]]
	[[ "$output" == *instances* ]]
	[[ "$output" == *db* ]]
	[[ "$output" == *config* ]]
	[[ "$output" == *audit* ]]
	[[ "$output" == *completions* ]]
	[[ "$output" == *man* ]]
}

@test "the version flag prints a version" {
	run "$BIN" --version
	[ "$status" -eq 0 ]
	[[ "${output,,}" == ownsearch* ]]
}

@test "the manual page renders" {
	run "$BIN" man
	[ "$status" -eq 0 ]
	[[ "${output,,}" == *ownsearch* ]]
}

@test "completions render for bash" {
	run "$BIN" completions bash
	[ "$status" -eq 0 ]
	[[ "$output" == *ownsearch* ]]
}

@test "db path honours the state directory" {
	run "$BIN" db path
	[ "$status" -eq 0 ]
	[[ "$output" == "$OWNSEARCH_STATE_DIR/instances.db" ]]
}

@test "doctor reports a missing database without creating it" {
	run "$BIN" doctor
	[ "$status" -eq 0 ]
	[[ "$output" == *"does not exist yet"* ]]
	[ ! -f "$OWNSEARCH_STATE_DIR/instances.db" ]
}

@test "an unknown status is refused" {
	run "$BIN" instances list --status nonsense
	[ "$status" -ne 0 ]
}
