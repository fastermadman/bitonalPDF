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

# --crop --split: 2 synthetic spreads (speckle blocks standing in for text, either side of a gutter,
# plus a dark scanner border along the top) must give 4 pages of one uniform size, border cut off.
for i in 1 2; do
  magick -seed "$i" -size 110x180 xc: +noise Random -colorspace Gray -threshold 70% -scale 500% "$T/blk.png"
  magick -size 1754x1240 xc:white "$T/blk.png" -geometry +150+150 -composite \
    "$T/blk.png" -geometry +1054+150 -composite -fill black -draw "rectangle 0,0 1754,30" "$T/sp$i.png"
done
magick "$T"/sp?.png -compress none -units PixelsPerInch -density 150 "$T/spread.pdf"  # uncompressed, else the script (rightly) refuses to write a not-smaller file
./bitonalpdf.sh --crop --split auto "$T/spread.pdf" "$T/spread.out.pdf" >"$T/spread.log" 2>&1 || true
[ -f "$T/spread.out.pdf" ] || { cat "$T/spread.log" >&2; echo "FAIL: no spread output" >&2; exit 1; }
[ "$(pdfinfo "$T/spread.out.pdf" | awk '/^Pages:/ {print $2}')" = 4 ] || { echo "FAIL: split page count" >&2; exit 1; }
sizes=$(pdfinfo -f 1 -l 4 "$T/spread.out.pdf" | awk '/^Page +[0-9]+ size:/ {print $4, $6}' | sort -u)
[ "$(echo "$sizes" | awk "END{print NR}")" = 1 ] || { echo "FAIL: pages differ in size: $sizes" >&2; exit 1; }
h=${sizes#* }; awk -v h="$h" 'BEGIN{exit !(h < 1240*72/150 - 20)}' || { echo "FAIL: border not cropped (height $h)" >&2; exit 1; }
echo "smoke ok"
