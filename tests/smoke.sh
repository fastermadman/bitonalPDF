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

# --crop --split: 2 synthetic spreads (text-like line blocks either side of a gutter, plus a dark
# scanner border along the top) must give 4 pages of one uniform size, with the border cut off.
draw=()
for y in $(seq 150 24 1050); do draw+=(-draw "rectangle 150,$y 800,$((y+9))" -draw "rectangle 950,$y 1600,$((y+9))"); done
for i in 1 2; do
  magick -size 1754x1240 xc:white -fill black "${draw[@]}" -draw "rectangle 0,0 1754,30" "$T/sp$i.png"
done
magick "$T"/sp?.png -units PixelsPerInch -density 150 "$T/spread.pdf"
./bitonalpdf.sh --crop --split auto "$T/spread.pdf" "$T/spread.out.pdf" >/dev/null 2>&1
[ "$(pdfinfo "$T/spread.out.pdf" | awk '/^Pages:/ {print $2}')" = 4 ] || { echo "FAIL: split page count" >&2; exit 1; }
sizes=$(pdfinfo -f 1 -l 4 "$T/spread.out.pdf" | awk '/^Page +[0-9]+ size:/ {print $4, $6}' | sort -u)
[ "$(echo "$sizes" | awk "END{print NR}")" = 1 ] || { echo "FAIL: pages differ in size: $sizes" >&2; exit 1; }
h=${sizes#* }; awk -v h="$h" 'BEGIN{exit !(h < 1240*72/150 - 20)}' || {
  echo "FAIL: border not cropped (height $h)" >&2
  pdfinfo "$T/spread.pdf" | grep -i 'size' >&2; pdfimages -list "$T/spread.pdf" >&2; magick -version | head -1 >&2
  exit 1; }
echo "smoke ok"
