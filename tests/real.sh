#!/bin/bash
# Run the pipeline on real scans that must not go into git (copyright): drop PDFs in tests/real/
# and run this locally. For each one it checks: output written, all pages one size, and (optional)
# the page count in tests/real/<name>.pages. Output goes to tests/real/out/ for eyeballing.
# Facts: tests/real/<name>.facts (from tests/measure.sh: size, ink box, footer ink per output page) is
# compared per page when present (size exact, box +-FACT_TOL per-mille, footer ink equal), so a wrong
# crop/split fails even though the file "looks fine". Create/refresh them, after LOOKING at out/, with --record.
# Usage: tests/real.sh [--record] [--rotate --crop --split auto --deskew]   (default flags: all four)
# ONLY=<name> runs just that scan.
# BIN=<script> runs another implementation (default: bitonalpdf.sh in the repo root).
set -uo pipefail
cd "$(dirname "$0")/real" || exit 1
RECORD=0; [ "${1:-}" = --record ] && { RECORD=1; shift; }
FLAGS=("$@"); [ ${#FLAGS[@]} -gt 0 ] || FLAGS=(--rotate --crop --split auto --deskew)
mkdir -p out; fail=0; found=0
for f in *.pdf; do
  [ -f "$f" ] || { echo "no PDFs in tests/real/ — put your scans there" >&2; exit 1; }
  found=1; name=${f%.pdf}; [ -z "${ONLY:-}" ] || [ "$name" = "$ONLY" ] || continue; o="out/$name.pdf"; rm -f "$o"
  start=$(date +%s)
  "${BIN:-../../bitonalpdf.sh}" "${FLAGS[@]}" "$f" "$o" >"out/$name.log" 2>&1; rc=$?
  secs=$(( $(date +%s) - start ))
  # rc 2 = pages flagged for manual check; anything else non-zero is a failure
  # "Not smaller" = the input was already small; correct for a clean PDF, so it counts as a pass
  if grep -q "^Not smaller" "out/$name.log"; then echo "$name: ok (not smaller — no file written)"; continue; fi
  if [ $rc -ne 0 ] && [ $rc -ne 2 ]; then echo "FAIL $name: exit $rc (see out/$name.log)"; fail=1; continue; fi
  [ -f "$o" ] || { echo "FAIL $name: no output written (see out/$name.log)"; fail=1; continue; }
  pin=$(pdfinfo "$f" | awk '/^Pages:/ {print $2}'); pout=$(pdfinfo "$o" | awk '/^Pages:/ {print $2}')
  nsizes=$(pdfinfo -f 1 -l "$pout" "$o" | awk '/^Page +[0-9]+ size:/ {print $4, $6}' | sort -u | awk 'END{print NR}')
  status=ok; [ "$nsizes" = 1 ] || status="FAIL pages differ in size ($nsizes sizes)"
  [ ! -f "$name.pages" ] || [ "$pout" = "$(cat "$name.pages")" ] || status="FAIL expected $(cat "$name.pages") pages"
  if [ $RECORD = 1 ]; then ../measure.sh "$o" > "$name.facts"; status="$status; facts recorded"
  elif [ -f "$name.facts" ]; then
    d=$(../measure.sh "$o" | awk -v tol="${FACT_TOL:-20}" 'NR==FNR {e[$1]=$0; n++; next} { m++
      split(e[$1], a); if ($2 != a[2] || $3 != a[3]) print "page " $1 ": size " $2 "x" $3 " (expected " a[2] "x" a[3] ")"
      else { for (i = 4; i <= 7; i++) if (($i-a[i] > tol) || (a[i]-$i > tol)) { print "page " $1 ": ink box " $4 "," $5 "," $6 "," $7 " (expected " a[4] "," a[5] "," a[6] "," a[7] ")"; break }
             if ($8 != a[8]) print "page " $1 ": footer ink " $8 " (expected " a[8] ")" } }
      END { if (m != n) print "page count " m " (facts say " n ")" }' "$name.facts" - | head -5)
    [ -z "$d" ] || status="FAIL facts differ: $(echo "$d" | paste -sd';' -)"
  else status="$status (no facts file)"; fi
  [ $rc -eq 0 ] || status="$status; needs manual check (exit 2)"
  echo "$name: $pin -> $pout pages, $(du -h "$f" | cut -f1) -> $(du -h "$o" | cut -f1), ${secs}s: $status"
  case $status in FAIL*) fail=1 ;; esac
done
[ $found = 1 ] && exit $fail
