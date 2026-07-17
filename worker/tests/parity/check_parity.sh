#!/usr/bin/env bash
# W-WP2 ID-parity gate.
#
# Runs a ported prototype binary and diffs its stdout+stderr against a frozen
# OneCAD-CPP corpus recording (corpus/expected-values/<name>.txt, captured at
# commit b4ddcccc). Proves the ported kernel emits byte-identical printed
# IDs/hashes/counts to the C++ oracle.
#
# The corpus files wrap the raw program output in a header block and append an
# "exit_code: N" trailer; we extract the program-output slice (between the
# "=== stdout+stderr (exit code appended) ===" marker and the "exit_code:"
# line) and compare it to a fresh run.
#
# Usage: check_parity.sh <binary> <corpus-file>
set -u

BIN="$1"
CORPUS="$2"

if [[ ! -x "$BIN" ]]; then
    echo "PARITY FAIL: binary not found/executable: $BIN" >&2
    exit 2
fi
if [[ ! -f "$CORPUS" ]]; then
    echo "PARITY FAIL: corpus recording not found: $CORPUS" >&2
    exit 2
fi

# --- Run the ported binary, capture combined output + exit code ---
actual="$("$BIN" 2>&1)"
actual_rc=$?

# --- Extract expected program output + exit code from the corpus recording ---
expected="$(awk '
    /^=== stdout\+stderr \(exit code appended\) ===$/ { grab=1; next }
    /^exit_code: / { print "RC:" $2; grab=0; next }
    grab { print }
' "$CORPUS")"

expected_rc="$(printf '%s\n' "$expected" | sed -n 's/^RC:\([0-9]*\)$/\1/p')"
expected_body="$(printf '%s\n' "$expected" | grep -v '^RC:')"

if [[ "$actual_rc" != "$expected_rc" ]]; then
    echo "PARITY FAIL: exit code $actual_rc != corpus $expected_rc" >&2
    echo "--- actual output ---" >&2
    printf '%s\n' "$actual" >&2
    exit 1
fi

if [[ "$actual" != "$expected_body" ]]; then
    echo "PARITY FAIL: output differs from corpus recording $CORPUS" >&2
    diff <(printf '%s\n' "$expected_body") <(printf '%s\n' "$actual") >&2
    exit 1
fi

echo "PARITY OK: $BIN matches $CORPUS (exit $actual_rc)"
exit 0
