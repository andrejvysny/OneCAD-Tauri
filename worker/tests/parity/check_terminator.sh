#!/usr/bin/env bash
# W-WP3a terminator-parity gate.
#
# WHY NOT full byte-parity (unlike check_parity.sh): the frozen corpus
# recordings for proto_loop_detector and proto_sketch_solver are dominated by Qt
# category logging (`onecad.core.sketch: ...`) carrying RANDOM per-run v4 UUIDs
# (QUuid::createUuid). That output is non-deterministic — not even the original
# C++ binary could reproduce it byte-for-byte on a second run. The corpus
# README (§7) states the meaningful signal is the terminator line + exit 0; the
# numeric DOF/positions/counts are asserted IN-SOURCE (not printed). The ported
# prototypes carry those SAME assert()s, and the worker strips Qt category
# logging (Rust owns logging), so this gate proves parity of the corpus's
# reproducible invariant: the binary passes every assert and reaches the same
# terminal state (terminator line present + identical exit code) as the oracle.
#
# Usage: check_terminator.sh <binary> <corpus-file>
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

# --- Corpus expected exit code ---
expected_rc="$(awk '/^exit_code: /{print $2}' "$CORPUS")"

# --- Corpus terminator: last non-empty program-output line before exit_code: ---
terminator="$(awk '
    /^=== stdout\+stderr \(exit code appended\) ===$/ { grab=1; next }
    /^exit_code: / { grab=0; next }
    grab && NF { last=$0 }
    END { print last }
' "$CORPUS")"

if [[ -z "$terminator" ]]; then
    echo "PARITY FAIL: could not extract terminator from corpus $CORPUS" >&2
    exit 2
fi

if [[ "$actual_rc" != "$expected_rc" ]]; then
    echo "PARITY FAIL: exit code $actual_rc != corpus $expected_rc" >&2
    printf '%s\n' "$actual" >&2
    exit 1
fi

if ! grep -qF -- "$terminator" <<<"$actual"; then
    echo "PARITY FAIL: terminator not found in output" >&2
    echo "expected terminator: $terminator" >&2
    echo "--- actual output ---" >&2
    printf '%s\n' "$actual" >&2
    exit 1
fi

echo "PARITY OK: $BIN reached corpus terminator \"$terminator\" (exit $actual_rc)"
exit 0
