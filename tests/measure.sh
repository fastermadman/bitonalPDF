#!/bin/bash
# Print the measurable facts of a PDF, one line per page, so "ok" can mean something:
#   <page> <width pt> <height pt> <x0> <y0> <x1> <y1> <foot>
# x0..y1 = bounding box of all ink in per-mille of the page (what the crop really left);
# foot = 1 if any ink sits in the bottom 12 % (page number / footer present).
# Rendered at 40 dpi with a lenient threshold so thin strokes (page numbers) still count.
# Usage: tests/measure.sh file.pdf
set -euo pipefail
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
pdftoppm -r 40 -gray "$1" "$T/p"
pdfinfo -f 1 -l 100000 "$1" | awk '/^Page +[0-9]+ size:/ {print $2+0, $4, $6}' | sort -n > "$T/sizes"
for f in "$T"/p-*.pgm; do
  n=$((10#$(basename "$f" .pgm | sed 's/^p-//')))
  read -r w h < <(magick "$f" -format '%w %h\n' info:)
  read -r bw bh bx by < <(magick "$f" -threshold 92% -format '%@\n' info: 2>/dev/null | tr 'x+' '  ')
  foot=$(magick "$f" -gravity South -crop "100%x12%+0+0" +repage -threshold 92% -format '%[fx:minima<1?1:0]' info:)
  awk -v n="$n" -v w="$w" -v h="$h" -v bw="${bw:-0}" -v bh="${bh:-0}" -v bx="${bx:-0}" -v by="${by:-0}" -v foot="$foot" \
      -v pt="$(awk -v n="$n" '$1==n {print $2, $3}' "$T/sizes")" 'BEGIN {
    printf "%d %s %d %d %d %d %d\n", n, pt, 1000*bx/w, 1000*by/h, 1000*(bx+bw)/w, 1000*(by+bh)/h, foot }'
done | sort -n
