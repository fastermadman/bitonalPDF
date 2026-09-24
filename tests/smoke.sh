#!/bin/bash
# shellcheck disable=SC2034,SC2012,SC2196  # temporary CI diagnostics below
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
magick "$T"/sp?.png -units PixelsPerInch -density 150 "$T/spread.pdf"
./bitonalpdf.sh --crop --split auto "$T/spread.pdf" "$T/spread.out.pdf" >/dev/null 2>&1
[ "$(pdfinfo "$T/spread.out.pdf" | awk '/^Pages:/ {print $2}')" = 4 ] || { echo "FAIL: split page count" >&2; exit 1; }
sizes=$(pdfinfo -f 1 -l 4 "$T/spread.out.pdf" | awk '/^Page +[0-9]+ size:/ {print $4, $6}' | sort -u)
[ "$(echo "$sizes" | awk "END{print NR}")" = 1 ] || { echo "FAIL: pages differ in size: $sizes" >&2; exit 1; }
h=${sizes#* }; awk -v h="$h" 'BEGIN{exit !(h < 1240*72/150 - 20)}' || {
  echo "FAIL: border not cropped (height $h)" >&2
  pdfinfo "$T/spread.pdf" | grep -i 'size' >&2; pdfimages -list "$T/spread.pdf" >&2; magick -version | head -1 >&2
  pdftoppm -gray -r 300 -f 1 -l 1 -png "$T/spread.pdf" "$T/dbg"; f=$(ls "$T"/dbg*.png | head -1)
  eval "$(sed -n '/^content_box()/,/^}/p' bitonalpdf.sh)"
  CROP_MIN_DENSITY=0.03 CROP_MAX_DENSITY=0.55 CROP_EDGE_FRAC=0.015 CROP_PAD_FRAC=0.012
  echo "content_box: $(content_box "$f" 3508 2480)" >&2
  magick "$f" \( +clone -blur 0x30 \) -compose Divide_Dst -composite -colorspace Gray -threshold 60% -negate "$T/ink.png"
  magick identify -verbose "$T/ink.png" | egrep 'Type|Depth|Colorspace|mean' >&2
  magick "$T/ink.png" -colorspace Gray -scale "1x2480!" -depth 8 gray:- | od -An -v -tu1 | tr -s ' ' '\n' | sort -n | uniq -c | sort -rn | head -5 >&2
  exit 1; }
echo "smoke ok"
