#!/bin/sh
# Runs the gates CI runs.
set -u
cd "$(dirname "$0")/.." || exit 1

fail=0
gate() {
    label=$1
    shift
    if out=$("$@" 2>&1); then
        printf '  ok    %s\n' "$label"
    else
        printf '  FAIL  %s\n' "$label"
        printf '%s\n' "$out" | tail -n 25 | sed 's/^/          /'
        fail=1
    fi
}

gate fmt cargo fmt --all --check
gate clippy cargo clippy --locked --all-targets --quiet -- -D warnings
gate test cargo test --locked --all-targets --quiet
gate doc env RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --quiet
gate ascii ./scripts/check-ascii.sh

[ "$fail" -eq 0 ] && echo "all gates passed" || echo "GATES FAILED"
exit "$fail"
