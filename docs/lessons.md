# What we tried, what works, what doesn't

Living log of the `--rotate/--crop/--split/--deskew` pipeline in `bitonalpdf.sh`. It records
**measured** results only; anything guessed is marked *(unverified)*. Real scans are copyrighted and
stay in the git-ignored `tests/real/`; findings about them are written down here instead.

Reference scans (local only) and the results `tests/real.sh` expects:

| File | What it is | Expected |
|---|---|---|
| `skewed` | skewed double pages, 23 pp, 16 MB (the "Otzen" scan in #3) | 23 → 46 pp, ~5 min |
| `sidste side helt til margin` | borders, blank facing page, 8 pp, 4.8 MB (the "Lundager" scan in #3) | 8 → 16 pp |
| `flerspaltet på nær første side` | multi-column article, must never be split | 9 → 9 pp |
| `ryg-side` | one spread, text right up to the spine, page numbers 26/27 | 1 → 2 pp |
| `ren-side`, `ren pdf` | clean, should be (nearly) a no-op | `ren pdf`: not smaller, no file written, counts as ok |

(The Otzen/Lundager ↔ file-name mapping is inferred from page counts and sizes in #3.)

## Status at a glance

| Area | State |
|---|---|
| Rotate (Tesseract OSD), split, deskew | Works on the real scans: skewed 23 → 46 pp, sidste side 8 → 16 pp, no false splits; multi-column and clean files behave (#3) |
| Crop: text block, no scanner borders, uniform page size | Works (#7) |
| Page numbers survive crop | Works on all six real scans (#19/#21) |
| Thin stray stripes no longer widen the crop | Works on `skewed` (#23/#24) |
| Newer ImageMagick | Works since #22 (before: silently wrong) |
| Wide dark bands / striped edge (`sidste side …`) | Works (#23): bands gone on the contact sheet, page numbers kept |
| Thin strokes lose ink at the hard 60 % threshold | Works (#20): hysteresis, 60 % + weak pixels up to 75 % next to a seed < 45 % |
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
finds nothing. Measured: `ryg-side` (text to the spine) went from "looks double, no confident gutter, left
unsplit" to 1 → 2 pages, cut checked visually; the other real scans are unaffected (#3, closing comment).
Ideas not yet tried for this case are in "Ideas from #8" below.

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

### 6b. Wide dark bands (#23)
Rule: a wide run lying entirely within the outer `CROP_BAND_FRAC` (10 %) of the page and cut off from the next run
by a gap is a scanner band and is dropped; sparse-ink extension and padding may not grow back into it. Columns are
now profiled *inside the row range* (rows first, densities scaled by ht/H so `CROP_MAX_DENSITY` keeps its meaning),
otherwise the band's columns widened the box. Lessons: (1) the flatten (divide by blur) removes the *interior* of a
big dark band, only a fringe survives as ink, so "starts at the edge" tests fail: test that the run lies *inside* the
edge zone; (2) the near-extension and the pad silently re-admitted the band, both needed a limit; (3) shell state does
not persist between tool calls, export the tunables when testing `content_box` by hand.
Effect on `sidste side …`: canvas 485x591 -> 389x581 pt (bands no longer enlarge it), page numbers kept. Also
changed sizes slightly on `skewed`, `ryg-side`, `flerspaltet`, `ren-side` (checked by eye, page numbers present).
**Known risk:** a running head lying alone in the top 10 % behind a gap is dropped like a band.

### 6c. Thin strokes: hysteresis instead of a higher threshold (#20)
A/B on `skewed` p5 (300 dpi, after flatten): black fraction / G4 size at 60 % 7.1 % / 89.4 kB, 70 % 7.9 %, 75 % 8.3 % /
89.7 kB; `-level`/sigmoidal ≈ 60 %; `-lat` 8.9 % / 91.5 kB (+2 %, noisier); render 600 dpi → 300 → t60 6.9 %. On a clean
page 60/70/75 look the same; on thin scans 75 closes broken serifs and dots best.
A higher *global* threshold fails the facts, because the scanner-edge fringe sits near 60-65 % grey: at 70 the `dark-band`
synth case and `skewed` p1 get ink at the box edge, at 65 `sidste side …` p11/12 get a dark line along the bottom.
Plain hysteresis with seeds at 60 % also failed: the fringe has a few pixels just under 60 that act as seeds.
**What works:** ink = darker than 60 % (unchanged) **plus** pixels up to `WEAK_THRESH` (75 %) that lie within
`HYST_RADIUS` (2 px) of a *seed* darker than `SEED_THRESH` (45 %). Black fraction on `skewed` p5 8.2 %, size 89.3 kB (no
growth); synth all PASS, `skewed`/`ryg-side`/`ren-side`/`flerspaltet` facts unchanged; `sidste side …` p9 (top, 40→18 ‰) and
p11 (left/right edge) get faint dotted marks in the margin, viewed and judged harmless: text is clearly better, no bands.
Facts of `sidste side …` re-recorded. Env `WEAK_THRESH`/`HYST_RADIUS`/`SEED_THRESH` tune it; `WEAK_THRESH<=THRESH` turns it off.
Port note: implement as a per-pixel rule (dark < 0.60, or dark < 0.75 and a pixel < 0.45 within 2 px), not via a morphology call.

### 7. A yardstick that says more than "ok" (#27)
`tests/measure.sh file.pdf` prints per output page: size in pt, the ink bounding box in per-mille of the
page, and whether there is ink in the bottom 12 % (footer/page number). Two consumers:
- `tests/real.sh` compares that against `tests/real/<name>.facts` (git-ignored): size exact, box ±20 ‰
  (`FACT_TOL`), footer flag equal, page count equal. `--record` writes them (do it only after looking at
  `out/`), `ONLY=<name>` runs one scan, `BIN=<script>` runs another implementation. A changed fact fails:
  flipping the footer flag of `ryg-side` page 1 gave `FAIL facts differ: page 1: footer ink 1 (expected 0)`.
- `tests/synth.sh` builds fixtures from rectangles/speckle (no fonts), runs `BIN` (default `bitonalpdf.sh`),
  measures, prints one row per case: PASS / FAIL / KNOWN-FAIL / XPASS (XPASS fails the run so a fixed
  defect gets moved out of the known list). It runs in CI after `smoke.sh`.

Measured (each case shown to fail on pre-fix code, same fixtures):

| Case | Pre-fix code | Result there | Current `main` |
|---|---|---|---|
| `pagenumber` (#19) | `7fa02c2` | FAIL (Mac and CI) | PASS |
| `stripe` (#23) | `0d2b7a7` | FAIL (Mac and CI) | PASS |
| `flatten` (#22) | `7fa02c2`, ImageMagick 7.1.2-31 (CI) | FAIL, output page blank | PASS |
| `flatten` (#22) | `7fa02c2`, ImageMagick 7.1.2-24 (Mac) | passes (the bug does not exist there) | PASS |
| `dark-band` (#23) | any before the fix | KNOWN-FAIL: ink box touches the top edge | PASS |

Durations: `synth.sh` ~84 s on CI (ubuntu; `smoke.sh` 54 s there), ~81 s on the Mac (`smoke.sh` 46 s). The
real-scan run took 4 min 49 s on the Mac when compared against the facts (`skewed` 151 s; all six pass, so the facts are reproducible), 10.5 min the first time while other jobs ran.
The real scans' recorded facts confirm the known defect: `sidste side …` pages 7, 8, 11 and 12 have an ink box
touching the top edge (11 also full width) (the dark bands, #23), and page 1 is blank (the blank facing page).
Pitfalls found: the pre-fix `7fa02c2` on ImageMagick -31 also "passes" `stripe`/`dark-band` because its
flatten is broken (blank page), so read a pre-fix row only for the case it is meant to prove; `-format '%w %h'`
without `\n` makes `read` fail under `set -e`; `magick … -format %@` warns on an all-white page.

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

## Open defects and decisions

- **Canvas = largest text block.** One stray mark can enlarge every page. Idea: a percentile instead of
  the max, with clipping guarded so text is never cut (#3 handover). Any widening of the crop (as in #19)
  must keep this in mind.
- **Small black marks / a thin vertical bar** on some `skewed` pages (#3): the thin bar is addressed by #24;
  small isolated marks were not re-checked separately.
- **Fixtures.** Three archive.org candidates (`sim_solicitors-journal_1866-09-08_10`,
  `sim_unitarian-register…_1842-10-01_21_40`, `pappenheimersnov00asht_1`) were checked by cover thumbnail
  and are single portrait pages, not spreads: useless for split tests. Still needed: real *spread*
  scans with clear rights (`licenseurl`/CC0/public domain); 2–4 page excerpts could then live in the repo.
- **A real scan is in git history.** `tests/real/lundager.pdf` was committed by accident in #11 and removed
  in #12, but stays in history. The repo is public; decision so far: accepted. `tests/real/` is git-ignored.
- **Synthetic fixture suite.** An old deleted branch (`claude/practical-heisenberg-o8cfcu`, last commit
  `b3bd6ac`, reachable by SHA only for a while) had `tests/generate_fixtures.py` + runner. Too heavy for
  the bash CI (#11, #14) but a good parity suite for the port: run bash and Rust on the same generated
  fixtures and compare (#9).
- **Original design choices (#1).** Order matters: rotate → crop → split → deskew → flatten/threshold. The
  first idea (crop symmetric around the gutter, then cut at 50 %) was replaced by cropping to the real
  content bounds and splitting at the detected gutter (#2). Multi-column pages are never split by default.
  `--ocr` (hidden text layer) is out on purpose: it does not help the markdown-extraction pipeline, only
  reading the PDF itself.

## Ideas to test (from #8, unverified: from web search/descriptions, not source-read)

1. **Spine-shadow signal.** Scan Tailor Advanced reportedly combines text evidence with a separate
   spine-darkness search; we only use ink columns of a background-flattened page, and flattening divides
   the shadow away. A per-column mean grey level *before* flattening is a cheap second signal
   (relevant for `ryg-side`).
2. **Order of operations.** OCRmyPDF: rotate → remove background → deskew → clean. We find the gutter
   before deskew, so a skewed spine smears the column profile. Test: deskew (or estimate the angle) first.
3. **Binarise first?** Hypothesis: smaller/faster to profile, but thresholding may erase the shadow or turn
   it into a solid band that reads as ink. Measure the shadow on grey first, then A/B.
4. **Rust candidates to spike:** `lopdf` (read/write, no render), `hayro` (pure-Rust render),
   `pdfium-render` (needs Pdfium), `ocr`/`ocrcer` crates (claim deskew, Sauvola, auto-rotate, columns).
   No Rust project found doing spread + border + spine in one tool.
5. **Reading list (prior art is GPL: learn the ideas, do not copy code unless our licence allows):**
   Scan Tailor Advanced (`filters/page_split/PageLayoutEstimator.cpp`, spine-darkness search, content-box
   finder), unpaper (border/black-edge detection, deskew), OCRmyPDF (step order).
6. #19 (page numbers), #20 (thresholding) and #23 (dark edges) should be regression cases for the port.

## Working tips

- `shellcheck` is not installed locally (`brew install shellcheck`); CI runs it.
- macOS `sed -i` needs a backup argument; use python for scripted edits.
- `tests/real.sh` takes ~8–10 min in total (`skewed` is the slow one).

## Not tried yet

- #20: contrast stretch / adaptive threshold / render-high-then-downsample before binarising.
- `--columns` (multi-column reading order) and `--ocr` (hidden text layer): deferred on purpose.
- A corpus beyond six scans; all constants are tuned on these.

## Notes for the Rust port (#8/#9)

- Golden fixtures: the six local scans (`.facts` files) plus `tests/synth.sh`; plug the binary in with `BIN=`. Compare page counts, page size and
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
