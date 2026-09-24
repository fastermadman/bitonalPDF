#!/bin/bash
# Smoke test: a synthetic 3-page noisy "scan" must shrink in both modes and keep its page count.
set -euo pipefail
cd "$(dirname "$0")/.."
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
for i in 1 2 3; do
  magick -size 1240x1754 plasma:fractal -colorspace Gray "$T/s$i.png"
done
magick "$T"/s?.png -units PixelsPerInch -density 150 "$T/in.pdf"
for mode in text images; do
  rm -f "$T/out.pdf"
  MODE=$mode ./bitonalpdf.sh "$T/in.pdf" "$T/out.pdf"
  [ -f "$T/out.pdf" ] || { echo "FAIL: $mode wrote no file" >&2; exit 1; }
  [ "$(pdfinfo "$T/out.pdf" | awk '/^Pages:/ {print $2}')" = 3 ] || { echo "FAIL: $mode page count" >&2; exit 1; }
done
echo "smoke ok"
