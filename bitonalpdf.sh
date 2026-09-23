#!/bin/bash
# bitonalPDF — shrink scanned PDFs. Needs: brew install poppler imagemagick
#
# Usage: [MODE=text|images] bitonalpdf.sh input.pdf [output.pdf] [threshold%=60] [dpi]
#   MODE=text    (default) 1-bit Group4 at 300 dpi. For pure text scans (typewriter/print):
#                colour and greytones are dropped on purpose. Higher threshold = bolder text
#                (try 55-70). The background is flattened first, so shadows from microfilm or
#                book spines don't wreck the threshold.
#   MODE=images  colour JPEG at 150 dpi (threshold unused): for books with pictures/diagrams.
# Output defaults to "<input>.1bit.pdf" / "<input>.shrunk.pdf"; the original is never touched.
# Both modes rasterise the pages (a scan has no text layer to lose).
# PROGRESS_FILE=<path>: one line per finished page is appended there (used by the droplet).
set -euo pipefail

MODE=${MODE:-text}
IN=${1:?usage: $0 input.pdf [output.pdf] [threshold%] [dpi]}
case $MODE in
  text)   SUFFIX=1bit;   DEFAULT_DPI=300 ;;
  images) SUFFIX=shrunk; DEFAULT_DPI=150 ;;
  *) echo "MODE must be text or images" >&2; exit 1 ;;
esac
OUT=${2:-"${IN%.pdf}.$SUFFIX.pdf"}
THRESH=${3:-60}
DPI=${4:-$DEFAULT_DPI}
PROGRESS_FILE=${PROGRESS_FILE:-/dev/null}
[ "$OUT" != "$IN" ] || { echo "output must differ from input" >&2; exit 1; }

PAGES=$(pdfinfo "$IN" | awk '/^Pages:/ {print $2}')
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

page() {
  local n=$1 p; p=$(printf %04d "$n")
  if [ "$MODE" = text ]; then
    pdftoppm -gray -r "$DPI" -f "$n" -l "$n" -png "$IN" "$TMP/r$p"
    magick "$TMP"/r"$p"-*.png \( +clone -blur 0x30 \) -compose Divide_Dst -composite \
      -threshold "$THRESH%" -type bilevel -units PixelsPerInch -density "$DPI" \
      -compress Group4 "$TMP/p$p.tif"
    rm -f "$TMP"/r"$p"-*.png
  else
    pdftoppm -r "$DPI" -jpeg -jpegopt quality=65 -f "$n" -l "$n" "$IN" "$TMP/p$p"
  fi
  echo "$n" >> "$PROGRESS_FILE"
}
export -f page
export IN TMP MODE THRESH DPI PROGRESS_FILE

echo "Processing $PAGES pages (4 at a time, MODE=$MODE)..."
seq 1 "$PAGES" | xargs -P 4 -I{} bash -c 'page {}'
magick "$TMP"/p*.[tj]* -units PixelsPerInch -density "$DPI" "$OUT"
if [ "$(stat -f %z "$OUT")" -ge "$(stat -f %z "$IN")" ]; then
  rm -f "$OUT"
  echo "Not smaller ($(du -h "$IN" | cut -f1) is already small) — no file written"
  exit 0
fi
echo "$(du -h "$IN" | cut -f1) -> $(du -h "$OUT" | cut -f1): $OUT"
