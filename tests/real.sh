#!/bin/bash
# Run the pipeline on real scans that must not go into git (copyright): drop PDFs in tests/real/
# and run this locally. For each one it checks: output written, all pages one size, and (optional)
# the page count in tests/real/<name>.pages. Output goes to tests/real/out/ for eyeballing.
# Usage: tests/real.sh [--rotate --crop --split auto --deskew]   (default: all four)
set -uo pipefail
cd "$(dirname "$0")/real" || exit 1
FLAGS=("$@"); [ ${#FLAGS[@]} -gt 0 ] || FLAGS=(--rotate --crop --split auto --deskew)
mkdir -p out; fail=0; found=0
for f in *.pdf; do
  [ -f "$f" ] || { echo "no PDFs in tests/real/ — put your scans there" >&2; exit 1; }
  found=1; name=${f%.pdf}; o="out/$name.pdf"; rm -f "$o"
  start=$(date +%s)
  ../../bitonalpdf.sh "${FLAGS[@]}" "$f" "$o" >"out/$name.log" 2>&1; rc=$?
  secs=$(( $(date +%s) - start ))
  # rc 2 = pages flagged for manual check; anything else non-zero is a failure
  if [ $rc -ne 0 ] && [ $rc -ne 2 ]; then echo "FAIL $name: exit $rc (see out/$name.log)"; fail=1; continue; fi
  [ -f "$o" ] || { echo "FAIL $name: no output written (see out/$name.log)"; fail=1; continue; }
  pin=$(pdfinfo "$f" | awk '/^Pages:/ {print $2}'); pout=$(pdfinfo "$o" | awk '/^Pages:/ {print $2}')
  nsizes=$(pdfinfo -f 1 -l "$pout" "$o" | awk '/^Page +[0-9]+ size:/ {print $4, $6}' | sort -u | awk 'END{print NR}')
  status=ok; [ "$nsizes" = 1 ] || status="FAIL pages differ in size ($nsizes sizes)"
  [ ! -f "$name.pages" ] || [ "$pout" = "$(cat "$name.pages")" ] || status="FAIL expected $(cat "$name.pages") pages"
  [ $rc -eq 0 ] || status="$status; needs manual check (exit 2)"
  echo "$name: $pin -> $pout pages, $(du -h "$f" | cut -f1) -> $(du -h "$o" | cut -f1), ${secs}s: $status"
  case $status in FAIL*) fail=1 ;; esac
done
[ $found = 1 ] && exit $fail
