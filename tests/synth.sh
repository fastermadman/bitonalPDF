#!/bin/bash
# Synthetic regression suite: generated fixtures (rectangles and speckle only, CI has no fonts), the
# pipeline run on each, the result *measured* (tests/measure.sh), one pass/fail row per case.
# Usage: [BIN=path/to/implementation] tests/synth.sh     (default BIN: bitonalpdf.sh in the repo root)
# BIN must accept the bitonalpdf.sh CLI (flags, input, output), so a Rust binary plugs in unchanged.
# Rows: PASS / FAIL / KNOWN-FAIL (an open defect that still fails: reported, exit 0) / XPASS (a known
# defect that now passes: exit 1, move it out of the KNOWN list). Exit 1 on any FAIL or XPASS.
set -uo pipefail
cd "$(dirname "$0")/.."
BIN=${BIN:-./bitonalpdf.sh}
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
MEASURE=tests/measure.sh
bad=0

# Speckle block standing in for text. Grey ink (gray15), not pure black: closer to real ink (lessons.md).
magick -seed 1 -size 110x180 xc: +noise Random -colorspace Gray -threshold 70% -scale 500% -fill gray15 -opaque black "$T/blk.png"
pdf() { magick "$1" -compress none -units PixelsPerInch -density 150 "$2"; }  # uncompressed, else the script (rightly) refuses to write a not-smaller file
page() { magick -size 1240x1754 xc:white "$T/blk.png" -geometry +150+150 -composite "$@"; }  # portrait page with one block (x150..800, y150..1050)

# fixtures ---------------------------------------------------------------------------------------
for i in 1 2; do  # spread: block either side of a gutter + dark scanner border along the top
  magick -seed "$i" -size 110x180 xc: +noise Random -colorspace Gray -threshold 70% -scale 500% "$T/b$i.png"
  magick -size 1754x1240 xc:white "$T/b$i.png" -geometry +150+150 -composite "$T/b$i.png" -geometry +1054+150 -composite \
    -fill gray15 -draw "rectangle 0,0 1754,30" "$T/sp$i.png"
done
pdf "$T/sp?.png" "$T/spread.pdf"
page -fill gray15 -draw "rectangle 600,1150 606,1185" -draw "rectangle 620,1150 626,1185" -draw "rectangle 640,1150 646,1185" "$T/pn.png"; pdf "$T/pn.png" "$T/pn.pdf"      # #19 page number
page -fill gray15 -draw "rectangle 40,300 43,900" "$T/bar.png"; pdf "$T/bar.png" "$T/bar.pdf"                                                                            # #23 thin stripe
page -fill gray15 -draw "rectangle 0,0 500,130" "$T/band.png"; pdf "$T/band.png" "$T/band.pdf"                                                                            # #23 wide dark band (open)
magick -size 1754x1240 xc:white \( "$T/blk.png" -resize 1450x200! \) -geometry +150+400 -composite "$T/wide.png"; pdf "$T/wide.png" "$T/wide.pdf"                        # wide table: no gutter, must stay whole
lines=(); for y in 300 500 700 900 1100 1300 1500; do lines+=(-draw "rectangle 200,$y,1040,$((y+5))"); done
magick -size 1240x1754 xc:gray85 -fill gray30 "${lines[@]}" "$T/flat.png"
pdf "$T/flat.png" "$T/flat.pdf"                                                                                                                                            # #22 thin ink on grey paper

# run <name> <known:0|1> <input.pdf> <check> [flags...]. check = awk expression over the measured
# facts; it sees pages n, W/H (pt of page 1), sizes (distinct sizes), x0,y0,x1,y1,foot (page 1) and ink (mean darkness).
run() {
  local name=$1 known=$2 in=$3 check=$4; shift 4
  local out="$T/$name.out.pdf" res detail
  "$BIN" "$@" "$in" "$out" >"$T/$name.log" 2>&1
  if [ ! -f "$out" ]; then res=0; detail="no output written"; else
    $MEASURE "$out" | awk -v ink="$(pdftoppm -r 40 -gray -f 1 -l 1 "$out" | magick pgm:- -format '%[fx:1-mean]' info:)" '
      { n++; if ($1 == 1) { W=$2; H=$3; x0=$4; y0=$5; x1=$6; y1=$7; foot=$8 } s[$2 " " $3] = 1 }
      END { for (k in s) sizes++; exit !('"$check"') }' && res=1 || res=0
    detail="$($MEASURE "$out" | head -2 | paste -sd'|' -)"
  fi
  local row
  case "$res$known" in 10) row=PASS ;; 00) row=FAIL; bad=1 ;; 01) row=KNOWN-FAIL ;; 11) row=XPASS; bad=1 ;; esac
  printf '%-12s %-11s %s\n' "$name" "$row" "$detail"
}

printf '%-12s %-11s %s\n' case result "measured (page[,page 2]: n W H x0 y0 x1 y1 foot; box in per-mille)"
# W,H in pt: 150 dpi px * 72/150
run spread     0 "$T/spread.pdf" 'n == 4 && sizes == 1 && H < 1240*72/150 - 20'   --crop --split auto  # 2 spreads -> 4 pages, top border cropped
run pagenumber 0 "$T/pn.pdf"     'H*150/72 >= 1050'                               --crop               # #19: strokes at y1150..1185 survive
run stripe     0 "$T/bar.pdf"    'W*150/72 < 640'                                 --crop               # #23: thin stripe far from the text does not widen the crop
run wide-table 0 "$T/wide.pdf"   'n == 1'                                         --crop --split auto  # a table wider than tall, no gutter: never split
run flatten    0 "$T/flat.pdf"   'ink > 0.005'                                                         # #22: thin ink on grey paper must survive flattening
run dark-band  1 "$T/band.pdf"   'y0 >= 8'                                         --crop               # #23 open: a wide dark band at the top must not stay in the crop (ink box must not touch the top edge)
exit $bad
