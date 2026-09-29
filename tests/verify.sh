#!/bin/bash
# #68: every check a pipeline change needs, in one go, raw output. Usage: [BASE_BIN=<baseline build>] tests/verify.sh
# BASE_BIN (e.g. a build of main) adds "identical / differs" per file for f1..f7 and tests/real/*.pdf; differs is not a
# failure. Exit 1 if cargo test, synth.sh or real.sh fail. The visual check (checker on the pages) is not part of this.
set -uo pipefail
R=$(cd "$(dirname "$0")/.." && pwd); BIN=$R/rust/target/release/bitonalpdf; T=$(mktemp -d); trap 'rm -rf "$T"' EXIT; rc=0
FLAGS=(--rotate --crop --split auto --deskew)
step() { echo; echo "== $1"; }
step "cargo test --release"; (cd "$R/rust" && cargo build --release -q && cargo test --release 2>&1 | grep -E "^test |test result") || rc=1
step "tests/synth.sh"; BIN=$BIN "$R/tests/synth.sh" || rc=1
step "tests/real.sh (page count, footers, facts)"; BIN=$BIN SIZE_TOL=1000 FACT_TOL=1000 "$R/tests/real.sh" || rc=1
step "tests/slotbox-suite.sh (f1..f7)"; "$R/tests/slotbox-suite.sh" "$T/suite"
if [ -n "${BASE_BIN:-}" ]; then
  step "cmp against BASE_BIN"
  for f in "$R"/tests/real/*.pdf "$R/tests/real/new pdfs dont upload"/*.pdf; do
    n=$(basename "$f" .pdf); "$BIN" "${FLAGS[@]}" "$f" "$T/n.pdf" >/dev/null 2>&1; "$BASE_BIN" "${FLAGS[@]}" "$f" "$T/b.pdf" >/dev/null 2>&1
    if [ ! -f "$T/n.pdf" ] && [ ! -f "$T/b.pdf" ]; then echo "no output  $n (not smaller)"
    elif cmp -s "$T/n.pdf" "$T/b.pdf"; then echo "identical  $n"; else echo "differs    $n"; fi
    rm -f "$T/n.pdf" "$T/b.pdf"
  done
fi
echo; [ $rc -eq 0 ] && echo "verify: ok" || echo "verify: FAILED"; exit $rc
