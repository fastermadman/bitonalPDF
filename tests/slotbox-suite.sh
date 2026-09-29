#!/bin/bash
# #55: run f1..f7 (tests/real/new pdfs dont upload/, alphabetical) with all four flags and print tests/page-metrics.py per file.
# Usage: tests/slotbox-suite.sh <outdir> [env assignments...]   e.g. tests/slotbox-suite.sh /tmp/new BITONAL_SLOTBOX=1
# Binary: rust/target/release/bitonalpdf (build first). Outputs <outdir>/fN.pdf, .facts, .bands.
set -u
R=$(cd "$(dirname "$0")/.." && pwd); O=$1; shift; mkdir -p "$O"; i=0
for f in "$R/tests/real/new pdfs dont upload"/*.pdf; do i=$((i+1))
  env "$@" "$R/rust/target/release/bitonalpdf" --rotate --crop --split auto --deskew "$f" "$O/f$i.pdf" >/dev/null 2>"$O/f$i.log"
  "$R/tests/measure.sh" "$O/f$i.pdf" > "$O/f$i.facts"
  python3 "$R/tests/edge-bands.py" "$O/f$i.pdf" > "$O/f$i.bands"
  python3 "$R/tests/page-metrics.py" "f$i" "$O/f$i.facts" "$O/f$i.bands"
done
