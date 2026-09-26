# What we tried, what works, what doesn't

Living log of the `--rotate/--crop/--split/--deskew` pipeline in `bitonalpdf.sh`. It records
**measured** results only; anything guessed is marked *(unverified)*. Real scans are copyrighted and
stay in the git-ignored `tests/real/`; findings about them are written down here instead.

Reference scans (local only): `skewed` (23 pp, 16 MB, skewed double pages), `sidste side helt til margin`
(8 pp, 4.8 MB, borders), `flerspaltet på nær første side` (9 pp, multi-column, must never split),
`ryg-side` (1 p, page numbers close to the text), `ren-side` and `ren pdf` (clean, ~no-op).

## Status at a glance

| Area | State |
|---|---|
| Rotate (Tesseract OSD), split, deskew | Works on both first real scans: skewed 23 → 46 pp, Lundager 8 → 16 pp, no false splits (#3) |
| Crop: text block, no scanner borders, uniform page size | Works (#7) |
| Page numbers survive crop | Works on all six real scans (#19/#21) |
| Thin stray stripes no longer widen the crop | Works on `skewed` (#23/#24) |
| Newer ImageMagick | Works since #22 (before: silently wrong) |
| Wide dark bands / striped edge (`sidste side …`) | **Not fixed** (#23 still open) |
| Thin strokes lose ink at the hard 60 % threshold | **Not started** (#20) |
| Speed | **Slow**: ~5 min for 23 pp on an M-series Mac (#8/#9, the reason for the Rust port) |

## Problems, causes, fixes

### 1. `-trim` keeps dark scanner borders (#7)
Symptom: crop kept the black border. Cause: `-trim` treats any non-background pixel as content.
Fix: threshold to an ink map, take per-row/column ink density; rows/columns that are nearly empty
(< `CROP_MIN_DENSITY` 3 %) or near-solid (> `CROP_MAX_DENSITY` 55 %) don't count; ignore the outer
1.5 %. **Works.** Side effect by design: every page is centred on one canvas the size of the largest
text block, so a single bad page can enlarge all pages.

### 2. Gutter found on only 1 of 8 spreads (#7)
Causes, both measured on `sidste side …`: (a) a dark border along the top made every column count
as ink, so no gap was found; (b) a blank facing page was read as a ~15 % wide "gutter" and skewed the
document-median fallback. Fixes: shave the top/bottom 3 % before profiling; a gutter wider than
`GUTTER_TRUST_FRAC` (8 %) is a blank page, not a spine. **Works.**

### 3. Text touching the spine: no ink-free gap (#3/#17)
Fallback: deepest dip of the smoothed column-ink profile, accepted only if below
`GUTTER_VALLEY_RATIO` (0.5) of the density on both sides, and only reached when the strict search
finds nothing. Added in #17; its effect on the real scans was not separately measured *(unverified)*.

### 4. Page numbers cut off (#19/#21)
Cause: a page-number line is a few digits, density far below the 3 % speck filter, so the crop ended
above it (same risk for running heads, footnote markers). Fix: keep the speck filter for finding the
block, then extend the box to rows/columns with density ≥ `CROP_NEAR_DENSITY` (0.4 %) within
`CROP_NEAR_FRAC` (8 % of the page) of it. Verified by eye on `ryg-side` (26, 27), `ren-side` (427) and on
`skewed`/`sidste side …` against the old code side by side (numbers missing before, present after).
The two constants come from the issue's suggestion and these scans, not from a corpus.
**Known risk:** a stripe within 8 % of the text is also admitted by this extension.

### 5. Different results in different ImageMagick versions (#22)
Found by CI (7.1.2-31, Linux) versus the Mac (7.1.2-24): `-compose Divide_Dst` gave the *blurred*
image instead of original ÷ blur, so thin ink vanished before thresholding. Measured on CI:
pixel 0.15 → 0.91 with `Divide_Dst`, 0.16 with `Divide_Src`. Fix: probe once at start which of the two
gives 0.25 / 0.5 = 0.5. Dense blocks still looked fine on the broken version, which is why the
big-block smoke test never noticed. **Lesson:** an image-op pipeline can be silently wrong on another
library version; the Rust port should not depend on a library's compose semantics (see below).

### 6. Thin dark stripe drags the crop to the page edge (#23/#24)
Cause (measured on `skewed` page 60 of the output): one column at x=59 with 5 % density (above the
speck filter) made the box start at x=10 although the text starts at x=593, so the scanner-edge bar
stayed in the output. Fix: content must be a *run* at least `CROP_MIN_RUN` (0.5 % of the page) wide,
merging gaps up to `CROP_RUN_GAP` (0.3 %); thinner runs are ignored, fall back to the old behaviour
if no run is wide enough. **Works** on `skewed` (bars on output pages 21/27 gone).
**Not fixed:** `sidste side …` still has dark bands at the top of a few pages and a striped left edge;
these are wide, so the run rule does not touch them (open in #23).

## Things that did not work or were misleading

- `real.sh` says "ok" while the output is wrong. It only checks that a file is written, all pages have
  one size and the page count. Whether the numbers/text survive must be **looked at**: contact sheet
  (`pdftoppm -r 30` + `magick montage`), or old-vs-new side by side on the same input page.
- A page run alone can behave differently from the same page in a full run: the document-median gutter
  fallback, the median single-page crop box and the uniform canvas only exist in a full run. The
  `skewed` bar on output page 27 did not reproduce on a single page; a 5-page run around it did.
- Pure-black test pixels + newer ImageMagick: use dark gray in synthetic fixtures. (This was a wrong
  first guess for #22 and did not fix it; the real cause is the compose swap above. The gray is kept
  because it is closer to real ink.)
- CI has no fonts: `magick … -annotate` fails there. Draw rectangles instead.
- `pdfinfo` `Page size: W x H pts`: awk fields are `$3` = width, `$5` = height (twice wrong in smoke tests).
- A regression test must be shown to **fail without the fix**; two of ours passed vacuously first.
- Don't run `real.sh` and `smoke.sh` at once, and don't `git stash` the script while a run uses it.

## Not tried yet

- #20: contrast stretch / adaptive threshold / render-high-then-downsample before binarising.
- `--columns` (multi-column reading order) and `--ocr` (hidden text layer): deferred on purpose.
- A corpus beyond six scans; all constants are tuned on these.

## Notes for the Rust port (#8/#9)

- Golden fixtures: the six local scans plus the smoke synthetics. Compare page counts, page size and
  the *per-axis content boxes*, not just "no crash".
- Re-implement the flatten (divide by a blurred copy) explicitly; do not inherit a library's compose enum.
- Port the density/run/near-extension logic as written above; the constants are the tuning state.
- Speed target: the bash version needs ~5 min for 23 pp at 300 dpi.

### Could the Rust CLI also run on Windows?
Very likely yes, with caveats; none of this has been spiked yet *(unverified)*:
- Rust itself, and pure-Rust crates for imaging/PDF writing/CCITT (`image`, `imageproc`, `lopdf`/`pdf-writer`,
  `fax`), build on Windows unchanged. This is the biggest gain over the bash script, which needs
  bash, ImageMagick, poppler and BSD-vs-GNU tool behaviour.
- Rendering pages that are not a single embedded image needs pdfium (`pdfium-render`) or `hayro`; pdfium
  means shipping `pdfium.dll` next to the exe.
- `--rotate` uses Tesseract OSD; on Windows Tesseract is a separate install, so keep it optional (as on
  macOS) or replace it with an own orientation heuristic.
- The droplet (`bitonalPDF.app`, AppleScript) is macOS only. Windows needs its own front end: drag-and-drop
  onto the `.exe`, a shortcut, or a small GUI.
- CI: add a `windows-latest` job running `smoke` for the Rust binary; keep bash-specific tests out of it.
