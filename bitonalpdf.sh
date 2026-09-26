#!/bin/bash
# bitonalPDF — shrink scanned PDFs. Needs: brew install poppler imagemagick
# Optional (only for --rotate): brew install tesseract
#
# Usage: [MODE=text|images] bitonalpdf.sh [options] input.pdf [output.pdf] [threshold%=60] [dpi]
#   MODE=text    (default) 1-bit Group4 at 300 dpi. For pure text scans (typewriter/print):
#                colour and greytones are dropped on purpose. Higher threshold = bolder text
#                (try 55-70). The background is flattened first, so shadows from microfilm or
#                book spines don't wreck the threshold.
#   MODE=images  colour JPEG at 150 dpi (threshold unused): for books with pictures/diagrams.
#
# Preprocessing options (all opt-in, applied per page before the mode-specific step above,
# in this order: rotate -> crop -> split -> deskew). See README for details.
#   --rotate         detect pages lying on their side/upside down (Tesseract OSD) and rotate them
#   --crop           trim scanner/microfilm borders per page, then centre every page on one common canvas size
#   --split auto|off|N%   cut two-page spreads into separate pages (default: off)
#                    auto  = detect double pages and their gutter automatically
#                    N%    = force the gutter at N% of page width for every double-shaped page
#   --deskew         straighten each resulting page (small-angle rotation)
#
# Output defaults to "<input>.1bit.pdf" / "<input>.shrunk.pdf"; the original is never touched.
# Both modes rasterise the pages (a scan has no text layer to lose).
# PROGRESS_FILE=<path>: one line per finished page is appended there (used by the droplet).
# Exit code 2 means the PDF was written but some pages looked like double pages without a
# confident gutter, so they were left unsplit — see the warning on stderr for page numbers.
set -euo pipefail

# --- tunables for --rotate/--crop/--split (best-effort heuristics; verify output on real scans) ---
OSD_MIN_CONFIDENCE=1.0        # tesseract orientation-confidence floor; below this, don't rotate
DOUBLE_AR_MIN=1.15            # width/height above which a page is even considered double-shaped
GUTTER_INK_THRESH=230         # 0-255 gray level below which a (flattened) pixel counts as "ink"
GUTTER_SEARCH_LO=0.20         # a detected gutter must sit within this fraction of page width...
GUTTER_SEARCH_HI=0.80         # ...to this fraction, or it's not trusted (gutter isn't always centred)
GUTTER_MIN_WIDTH_FRAC=0.006   # gutter gap must be at least this wide (fraction of page width)
GUTTER_MAX_WIDTH_FRAC=0.22    # ...and at most this wide, else it's probably a blank facing page
GUTTER_MIN_INK_FRAC=0.03      # each half must have at least this fraction of ink-bearing columns
GUTTER_EDGE_SHAVE=0.03        # top/bottom band (fraction of page height) ignored when profiling columns for the gutter
GUTTER_MIN_INK_ROWS=2         # a column needs this many dark rows (of GRID_ROWS) to count as ink; one speck doesn't
GUTTER_TRUST_FRAC=0.08        # a "gutter" wider than this (fraction of page width) is a blank facing page, not a spine
GUTTER_VALLEY_RATIO=0.5       # fallback when no ink-free gap exists (text touches the spine): accept the deepest ink-density dip if it is below this fraction of the density on both sides
GRID_ROWS=48                  # rows sampled when building the per-column ink profile
CROP_MIN_DENSITY=0.03         # a row/column needs at least this ink fraction to count as content...
CROP_MAX_DENSITY=0.55         # ...and at most this (near-solid rows/columns are scanner borders)
CROP_MIN_RUN=0.005            # content must be a run at least this wide (fraction of page size); thinner stripes (scanner-edge/spine shadow) are ignored...
CROP_RUN_GAP=0.003            # ...where gaps up to this wide (between letters/lines) don't break a run
CROP_NEAR_DENSITY=0.004       # sparse ink (page numbers, running heads) this close to the text block is kept: weaker than MIN_DENSITY, but only within CROP_NEAR_FRAC
CROP_NEAR_FRAC=0.08           # how far (fraction of page size) beyond the text block sparse ink is searched for
CROP_BAND_FRAC=0.10          # a wide run lying entirely this close to an edge, cut off from the next run by a gap, is a scanner band: dropped
CROP_EDGE_FRAC=0.015          # ignore the outer band of each edge (fraction of page size)
CROP_PAD_FRAC=0.012           # margin kept around the detected text block

MODE=${MODE:-text}
ROTATE=0
CROP=0
DESKEW=0
SPLIT_MODE=off
SPLIT_FIXED=""

usage() { echo "usage: $0 [--rotate] [--crop] [--split auto|off|N%] [--deskew] input.pdf [output.pdf] [threshold%] [dpi]" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --rotate) ROTATE=1; shift ;;
    --crop) CROP=1; shift ;;
    --deskew) DESKEW=1; shift ;;
    --split)
      [ $# -ge 2 ] || usage
      case "$2" in
        auto) SPLIT_MODE=auto ;;
        off) SPLIT_MODE=off ;;
        *%) SPLIT_MODE=fixed; SPLIT_FIXED=${2%\%}
            case "$SPLIT_FIXED" in ''|*[!0-9.]*) usage ;; esac ;;
        *) usage ;;
      esac
      shift 2 ;;
    --) shift; break ;;
    -*) usage ;;
    *) break ;;
  esac
done

case $MODE in
  text)   SUFFIX=1bit;   DEFAULT_DPI=300 ;;
  images) SUFFIX=shrunk; DEFAULT_DPI=150 ;;
  *) echo "MODE must be text or images" >&2; exit 1 ;;
esac
IN=${1:?missing input.pdf. usage: $0 [--rotate] [--crop] [--split auto|off|N%] [--deskew] input.pdf [output.pdf] [threshold%] [dpi]}
OUT=${2:-"${IN%.pdf}.$SUFFIX.pdf"}
THRESH=${3:-60}
DPI=${4:-$DEFAULT_DPI}
PROGRESS_FILE=${PROGRESS_FILE:-/dev/null}
[ "$OUT" != "$IN" ] || { echo "output must differ from input" >&2; exit 1; }

if [ "$ROTATE" = 1 ] && ! command -v tesseract >/dev/null 2>&1; then
  echo "--rotate needs tesseract (brew install tesseract) — proceeding without rotation" >&2
  ROTATE=0
fi

PAGES=$(pdfinfo "$IN" | awk '/^Pages:/ {print $2}')
TMP=$(mktemp -d)
# Divide_Dst/Divide_Src swapped meaning between ImageMagick versions (#22): pick the one that gives orig/blur (0.25/0.5 = 0.5).
DIVIDE=Divide_Dst
[ "$(magick xc:gray25 xc:gray50 -compose Divide_Dst -composite -format '%[fx:mean<0.7?1:0]' info:)" = 1 ] || DIVIDE=Divide_Src
export DIVIDE
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/meta" "$TMP/plan"

# ---------- pass A: render, rotate, measure geometry (parallel, per page) ----------
render() {
  local n=$1 p; p=$(printf %04d "$n")
  local w="$TMP/w$p.png"
  if [ "$MODE" = text ]; then
    pdftoppm -gray -r "$DPI" -f "$n" -l "$n" -png "$IN" "$TMP/w$p"
  else
    pdftoppm -r "$DPI" -png -f "$n" -l "$n" "$IN" "$TMP/w$p"
  fi
  mv "$TMP/w$p"-*.png "$w"
}

rotate_page() {
  local w=$1 out deg conf
  out=$(tesseract "$w" stdout --psm 0 2>/dev/null) || return 0
  deg=$(printf '%s\n' "$out" | awk -F': ' '/^Rotate:/{print $2}')
  conf=$(printf '%s\n' "$out" | awk -F': ' '/^Orientation confidence:/{print $2}')
  [ -n "${deg:-}" ] && [ "$deg" != 0 ] || return 0
  awk -v c="${conf:-0}" -v m="$OSD_MIN_CONFIDENCE" 'BEGIN{exit !(c+0>=m)}' || return 0
  magick "$w" -rotate "$deg" -background white +repage "$w"
}

# per-column darkest-pixel profile of $1, used by measure_geometry to find text-block edges.
# Flatten first (same background-normalisation the final threshold step uses): a spine shadow
# is a smooth gradient and mostly divides away, while thin text ink stays dark against its own
# local background — without this, a dark shadow reads as "ink" and hides the real gutter.
ink_profile() {
  local w=$1 W=$2 H=$3
  # Shave the top/bottom edge bands first: a dark scanner border running along an edge would
  # otherwise make every column count as ink and hide the gutter altogether.
  local shave; shave=$(awk -v h="$H" -v f="$GUTTER_EDGE_SHAVE" 'BEGIN{printf "%d", h*f}')
  magick "$w" \( +clone -blur 0x30 \) -compose "$DIVIDE" -composite \
    -colorspace Gray -shave "0x$shave" -resize "${W}x${GRID_ROWS}!" -depth 8 txt:- | awk -v W="$W" -v T="$GUTTER_INK_THRESH" -v R="$GUTTER_MIN_INK_ROWS" '
    /^[0-9]+,[0-9]+:/ {
      split($0, parts, ":"); split(parts[1], xy, ","); x = xy[1] + 0
      gi = index($0, "gray("); s = substr($0, gi + 5); ci = index(s, ")")
      g = substr(s, 1, ci - 1) + 0
      if (g < T) dark[x]++
    }
    END { for (x = 0; x < W; x++) print x, (dark[x] >= R) ? 1 : 0, dark[x] + 0 }'
}

# text-block bounds of $1 as "w h x y". Plain `-trim` treats a dark scanner border as content and
# keeps it. Instead: flatten, threshold to an ink map, and take per-row/column ink density. Rows and
# columns that are near-solid (border stripes) or nearly empty (specks) don't count, and the outer
# CROP_EDGE_FRAC band is ignored, so borders that vary from page to page are cut per page.
content_box() {
  local w=$1 W=$2 H=$3
  local ink="$w.ink.png"
  magick "$w" \( +clone -blur 0x30 \) -compose "$DIVIDE" -composite \
    -colorspace Gray -threshold 60% -negate "$ink" || return 1
  local prof
  for axis in rows cols; do
    # raw 8-bit gray bytes, one per column/row (the txt: format differs between ImageMagick versions)
    # columns are profiled inside the row range only, so a band above/below the text does not put its columns into the box
    if [ "$axis" = cols ]; then read -r y0 ht < "$w.rows"; prof=$(magick "$ink" -crop "${W}x${ht}+0+${y0}" +repage -colorspace Gray -scale "${W}x1!" -depth 8 gray:- | od -An -v -tu1)
    else prof=$(magick "$ink" -colorspace Gray -scale "1x${H}!" -depth 8 gray:- | od -An -v -tu1); fi
    printf '%s\n' "$prof" | awk -v N="$([ "$axis" = cols ] && echo "$W" || echo "$H")" \
      -v lo="$CROP_MIN_DENSITY" -v hi="$CROP_MAX_DENSITY" -v edge="$CROP_EDGE_FRAC" -v pad="$CROP_PAD_FRAC" -v nlo="$CROP_NEAR_DENSITY" -v near="$CROP_NEAR_FRAC" -v minr="$CROP_MIN_RUN" -v rungap="$CROP_RUN_GAP" -v bandf="$CROP_BAND_FRAC" -v sc="$([ "$axis" = cols ] && awk -v a="$ht" -v b="$H" 'BEGIN{print a/b}' || echo 1)" '
      { for (k = 1; k <= NF; k++) { i = n++; d = $k / 255 * sc; dens[i] = d } }
      END {
        # runs of content rows/columns (density in range, inside the edge band), merged across small gaps;
        # the block spans the runs that are wide enough. No run wide enough: fall back to all of them.
        gap = int(rungap * N); minrun = int(minr * N); rs = -1; fin = int(N - 1 - edge * N) + 1
        for (i = int(edge * N); i <= fin; i++) {
          g = (i < fin && dens[i] >= lo && dens[i] <= hi)
          if (g) { if (rs < 0) rs = i; re = i; continue }
          if (rs >= 0 && (i - re > gap || i == fin)) {
            if (first0 == "") first0 = rs; last0 = re
            if (re - rs + 1 >= minrun) { nw++; wrs[nw] = rs; wre[nw] = re }
            rs = -1 }
        }
        # a wide run lying entirely in the outer CROP_BAND_FRAC of the page that is cut off from the rest by a gap is a scanner band, not text
        e0 = int(edge * N); band = int(bandf * N)
        lo_lim = e0; hi_lim = N - 1 - e0
        if (nw >= 2 && wre[1] <= band) { lo_lim = wre[1] + 1; for (k = 1; k < nw; k++) { wrs[k] = wrs[k+1]; wre[k] = wre[k+1] } nw-- }
        if (nw >= 2 && wrs[nw] >= N - 1 - band) { hi_lim = wrs[nw] - 1; nw-- }
        if (nw >= 1) { first = wrs[1]; last = wre[nw] }
        if (first == "") { first = first0; last = last0 }
        if (first == "") { first = 0; last = N - 1 }
        else {  # extend to sparse ink near the block; the speck filter still decides where the block is
          f0 = first; l0 = last
          for (i = f0 - 1; i >= f0 - near * N && i >= lo_lim; i--) if (dens[i] >= nlo && dens[i] <= hi) first = i
          for (i = l0 + 1; i <= l0 + near * N && i <= hi_lim; i++) if (dens[i] >= nlo && dens[i] <= hi) last = i
        }
        first = int(first - pad * N); last = int(last + pad * N)
        if (first < 0) first = 0; if (last > N - 1) last = N - 1
        if (lo_lim > e0 && first < lo_lim) first = lo_lim; if (hi_lim < N - 1 - e0 && last > hi_lim) last = hi_lim
        print first, last - first + 1 }' > "$w.$axis"
  done
  read -r x0 wd < "$w.cols"; read -r y0 ht < "$w.rows"; rm -f "$w.cols" "$w.rows" "$ink"
  echo "$wd $ht $x0 $y0"
}

measure_geometry() {
  local n=$1 p; p=$(printf %04d "$n")
  local w="$TMP/w$p.png" meta="$TMP/meta/$p.txt"
  local W H tx ty tw th
  read -r W H < <(magick identify -format '%w %h' "$w")
  read -r tw th tx ty < <(content_box "$w" "$W" "$H") || { tw=$W; th=$H; tx=0; ty=0; }
  tx=$((tx)); ty=$((ty))
  local ar; ar=$(awk -v w="$W" -v h="$H" 'BEGIN{printf "%.4f", w/h}')
  local ar_candidate; ar_candidate=$(awk -v ar="$ar" -v m="$DOUBLE_AR_MIN" 'BEGIN{print (ar>=m)?1:0}')

  local gutter_found=0 gutter_x=0 gutter_w=0
  if [ "$ar_candidate" = 1 ] && [ "$SPLIT_MODE" != off ]; then
    if [ "$SPLIT_MODE" = fixed ]; then
      gutter_x=$(awk -v w="$W" -v f="$SPLIT_FIXED" 'BEGIN{printf "%d", w*f/100}')
      gutter_found=1
    else
      local profile; profile=$(ink_profile "$w" "$W" "$H")
      read -r gutter_found gutter_x gutter_w < <(printf '%s\n' "$profile" | awk -v W="$W" \
        -v lo="$GUTTER_SEARCH_LO" -v hi="$GUTTER_SEARCH_HI" \
        -v minwf="$GUTTER_MIN_WIDTH_FRAC" -v maxwf="$GUTTER_MAX_WIDTH_FRAC" -v minink="$GUTTER_MIN_INK_FRAC" -v vr="$GUTTER_VALLEY_RATIO" '
        { ink[$1] = $2; cnt[$1] = $3 }
        # no ink-free gap (text touches the spine, or skew keeps every column faintly inked): take the
        # deepest dip of a smoothed ink-density profile inside the search window, if it is clearly
        # below the density on both sides. Prints "found x".
        function valley(   sw, x, k, sm, n, best, bx, lb, rb, ln, rn, a1, a2) {
          sw = int(W * 0.003); if (sw < 1) sw = 1
          for (x = 0; x < W; x++) { n = 0; sm[x] = 0
            for (k = x - sw; k <= x + sw; k++) if (k >= 0 && k < W) { sm[x] += cnt[k]; n++ }
            sm[x] /= n }
          best = 1e9; bx = -1
          for (x = int(W * lo); x <= int(W * hi); x++) if (sm[x] < best) { best = sm[x]; bx = x }
          if (bx < 0) return "0 0"
          a1 = int(W * 0.03); a2 = int(W * 0.15)
          for (k = bx - a2; k <= bx - a1; k++) if (k >= 0) { lb += sm[k]; ln++ }
          for (k = bx + a1; k <= bx + a2; k++) if (k < W) { rb += sm[k]; rn++ }
          if (ln == 0 || rn == 0) return "0 0"
          lb /= ln; rb /= rn
          return (best <= vr * lb && best <= vr * rb && lb > 0 && rb > 0) ? ("1 " bx) : "0 0"
        }
        END {
          # widest run of non-ink columns whose midpoint falls in the centre search window —
          # more robust than "nearest ink from the centre" against a single noisy pixel
          bestlen = -1; bests = -1; beste = -1; runstart = -1
          for (x = 0; x <= W; x++) {
            isink = (x < W) ? ink[x] : 1
            if (!isink) { if (runstart < 0) runstart = x }
            else if (runstart >= 0) {
              rs = runstart; re = x - 1; runstart = -1
              mid = (rs + re) / 2
              if (mid / W >= lo && mid / W <= hi) {
                len = re - rs
                if (len > bestlen) { bestlen = len; bests = rs; beste = re }
              }
            }
          }
          if (bests < 0) {
            print valley(), 0
          } else {
            gx = int((bests + beste) / 2)
            gw = beste - bests
            li = 0; ln = 0; ri = 0; rn = 0
            half = int(W / 2)
            for (x = 0; x < half; x++) { ln++; li += ink[x] }
            for (x = half; x < W; x++) { rn++; ri += ink[x] }
            lifrac = (ln > 0) ? li / ln : 0
            rifrac = (rn > 0) ? ri / rn : 0
            gfrac = gx / W
            gwfrac = gw / W
            ok = (gfrac >= lo && gfrac <= hi && gwfrac >= minwf && gwfrac <= maxwf && lifrac >= minink && rifrac >= minink)
            if (ok) print 1, gx, gw; else print valley(), 0
          }
        }')
    fi
  fi

  {
    echo "W=$W"; echo "H=$H"; echo "AR_CANDIDATE=$ar_candidate"
    echo "TRIM_X=$tx"; echo "TRIM_Y=$ty"; echo "TRIM_W=$tw"; echo "TRIM_H=$th"
    echo "GUTTER_FOUND=$gutter_found"; echo "GUTTER_X=$gutter_x"; echo "GUTTER_W=$gutter_w"
  } > "$meta"
}

passA() {
  local n=$1
  render "$n"
  local p; p=$(printf %04d "$n")
  [ "$ROTATE" = 1 ] && rotate_page "$TMP/w$p.png"
  if [ "$CROP" = 1 ] || [ "$SPLIT_MODE" != off ]; then
    measure_geometry "$n"
  fi
}
export -f render rotate_page ink_profile content_box measure_geometry passA
export IN TMP MODE DPI ROTATE CROP SPLIT_MODE SPLIT_FIXED
export OSD_MIN_CONFIDENCE DOUBLE_AR_MIN GUTTER_INK_THRESH GUTTER_SEARCH_LO GUTTER_SEARCH_HI
export GUTTER_MIN_WIDTH_FRAC GUTTER_MAX_WIDTH_FRAC GUTTER_MIN_INK_FRAC GRID_ROWS
export CROP_BAND_FRAC GUTTER_VALLEY_RATIO GUTTER_EDGE_SHAVE GUTTER_MIN_INK_ROWS GUTTER_TRUST_FRAC CROP_MIN_DENSITY CROP_NEAR_DENSITY CROP_NEAR_FRAC CROP_MIN_RUN CROP_RUN_GAP CROP_MAX_DENSITY CROP_EDGE_FRAC CROP_PAD_FRAC

echo "Pass 1/2: rendering + measuring $PAGES pages (4 at a time, MODE=$MODE)..."
seq 1 "$PAGES" | xargs -P 4 -I{} bash -c 'passA {}'

# ---------- between passes: document-level medians (sequential, cheap) ----------
median() { sort -n | awk '{a[NR]=$1; n=NR} END{if(n==0){print ""; exit} if(n%2)print a[(n+1)/2]; else print int((a[int(n/2)]+a[int(n/2)+1])/2)}'; }

SINGLE_X0=""; SINGLE_Y0=""; SINGLE_X1=""; SINGLE_Y1=""
DOUBLE_GFRAC=""

if [ "$CROP" = 1 ]; then
  for n in $(seq 1 "$PAGES"); do
    p=$(printf %04d "$n"); m="$TMP/meta/$p.txt"
    [ -f "$m" ] || continue
    ( . "$m"
      if [ "$AR_CANDIDATE" != 1 ]; then
        echo "$TRIM_X" >> "$TMP/.x0"; echo "$TRIM_Y" >> "$TMP/.y0"
        echo "$((TRIM_X+TRIM_W))" >> "$TMP/.x1"; echo "$((TRIM_Y+TRIM_H))" >> "$TMP/.y1"
      fi )
  done
  [ -f "$TMP/.x0" ] && { SINGLE_X0=$(median < "$TMP/.x0"); SINGLE_Y0=$(median < "$TMP/.y0")
                          SINGLE_X1=$(median < "$TMP/.x1"); SINGLE_Y1=$(median < "$TMP/.y1"); }
fi

# 1 if this page's detected gutter is too wide to be a spine (needs GUTTER_W/W from a sourced meta file)
gutter_wide() { awk -v gw="$GUTTER_W" -v w="$W" -v t="$GUTTER_TRUST_FRAC" 'BEGIN{print (gw > t*w) ? 1 : 0}'; }

if [ "$SPLIT_MODE" = auto ]; then
  for n in $(seq 1 "$PAGES"); do
    p=$(printf %04d "$n"); m="$TMP/meta/$p.txt"
    [ -f "$m" ] || continue
    ( . "$m"
      if [ "$AR_CANDIDATE" = 1 ] && [ "$GUTTER_FOUND" = 1 ] && [ "$(gutter_wide)" = 0 ]; then
        awk -v x="$GUTTER_X" -v w="$W" 'BEGIN{printf "%.4f\n", x/w}' >> "$TMP/.gfrac"
      fi )
  done
  [ -f "$TMP/.gfrac" ] && DOUBLE_GFRAC=$(median < "$TMP/.gfrac")
fi

REVIEW_FILE="$TMP/review.txt"
: > "$REVIEW_FILE"

compute_plan() {
  local n=$1 p; p=$(printf %04d "$n")
  local m="$TMP/meta/$p.txt" plan="$TMP/plan/$p.txt"
  if [ -f "$m" ]; then
    ( . "$m"
      out_split=0; out_x0=""; out_y0=""; out_x1=""; out_y1=""; out_gx=""
      if [ "$SPLIT_MODE" != off ] && [ "$AR_CANDIDATE" = 1 ]; then
        gx=""
        if [ "$GUTTER_FOUND" = 1 ] && { [ -z "$DOUBLE_GFRAC" ] || [ "$(gutter_wide)" = 0 ]; }; then
          gx=$GUTTER_X
        elif [ "$SPLIT_MODE" = auto ] && [ -n "$DOUBLE_GFRAC" ]; then
          gx=$(awk -v f="$DOUBLE_GFRAC" -v w="$W" 'BEGIN{printf "%d", f*w}')
        fi
        if [ -n "$gx" ]; then
          # crop to the real content bounds (never loses text) and split at the actual
          # detected gutter, rather than forcing a symmetric box and cutting it at 50%
          out_split=1
          out_x0=$TRIM_X; out_x1=$((TRIM_X+TRIM_W)); out_y0=$TRIM_Y; out_y1=$((TRIM_Y+TRIM_H)); out_gx=$gx
          # a blank facing page has no content, so the box can start/end on the far side of the
          # gutter: mirror the other half's width instead of collapsing the blank page to nothing
          if [ "$out_x0" -ge "$gx" ]; then out_x0=$((gx-(out_x1-gx))); [ "$out_x0" -lt 0 ] && out_x0=0; fi
          if [ "$out_x1" -le "$gx" ]; then out_x1=$((gx+(gx-out_x0))); [ "$out_x1" -gt "$W" ] && out_x1=$W; fi
        else
          echo "$n" >> "$REVIEW_FILE"
        fi
      fi
      if [ "$out_split" = 0 ] && [ "$CROP" = 1 ]; then
        if [ -n "$SINGLE_X0" ]; then out_x0=$SINGLE_X0; out_y0=$SINGLE_Y0; out_x1=$SINGLE_X1; out_y1=$SINGLE_Y1
        else out_x0=$TRIM_X; out_y0=$TRIM_Y; out_x1=$((TRIM_X+TRIM_W)); out_y1=$((TRIM_Y+TRIM_H)); fi
      fi
      { echo "SPLIT=$out_split"; echo "X0=$out_x0"; echo "Y0=$out_y0"; echo "X1=$out_x1"; echo "Y1=$out_y1"; echo "GX=$out_gx"; } > "$plan"
    )
  else
    { echo "SPLIT=0"; echo "X0="; echo "Y0="; echo "X1="; echo "Y1="; echo "GX="; } > "$plan"
  fi
}
for n in $(seq 1 "$PAGES"); do compute_plan "$n"; done

# one output size for the whole document: the largest text box (never clips text). Every page is
# centred on a white canvas of that size in pass B, so a book doesn't jump around when read.
TW=0; TH=0
if [ "$CROP" = 1 ] || [ "$SPLIT_MODE" != off ]; then
  read -r TW TH < <(awk -F= '
    $1=="SPLIT" { s=$2; x0=y0=x1=y1=gx="" }
    $1=="X0" { x0=$2 } $1=="Y0" { y0=$2 } $1=="X1" { x1=$2 } $1=="Y1" { y1=$2 }
    $1=="GX" { gx=$2
      if (x0 != "") {
        if (y1-y0 > th) th = y1-y0
        if (s == 1) { if (gx-x0 > tw) tw = gx-x0; if (x1-gx > tw) tw = x1-gx }
        else if (x1-x0 > tw) tw = x1-x0
      } }
    END { print tw+0, th+0 }' "$TMP"/plan/*.txt)
fi

# ---------- pass B: crop/split/deskew + mode-specific finishing (parallel, per page) ----------
finish_slot() {
  # crop+threshold (text) or crop+jpeg (images) a single working file into its final slot
  local src=$1 dst_base=$2
  if [ "$MODE" = text ]; then
    magick "$src" \( +clone -blur 0x30 \) -compose "$DIVIDE" -composite \
      -threshold "$THRESH%" -type bilevel -units PixelsPerInch -density "$DPI" \
      -compress Group4 "$dst_base.tif"
  else
    magick "$src" -quality 65 -units PixelsPerInch -density "$DPI" "$dst_base.jpg"
  fi
}

passB() {
  local n=$1 p; p=$(printf %04d "$n")
  local w="$TMP/w$p.png" plan="$TMP/plan/$p.txt"
  local SPLIT=0 X0="" Y0="" X1="" Y1="" GX=""
  [ -f "$plan" ] && . "$plan"

  if [ "$SPLIT" = 1 ]; then
    local h=$((Y1-Y0)) lwidth=$((GX-X0)) rwidth=$((X1-GX))
    local lw="$TMP/s${p}a.png" rw="$TMP/s${p}b.png"
    magick "$w" -crop "${lwidth}x${h}+${X0}+${Y0}" +repage "$lw"
    magick "$w" -crop "${rwidth}x${h}+${GX}+${Y0}" +repage "$rw"
    if [ "$DESKEW" = 1 ]; then
      magick "$lw" -background white -deskew 40% +repage "$lw"
      magick "$rw" -background white -deskew 40% +repage "$rw"
    fi
    if [ "$TW" -gt 0 ]; then
      magick "$lw" -background white -gravity center -extent "${TW}x${TH}" +repage "$lw"
      magick "$rw" -background white -gravity center -extent "${TW}x${TH}" +repage "$rw"
    fi
    finish_slot "$lw" "$TMP/p${p}a"
    finish_slot "$rw" "$TMP/p${p}b"
  else
    local sw="$w"
    if [ -n "$X0" ]; then
      sw="$TMP/s${p}a.png"
      magick "$w" -crop "$((X1-X0))x$((Y1-Y0))+${X0}+${Y0}" +repage "$sw"
    fi
    [ "$DESKEW" = 1 ] && magick "$sw" -background white -deskew 40% +repage "$sw"
    if [ "$TW" -gt 0 ] && [ -n "$X0" ]; then
      magick "$sw" -background white -gravity center -extent "${TW}x${TH}" +repage "$sw"
    fi
    finish_slot "$sw" "$TMP/p${p}a"
  fi
  echo "$n" >> "$PROGRESS_FILE"
}
export -f finish_slot passB
export THRESH PROGRESS_FILE DESKEW TW TH

echo "Pass 2/2: cropping/splitting/deskewing + MODE=$MODE finishing..."
seq 1 "$PAGES" | xargs -P 4 -I{} bash -c 'passB {}'

magick "$TMP"/p*.[tj]* -units PixelsPerInch -density "$DPI" "$OUT"

if [ "$(wc -c <"$OUT")" -ge "$(wc -c <"$IN")" ]; then
  rm -f "$OUT"
  echo "Not smaller ($(du -h "$IN" | cut -f1) is already small) — no file written"
  exit 0
fi
echo "$(du -h "$IN" | cut -f1) -> $(du -h "$OUT" | cut -f1): $OUT"

if [ -s "$REVIEW_FILE" ]; then
  echo "Warning: pages look like double pages but no confident gutter was found — left unsplit: $(paste -sd, "$REVIEW_FILE")" >&2
  echo "Check these by hand, or re-run with --split <N%> to force a gutter position." >&2
  exit 2
fi
