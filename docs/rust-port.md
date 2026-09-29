# Rust port: decisions and measurements

Spike code: `rust/` (one file, `src/main.rs`). Build: `cd rust && cargo build --release`; the binary takes the
`bitonalpdf.sh` CLI, so `BIN=rust/target/release/bitonalpdf tests/synth.sh` works. Text mode only (no `MODE=images`).
Since #29 it does `--rotate` (Tesseract, as in bash; since #30 an own detector by default, section 3), `--crop`, `--split auto|off|N%` and `--deskew`.
`BITONAL_TIMING=1` prints per-page geometry and stage times, `BITONAL_RENDER=1` forces the render path; the A/B knobs of
section 2 are listed at the top of `main.rs`.

All numbers: Apple M4 (10 cores), macOS, release build, the six local scans in `tests/real/`. Bash = `bitonalpdf.sh`
**without flags** (same work as the spike: render → flatten → hysteresis threshold → G4), ImageMagick 7.1.2, poppler.

## 1. PDF layers (#28)

### Choice

| Layer | Crate | Licence | Why |
|---|---|---|---|
| Read + page geometry | `hayro` 0.7 (`hayro-syntax`, `hayro-interpret`) | MIT OR Apache-2.0 | One pure-Rust crate for parsing, image decoding (DCT/JPEG, CCITT, JBIG2, JPX, Flate) and rendering. `/Rotate`, crop box and colour spaces are handled once for both paths |
| Embedded image directly | `hayro-interpret` `Device` trait (own ~50 lines: `Probe`) | same | The interpreter runs the page against a device that only records draws. A page that draws exactly **one** raster image, with nothing else, axis-aligned (any quarter turn/flip), scale within 1 % of the target dpi and covering the page, is decoded directly and mapped onto the page grid. Anything else is rendered |
| Render (fallback) | `hayro` (`vello_cpu`, CPU only) | MIT OR Apache-2.0 | Pure Rust, no DLL to ship. Pixel size rounded up like `pdftoppm` (e.g. 481.89 pt → 2008 px at 300 dpi), so page sizes match |
| G4 encode | `fax` 0.3 | MIT | Pure Rust, 26 M downloads, part of pdf-rs |
| PDF write | `pdf-writer` 0.15 (typst) | MIT OR Apache-2.0 | Low-level writer: one `/CCITTFaxDecode` (`K -1`) image per page, `MediaBox` = px × 72 / dpi |

Whole dependency tree (76 crates, `cargo tree`): only MIT, Apache-2.0, BSD-2/3, Zlib, Unicode-3.0, BSL-1.0 and 0BSD,
all permissive and compatible with the project's AGPL-3.0; nothing forces a licence change. Release binary 6.6 MB,
no system libraries beyond libc.

### Rejected / not used

| Option | Licence | Status (crates.io, 2026-09) | Why not |
|---|---|---|---|
| `lopdf` | MIT | 0.45, active | Would do reading and image extraction by hand (walk content stream, match `cm` + `Do`, decode every filter/colour space ourselves), and we would still need a renderer. hayro already does all of that. Added, then removed without measuring |
| `zune-jpeg` | MIT/Apache/Zlib | active | Only needed with lopdf; hayro decodes JPEG itself |
| `pdfium-render` | MIT OR Apache-2.0 (+ pdfium: BSD-3/Apache) | 0.9.4, active | Would need a pdfium binary shipped per OS (`.dll`/`.dylib`/`.so`), against the single-binary goal. hayro was fast enough, so it was **not measured** (unverified) |
| `mupdf` | AGPL-3.0 | 0.8, active | Licence fits, but it is C behind bindings (needs a C toolchain, harder Windows builds). Not measured |
| `pdf` (pdf-rs) | MIT | 0.10 | Parser only, no renderer; same argument as lopdf |
| `printpdf` | MIT | active | Higher-level writer. `pdf-writer` is smaller and gives direct control over the CCITT stream |

### Measurements

**Page count and page size vs bash** (`tests/measure.sh` on both outputs, per page): identical on all five scans that
produce a file (`ren pdf` is "not smaller" in both). Ink box within 20 ‰ on every page except `sidste side …` p4 and p6
(top edge 8 ‰ vs 42 ‰), see "Dithered 1-bit pages" below. Footer flag identical everywhere.

**Output size, same 1-bit input** (the bash output fed back through the spike: CCITT is decoded by hayro, the
bitmap goes through flatten/threshold unchanged, then gets re-encoded). The bitmaps come out **bit-identical** (`pdfimages` +
`magick compare -metric AE` = 0 on all 42 pages):

| Scan | bash | Rust | Δ |
|---|---:|---:|---:|
| flerspaltet | 521 632 | 514 824 | −1.31 % |
| ren-side | 54 476 | 53 327 | −2.11 % |
| ryg-side | 104 202 | 103 053 | −1.10 % |
| sidste side | 1 098 820 | 1 092 744 | −0.55 % |
| skewed | 1 841 999 | 1 825 288 | −0.91 % |

**Output size, full pipeline from the original scan** (different JPEG decoder, box-blur approximation of the σ 30
Gaussian, see #29): flerspaltet −2.2 %, ren-side −4.5 %, ryg-side +2.5 %, skewed +1.3 %, **sidste side +13 %**. The +13 %
comes from two pages only (below). The JPEG pages there are +5 %.

**Time** (whole file, wall clock / CPU):

| Scan | pages | bash wall | Rust wall | bash CPU | Rust CPU |
|---|---:|---:|---:|---:|---:|
| skewed | 23 | 77.8 s | 0.93 s | 289 s (12.6 s/page) | 6.3 s (0.27 s/page) |
| sidste side | 8 | 25.8 s | 0.36 s | | |
| flerspaltet | 9 | 32.2 s | 1.00 s | | |
| ryg-side | 1 | 11.3 s | 0.17 s | | |
| ren-side | 1 | 7.3 s | 0.08 s | | |

That is about **84× faster wall-clock and 46× less CPU** per page. Bash runs 4 pages at a time; the spike runs one
thread per core. Per page on `skewed` (300 dpi, 2480×3508): get image 51 ms, flatten + threshold 287 ms, G4 9 ms. The
image ops, not PDF I/O, are now the cost; #29 decides whether they stay this cheap once crop/split/deskew are added.

**Render vs extract, 100/150/300 dpi** (ms per page for the "get" stage; peak RSS of the whole run; `pdftoppm -gray` for
page 1 as reference):

| Page | dpi | path | get | RSS | pdftoppm |
|---|---:|---|---:|---:|---:|
| ryg-side (scan, 300 ppi JPEG) | 100 | render | 22–30 ms | 72 MB | 64–85 ms, 13 MB |
| | 150 | render | 27 ms | 94 MB | 75 ms, 16 MB |
| | 300 | image | 28 ms | 178 MB | 143 ms, 35 MB |
| | 300 | render (forced) | 70 ms | 186 MB | |
| ren-side (vector) | 100 / 150 / 300 | render | 8 / 9 / 15 ms | 16 / 28 / 93 MB | 22 / 24 / 31–46 ms |
| flerspaltet (vector, 9 pp) | 100 / 150 / 300 | render | 14 / 17 / 37–63 ms | 131 / 296 / 1110 MB | 44 / 49 / 59 ms |

- hayro renders as fast as or faster than poppler here. A 300-ppi scan at a lower dpi is rendered (downscaled), not
  extracted, which is correct.
- Memory is dominated by the spike's f32 flatten buffers (≈ 120 MB per page in flight at 300 dpi, × threads), not by
  the reader (an RGBA page at 300 dpi is 35 MB). With 10 threads `skewed` peaks at 1.5 GB. Cheap to cut (u8/u16 buffers,
  or cap threads) if it matters; not done in the spike.
- **Can a low-dpi render be reused?** No need for a second render in either direction: getting the 300-dpi page costs
  30–70 ms, so the analysis image for #29 should be a downsample of that one page (one pass, cheaper than one of the six
  box-blur passes in the flatten; derived, not timed separately). A 100-dpi render cannot be upscaled for the final image.
  Rendering twice (100 for analysis + 300 final) costs 22–30 ms extra per page, affordable but pointless.

**Tests with `BIN=`:**
- `tests/synth.sh`: `flatten` PASS; `spread`, `pagenumber`, `stripe`, `wide-table`, `dark-band` FAIL ("no output
  written": they need `--crop`/`--split`, #29). All six pass since #29 (section 2).
- `tests/real.sh --split off`: runs through all six, FAIL on the facts because they were recorded with all four flags
  (sizes/page counts after crop+split). The per-page comparison against bash without flags is the one above.
- `cargo test`: round trip bitmap → G4 → PDF → hayro → extraction → same bitmap and page size. Shown to fail with the
  G4 polarity flipped and with the fit check broken.

### Dithered 1-bit pages (solved in #29, section 2)
`sidste side …` p4 and p6 are CCITT images from the scanner, 3512×2480 (4 px wider than A4 at 300 dpi), with the dark
scanner band **dithered**. poppler downsamples them to 3508 px with a filter, which smears the dither into grey, and
flatten then blanks the band's middle. The spike maps them 1:1 (nearest-neighbour, dropping 4 columns), so the
dither dots survive: same text, but 320/340 kB instead of 251/264 kB per page and an ink box reaching the top. hayro's
render path gives byte-identical output to extraction here, so it is not a reader difference. Options for #29: a light blur
on 1-bit sources before flatten, or leave it to `--crop` (the band is exactly what #23 removes). Not decided here.

### Portability
- macOS: built and measured here. Linux and Windows: `.github/workflows/rust.yml` runs `cargo test --release` on
  ubuntu, macOS and windows-latest (see the PR's checks for the result). No system packages are needed; hayro,
  fax and pdf-writer are pure Rust (hayro has `#![forbid(unsafe_code)]`).
- The default path needs no brew/apt installs. Only the optional `--rotate` (Tesseract) remains, see #30.

### Verdict for #28
Floor and target met: page count and page size identical, size −0.5 … −2.1 % at the same 1-bit input, ≈ 46× less CPU per
page, one static-ish 6.6 MB binary, all licences permissive. Stack: **hayro (read, extract, render) + fax (G4) +
pdf-writer (PDF)**.

## 2. Image ops and pipeline order (#29)

### What was built
All of `bitonalpdf.sh` text mode is ported in `rust/src/main.rs`, with **no new crates**: every op is plain Rust over a
`Vec<f32>` page (0 = black … 1 = white).

| Op | How | Bash equivalent |
|---|---|---|
| Flatten | pixel ÷ Gaussian(σ 30) of the page, written out (no compose enum, #22). Gaussian: exact kernel on a ¼-size copy with a 4σ border of replicated edge pixels, bilinear back up. Max error vs an exact full-size Gaussian 0.03 % (`blur_matches_exact`) | `-blur 0x30` + `Divide_Dst/Src` |
| Threshold | #20 hysteresis rule per pixel (< 60 %, or < 75 % with a seed < 45 % within radius 2) | `-threshold`, `-morphology Dilate Disk:2` |
| Crop box | literal port of the `content_box` awk (density rows, then columns inside the row range, runs, band rule, near-extension, pad), including 8-bit quantisation of densities and awk's integer truncations | `content_box` |
| Gutter | per-column dark count over 48 bands (lenient grey < 230), widest ink-free run, valley fallback | `ink_profile` + awk |
| Plan | median single-page box, median gutter fraction (a real median, see #40), blank-page mirroring, one canvas = largest box | between passes |
| Deskew | own projection-profile estimator (Postl): coarse 0.25° over ±10°, then 0.025°, score = Σ(Δ row count)², middle of the best plateau; bilinear rotation onto the canvas | `-deskew 40%` |
| Orientation | Tesseract OSD via `tesseract … --psm 0` exactly as bash; optional, skipped with a warning if missing (#30 decides the replacement) | `rotate_page` |

Two passes like bash: pass A measures every page (the medians and the canvas need all pages), pass B gets the page
again, flattens it once, cuts, deskews each slot, centres it on the canvas, thresholds and encodes. Pass B re-decodes the
page (30–80 ms) instead of keeping 23 flattened pages in memory.

Tests: `cargo test` covers the G4/PDF round trip (#28), skew found and undone (fails with the rotation sign flipped),
crop drops a band but keeps a page number, fast blur = exact blur. `tests/synth.sh` with `BIN=`: all six PASS, same
numbers as bash within 1–2 px (e.g. `spread` 354 vs 359 pt wide, `wide-table` 106 vs 109 pt high).

### Reference: bash has a bug on `skewed` (#40)
The committed `skewed.facts` say every output page is 791.52 pt wide: bash's `median()` truncates the median gutter
**fraction** to `int`, which is 0 for an even count (22 trusted gutters on `skewed`), so page 22 is split at x = 0 and
the canvas becomes the whole spread. All parity numbers below are against bash **with that one line fixed** (scratch copy;
the other four scans give byte-identical facts with and without the fix). `tests/real.sh` got `SIZE_TOL=<pt>` for a
second implementation, because an exact page-size match needs the same pixel on every page's box edge.

### Parity with bash (all four flags)
Tolerances: page size ≤ 3 pt (12 px, 0.6 % of the page), ink box ±20 ‰ (`FACT_TOL`), footer flag equal, page count equal.

| Scan | pages | bash wall / CPU | Rust wall | size bash → Rust | pages outside tolerance |
|---|---:|---:|---:|---:|---|
| skewed | 23 → 46 | 177 s / 638 s | 5.2 s | 1 876 → 1 859 kB (−0.9 %) | 4: p1, 39, 41, 43 |
| sidste side | 8 → 16 | 49 s / 171 s | 2.2 s | 797 → 769 kB (−3.5 %) | 4: p1, 7, 8, 11 |
| ryg-side | 1 → 2 | 19 s | 1.4 s | 101 → 101 kB (−0.2 %) | 0 |
| ren-side | 1 → 1 | 10 s | 0.7 s | 53 → 51 kB (−4.2 %) | 0 |
| flerspaltet | 9 → 9 | 44 s / 131 s | 1.5 s | 514 → 506 kB (−1.6 %) | 0 |
| ren pdf | 1 | 19 s | | not smaller, no file (both) | |

Page counts, split decisions and page sizes (within 0.24 pt = 1 px) match on every scan. The per-page geometry was also
compared directly (bash `meta/` files vs the Rust pass A, px at 300 dpi): crop boxes within ±3 px on 40 of 42 pages,
gutters within ±5 px on 29 of 31 double pages; `sidste` p7 is 9 px off, `sidste` p4 (a dithered 1-bit page) 38 px, still
inside its 150 px wide blank spine gap. The 8 pages outside tolerance, each looked at:
- **skewed p39/41/43** (left pages): a speck in the right margin. The gutter is 4–5 px further right than in bash, so the
  slot takes 4–5 px more of the spine. Text identical.
- **skewed p1** (left half of the first spread, a nearly blank page): the text block of the right page starts at column
  2091 in both; the 8 % near-window reaches back to column 1811, which has density 1/255 in bash and 3/255 in Rust. Rust
  extends the box over a 250 px sliver of the left page's line ends, bash mirrors the right page's width. Both cut the few
  lines on that page; bash shows more of them. A cliff of the crop rule, not of the port (below).
- **sidste p1** (blank facing page): 2–3 specks instead of 1.
- **sidste p7/8/11** (the dithered 1-bit scan pages): Rust is **cleaner**: no dither dots along the edge, no spine line,
  same text, page numbers 18/19/22 present; 49–52 kB instead of 55–59 kB per page.

Thin strokes: black fraction on `skewed` pp. 9–12 bash vs Rust 10.66/10.65 %, 8.82/8.88 %, 9.83/9.83 %, 8.05/7.67 %
(the last: fewer margin marks); a 500×160 px crop of body text is indistinguishable.

### What it took to get there (goes into lessons)
1. **The blur kernel decides crop edges.** The #28 flatten (3 box blurs) was up to 9 % off the Gaussian at a dark page
   edge. That moved a few rows of band fringe across the 0.55 / 0.004 density limits and flipped the band rule: top edge
   72–87 px off on three `sidste` pages. An exact Gaussian fixed all crop boxes
   to ±1 px but costs ~2.5 s per blur. The low-res exact Gaussian costs less than the box blur, but it first repeated a
   3-row block average at the edge instead of the edge pixel (1 % off, still two flips). Replicating the edge pixels
   *before* downsampling fixed that.
2. **The port of the crop logic is exact; its inputs are not.** `axis_box` and the bash awk give the same result on the
   same density profile (`flerspaltet` p1: 209/674 from both). Every remaining difference comes from a density one or two
   levels of 255 apart sitting right on a limit (0.03, 0.004 after 8-bit quantisation, the 8 % window, the 10 % band
   zone). Rendered vector pages (hayro vs poppler anti-aliasing) shift densities by a few levels, enough for a column at
   0.029 vs 0.032.
3. **Porting ImageMagick's `-resize` did not help.** A Lanczos port matched IM's 48-band profile better per pixel
   (mean error 1.2 vs 2.3 grey levels, half the flips at 230), but on `skewed` p21 it tipped the "widest ink-free run" to
   a run 243 px away. The area average stays within 5 px everywhere, so it stays.
4. **Dithered 1-bit pages (#28):** poppler resamples a 3512-px 1-bit page to 3508 px with interpolation (12 % of its
   pixels become grey), which turns the dithered band into grey that the flatten removes. Rust now blurs (σ 0.8) a
   1-bit image **only when it has to be resampled**, and only for the output image. Measured on the blurred copy, the
   band rule dropped `sidste` p4's last run and its page numbers 18/19. 1-bit pages at exact size (e.g. our own output
   fed back) are untouched, so a 1-px line is never softened.
5. **Skew estimator:** sum of squared counts plateaus for thick lines (1.925° found for 2°); near the true angle the bins
   only change once a line end moves a whole pixel, so the search takes the middle of the best plateau and measures x
   from the centre. On `skewed` it matches ImageMagick's `-deskew 40%` within 0.05° (IM reports the opposite sign):
   p5 0.225/0.050 vs 0.224/0.028, p12 0.80/0.75 vs 0.81/0.70, p20 0.050/0.35 vs 0.028/0.34.

### Pipeline order: A/B (the four questions)
All variants on all scans against the same facts, same binary, knobs as env vars (`main.rs` header). Pages outside
tolerance / total output size / `skewed` wall time (with Tesseract):

| Variant | skewed | sidste | ryg | ren | fler | size | skewed time |
|---|---:|---:|---:|---:|---:|---:|---:|
| **base**: flatten once, analysis at 300 dpi, deskew per slot | 4 | 4 | 0 | 0 | 0 | 3 209 kB | 5.2 s |
| `SLOTFLAT` (flatten each slot after cut/deskew, as bash) | 4 | 2 | 0 | 0 | 0 | 3 208 kB | 5.1 s |
| `DESKEW_FIRST` (whole-page angle, straighten before measuring) | 4 | 4 | 0 | 0 | 0 | 3 209 kB | 5.7 s |
| `INKGUTTER` (gutter from the 60 % ink map) | **46** | **16** | 0 | 0 | 0 | 3 210 kB | 5.3 s |
| `ADPI=150` (analysis at 150 dpi) | 5 | **16** | **2** | 0 | 0 | 3 207 kB | 6.2 s |
| `ADPI=100` | **46** | **16** | **2** | 0 | 0 | 3 206 kB | 5.3 s |

1. **Flatten once on the full page: yes.** Same parity as per-slot flattening. The two `sidste` pages it "loses" are the
   ones where it is cleaner than bash: the slot's clamped cut edge makes bash keep a spine line. Same time. It removes a
   design wrinkle: analysis and output see the same flattened image, so the crop is measured on what is output.
2. **Deskew before measuring: no.** No page changed. On `skewed`, 11 of 23 spreads have halves skewed > 0.5° apart,
   mostly in **opposite directions** (p13: +1.38° / −1.15°, p11: +0.85° / −1.0°: the book fans open), so one
   whole-spread angle cannot replace per-page deskew, and the gutter was found on 22/23 spreads without it. Skew here is
   at most 1.4°. It would be worth it on scans with > 2° common skew where the gutter search fails.
3. **Analysis at lower resolution: no.** It breaks parity (150 dpi: `ryg-side` = `sidste` p8 loses its page numbers,
   bottom edge 192 px up, because the last text lines become a separate run in the bottom 10 % band zone; `skewed` p20
   right edge 84 px in, `sidste` p4/p6 bottom 56/24 px; 100 dpi changes the canvas) and saves nothing: with the fast blur the whole pass A analysis is ~0.2 s/page CPU. The #28 question
   ("reuse a low-dpi render?") is moot: get the page once at 300 dpi.
4. **Threshold once: half.** One flatten serves analysis and output, and crop density uses its 60 % map. The gutter must
   keep its lenient grey level: reading it off the 60 % map (dark = > 10 % ink in a band) moves gutters on every spread
   (canvas changes, 62 pages out). The output threshold has to come after the deskew rotation (rotating a bitmap
   loses thin strokes), so it is a second threshold by design. It is the cheap part anyway (the blur was the cost).

### Speed and memory
`skewed` (23 spreads, 300 dpi, M4, 10 threads): **5.2–5.9 s wall, 39 s CPU with `--rotate`**, of which Tesseract OSD is
~1.3–1.5 s per page (pass A 4.0–4.5 s). **Without `--rotate`: 2.0 s wall, 15.5 s CPU** (0.67 s CPU per spread).
Bash: 177 s wall, 638 s CPU. That is **30× wall / 16× CPU with OSD, 88× wall / 41× CPU for the image ops**. Per page
in pass B: get 76 ms, cut + deskew + flatten + threshold + G4 of two slots ~380 ms. Peak RSS 1.2 GB (f32 buffers × 10
threads); u8/u16 buffers or a thread cap would cut it, not done. The orientation step is now the bottleneck, which is #30.

### Review: is this the right design?
**Recommended order** (what the spike now does, base variant):
get page at output dpi (extract or render) → orientation (quarter turn) → **flatten once** → ink map → crop box + gutter
(full resolution) → document plan (medians, canvas) → per slot: cut from the flattened page → **deskew per slot** →
centre on canvas → hysteresis threshold → G4.

Prior art agrees on the main points: Scan Tailor Advanced's stages are Split Pages → Deskew → Select Content → Margins →
Output, with illumination normalisation and binarisation only in Output (its README). So split before deskew,
deskew per page, binarise last. unpaper's man page does not state a full order (filters apply before deskew, `--wipe`
"after deskewing and before automatic border-scan"). The OCRmyPDF order in lessons.md stays unverified.

**What a Rust design should drop or do differently from bash:**
- Flatten three times (crop, gutter, slot) → once. Threshold three times → one analysis map + one output threshold.
- Library compose semantics and per-page process spawning → gone (the 40× is mostly this plus the blur).
- The integer median of a fraction (#40) → a real median.
- Keep: two passes (document medians/canvas), analysis at output resolution, per-slot deskew, the lenient gutter level,
  binarise last.

**What should change next (heuristics, not the port).** The rules are full of hard cliffs: densities on 1/255 steps
compared with 0.004 and 0.03, an 8 % window, a 10 % band zone, "widest run" between near-equal runs. Every remaining
difference above is one of them tipping. A port cannot be more stable than its rules. Candidates, each to be measured
on the suite:
- The near-extension should require a small run (like the main block), not a single column at 2/255.
- The band rule should require band-like evidence (full-width, or dense before flattening, see lessons idea 1), not
  "any run in the outer 10 % behind a gap". It has now dropped text lines/page numbers twice in variants.
- Choose the gutter by score (width × emptiness × closeness to the document median) instead of "widest run", so two
  near-equal candidates don't flip.
- Canvas = largest box still lets one bad page enlarge all (open in lessons).

**Evidence that would change the recommendation:** scans with > 2° common skew where the gutter search fails
(→ estimate a page angle first, then per-slot deskew as now); documents with hundreds of pages where RSS matters
(→ u8 buffers, cache flattened pages or cap threads); a thin-stroke loss at other dpi (→ scale the blur and radius
with dpi; bash uses 30 px regardless of dpi, the spike keeps that for parity).

### Verdict for #29
Image ops are cheap and exact enough in plain Rust: no image crate needed, same page counts, splits and sizes as bash on
all real scans, 8 of 74 pages outside tolerance (all looked at: specks, one blank-page cliff, and three pages cleaner than
bash), −0.2 … −4.2 % size, 41× less CPU for the image work. Tesseract OSD is now 75 % of the run time (#30).

## 3. Orientation without Tesseract (#30)

### Result
`--rotate` in the spike now uses its own detector (`osd_own` in `main.rs`, ~60 lines, no new crate, no system
dependency). `BITONAL_OSD=tesseract` brings back the old Tesseract call (kept as a fallback and for the comparison
below). On the six local scans the output PDFs are **byte-identical** with both detectors (`cmp`), so the parity of
section 2 (`BIN=… SIZE_TOL=3 tests/real.sh`: same 4 + 4 pages outside tolerance on `skewed`/`sidste`, same `skewed.facts`
mismatch from #40) is unchanged. `cargo test` has `orientation_found_for_all_turns` (synthetic lines with ascenders and
descenders, all four turns; shown to fail with the direction sign flipped).

### How it works
Both stages look at a flattened darkness map (1 − flatten, the #29 flatten) of a downsampled copy, in `tile × tile`
squares (equal sample count for rows and columns), central 90 % of the page only (scanner borders).
1. **Axis (0/180 vs 90/270).** Text lines make row profiles sharper than column profiles: score = Σ(Δ profile)² / Σ profile²
   summed over all tiles, rows vs columns. Done at **1/8 size (37 dpi)**, tile 48 px.
2. **Direction (0 vs 180, 90 vs 270).** Per tile profile along the text axis, cut into lines at local minima of the
   smoothed profile. Per line, take the rows ≥ 50 % of the line's peak as the x-height band; **ink above the band
   (ascenders, capitals, digits) minus ink below it (descenders)**, normalised. Positive = upright. At **1/4 size (75 dpi)**.
3. Confidence = axis ratio × |direction score|; below 0.25 the page is left as it is (like Tesseract's confidence < 1).

### Accuracy
Test set: every page of the six local files (47 pages, `ren pdf` has 5), first turned upright by Tesseract, then
rotated with `rot90` by 0/90/180/270 → **188 cases**, expected answer = the inverse turn. `bitonalpdf --osd-eval file.pdf`
prints one line per case (`EVAL_TESS=1` adds Tesseract's answer and time).

| Detector | correct | notes |
|---|---:|---|
| **own (default: k 8/4, tile 48)** | **186 / 188** (98.9 %) | 0 wrong axis, 2 wrong direction, both on `ren pdf` p2 (below) |
| own, direction at 1/2 size | 188 / 188 | (see the grid; no reason to pay for it) |
| Tesseract | 188 / 188 | **by construction** (it defines "upright"), so this only shows it is rotation-consistent |

Per file, own default: `skewed` 92/92, `sidste side` 32/32, `flerspaltet` 36/36, `ryg-side` 4/4, `ren-side` 4/4, `ren pdf` 18/20.
The 23 + 8 + 1 pages that Tesseract reads as 270° (scanned spreads lying on their side) are found as 270°. The local
suite has 0 and 270 as real cases; 90 and 180 exist only as pixel-exact `rot90` copies. That is a weaker test than a
real scan fed upside down (no scanner/feeder asymmetry, no second resampling), **so 98.9 % is an upper bound**.

The two misses: `ren pdf` p2 is a portrait page with a caption line on top and a wide table lying sideways below it.
The content is mixed; Tesseract follows the caption (0°), the detector follows the (much larger) table and turns it
90°. Either answer is defensible, and the "truth" here is Tesseract's. Confidence 0.12 / 0.23; the 0.25 gate would have
returned 0 for both (right for one of them), but the gate was set after seeing these numbers, so it is not validated.

### Time
Per detection, all pages in parallel on 10 threads (so both are measured under the same load): **own 102 ms, Tesseract
1 126 ms** (11×; own includes two flattens and the downsampling). Whole runs, all four flags, wall clock:

| Scan | pages | own | Tesseract | (no `--rotate`, #29) |
|---|---:|---:|---:|---:|
| skewed | 23 | 2.31 s | 5.09 s | 2.0 s |
| sidste side | 8 | 0.96 s | 2.42 s | |
| flerspaltet | 9 | 0.80 s | 1.60 s | |
| ryg-side | 1 | 0.55 s | 1.40 s | |
| ren-side | 1 | 0.34 s | 0.69 s | |

So orientation now costs ~0.3 s on `skewed` instead of ~3 s, and the 30× (with OSD) of section 2 becomes ~75× against bash.

### What did not work (in the order tried)
All numbers: the same 168 cases (without `ren pdf`), before the final design.
1. **Row vs column sharpness on the 60 % ink map at 100 dpi, whole-page profiles:** 83 / 168. 43 wrong axis (skewed
   spreads: two halves skewed differently smear a page-wide row profile) and 42 wrong direction.
2. **Same in 8 vertical strips:** 43 / 168, *worse*: a short strip has a noisier column profile, and noise counts as sharpness.
3. **Square tiles (fixes the noise bias), ink map:** 70 / 168. A thin stroke averaged to 1/3 size is grey, mostly above
   the 0.6 line, so the ink map lost the text. **Using the flattened darkness itself instead of a threshold:** 99 / 168.
4. **Lower resolution for the axis** is what fixed it: at 1/8 size the axis was right in 168/168 (1/6 with tile 64: 4 wrong;
   1/4 with tile 96: 27 wrong). At 100 dpi the glyph stems make the column profile as "sharp" as the line structure;
   at 37 dpi only the lines survive.
5. **Direction from "steps down outweigh steps up"** (baseline crisper than the x-height top): right on every scan
   (`skewed`, `sidste`, `ryg`, `ren`), **inverted on `flerspaltet`** (rendered vector text): 26–34 of 36 wrong at every
   resolution from 1/2 to 1/8. Why was not investigated *(unverified: crisp vector edges make both edges equally sharp)*.
   That it worked on the scans might be a property of scanned edges rather than of the letters, so it was dropped.
   The ascender/descender feature is right on all of them from 1/4 size (at 1/8: 166/168).
6. **Sensitivity of the final design** (correct of 188, `BITONAL_OSDKA/KD/TILE`):

| axis 1/k, direction 1/k, tile | correct | | axis, dir, tile | correct |
|---|---:|---|---|---:|
| 8, 4, 48 (default) | 186 | | 16, 4, 24 | 186 |
| 8, 2, 48 | 188 | | 12, 4, 32 | 186 |
| 8, 8, 48 | 186 | | 8, 4, 32 / 64 | 184 / 186 |
| 6, 4, 64 | 180 (6 wrong axis) | | **6, 4, 32** | **106** (78 wrong axis) |

The tile has to be sized against the line pitch at the axis resolution (12 pt lines: 8 px at 1/6, ~4.5 px at 1/8);
1/6 with a tile that holds four lines breaks. The default sits on a plateau (everything from 1/8 to 1/16 works), which
is the only reason to trust it beyond these 47 pages.

### Not done / open
- **Real 90/180 scans, other scripts and fonts.** All five documents are Danish/Latin text, four of them one scanner and
  one book. The direction rule is a statement about Latin letter shapes (ascenders more frequent than descenders) and
  will not carry over to Cyrillic, Greek or CJK; the axis rule may. No non-Latin sample exists locally.
- **Pages with little text** (figures, blank facing pages, title pages): the confidence gate is the only protection;
  tested on photographed material in #45 (below: 0 of 77 upright pages turned after the `OSD_DARK_MAX` fix). A whole-document vote (idea 3 in the issue: run on a few pages, apply the majority) was not needed for
  accuracy on this suite, and would be the next step if single pages turn out unreliable. It is a plan change, not a detector
  change, and would make a genuinely mixed document wrong.
- **Mixed orientation on one page** (`ren pdf` p2) is unsolved by design: one quarter turn per page.
- `ocrs` and other OCR crates (idea 4) were **not evaluated**: the hand-made detector was accurate and ~11× faster than
  Tesseract already, and an OCR model would bring a model file and a heavier dependency, against the purpose of #30.
- Tesseract stays available (`BITONAL_OSD=tesseract`); nothing here retires it for documents outside this suite.

### Verdict for #30
Own detector as the default for `--rotate`: no system dependency, 186/188 on the local suite (misses: one ambiguous
page), identical output to Tesseract on all six scans, 11× faster per page. The weak part is coverage of the test, not
the numbers: five Latin documents and synthetic 90/180.

### Photographed pages (#45)
Section 4.3 found the detector turning 12 of the 77 upright pages of f1–f7 (photographed spreads, pictures, colour).
New case set: those 77 pages, truth = as they are (all upright, 4.3), and their `rot90` copies → **308 cases**
(`EVAL_UPRIGHT=1 bitonalpdf --osd-eval fN.pdf`: the page is taken as upright instead of asking Tesseract). The eval line
now also prints the two parts of the confidence, `ratio` (axis) and `asym` (direction), so a miss can be assigned to a stage.

**Cause (measured on f7 p5, per tile and per line).** The text tiles gave direction scores of about ±0.1 each; the bottom
row of tiles, where the dark book edge/table is still inside the central 90 %, gave about −1.5 each and decided the page.
There the "line" between two profile minima is the edge itself: peak darkness 0.71 against 0.20–0.23 for the text lines,
running into the tile border, so its fading side counts as a huge descender. The same happens in the axis stage with
pictures and dark picture borders (f1 p5, p22): their edges outweigh the few text lines. So the issue's first candidate
(dark surround) and third (pictures read as text) are one mechanism. Lamp glow was not examined on its own: the fix below
does not address it and f7 (the glow document) has no upright page turned after it.

**Fix: one constant, `OSD_DARK_MAX` = 0.4.** Flattened darkness above it is not text. The axis stage leaves out every tile
whose densest row or column is above it (for rows and columns alike, so the sample counts stay equal); the direction stage
leaves out every line whose smoothed peak is above it. Nothing else changed: same stages, resolutions, gate 0.25.

| (gate 0.25 = what `--rotate` does) | before | after |
|---|---:|---:|
| old suite (188, Tesseract truth) | 186 | **186** (same two `ren pdf` p2 misses) |
| new, upright pages turned (of 77) | 14 * | **0** |
| new, turned copies right / left as they are / turned wrong (of 231) | 101 / 111 / 19 | **150 / 80 / 1** |
| new, detector alone without gate (of 308) | 178 | 275 |

**Whole runs** (all four flags, f1–f7): main turns f1 p5/p22, f2 p10/15/16 and f7 p1/2/4–8 (the 12 of 4.3); after the fix no page is
turned, the page counts are back at the Tesseract baseline (f1 50, f7 18), and **all seven outputs are byte-identical to
`BITONAL_OSD=tesseract`** (`cmp`). `BIN=… SIZE_TOL=3 tests/real.sh`: same output as main (same two known `skewed`/`sidste` facts
failures), the five written PDFs byte-identical. A page where every tile is too dark for text now returns 0° with confidence 0
(without the guard 0/0 would give a NaN ratio, which `min(1e3)` turns into full confidence); no page of either suite hits it.

\* 12 in the real runs of 4.3; two more sit at 0.25 when the confidence is rounded to two decimals as in the eval line.
The one wrong turn left is f2 p1 at 90° (a cover). Per file after the fix, turned copies right (of 3 × pages): f1 62/75, f2 39/54,
f3 24/27, f4 15/15, f5 9/21, f6 0/12, f7 1/27. Upright pages turned by gate: 0 at every gate from 0.2 to 0.6 (the old suite stays
at 186 for gates 0.2–0.3, 184 at 0.35). The cap: 0.3, 0.35 and 0.4 all give 0 upright turned; old 184 / 186 / 186 and turned copies right
162 / 159 / 150. 0.35 and 0.4 both hold 186 for gates 0.2–0.3; 0.4 was kept as the value furthest from the text peaks. Detection time unchanged (same run, same load: 202 vs 205 ms old, 281 vs 274 ms new).

`cargo test`: `direction_ignores_dark_edge` (one tile profile: a text line plus an f7-shaped edge; fails without the cap).
`orientation_found_for_all_turns` now draws each synthetic letter as two 2-px stems instead of a solid block: solid
blocks at ~60 % coverage are darker than any text line (0.2–0.3 at 75 dpi) and were capped away.

What it does not do: on photographed material the detector is now **safe but not useful for real turned pages**:
of the 231 turned copies, 80 are left as they are (f6 0/12, f7 1/27 right). A photographed book that actually needs turning
still needs `BITONAL_OSD=tesseract`. The cap also means very dense or bold text (line peak above 0.4 at 75 dpi) no longer votes;
the old suite did not change, but a page of heavy headings could fall back to 0°.

What did not work (all at gate 0.25 or raw, same 188 + 308 cases):
- **Detrending each line** (measure above/below the straight line between the two bounding minima, against glow gradients):
  worse everywhere, new 123/308 raw (from 178), old 151/188.
- **Skipping lines cut by the tile border** (no minimum at one end): new 198/308, f7 still 6/36; the edge line is not always cut.
- **Skipping tiles by mean darkness** in the axis stage (0.1–0.3): f1 raw 47–55 of 100 (from 48); the picture tiles are not dark on
  average, their edges are. The max-row/column rule above is what fixed f1 (raw 48 → 94 of 100).
- **Raising the gate** alone: 0 upright turned needs 0.45, which costs the old suite 9 cases (177/188).
- **A whole-document vote** was not tried: f7 had 8 of 9 pages wrong the same way, so a vote would have followed the error.

## 4. B4 groundwork: material, baseline, codecs, signals (#43)

Measurements for #38 (mixed output). **Data only: no thresholds, no rules, no design.** The seven PDFs are not open source
and live git-ignored in `tests/real/new pdfs dont upload/`; nothing derived from their pixels is committed (no pages,
crops, contact sheets), only numbers and words. Same machine as above (M4, 10 cores, release build). Numbering `f1…f7` =
alphabetical order of the file names, which is also the order `*.pdf` expands to.

Reproduce (all outputs land in the ignored folder):
```
cd rust && cargo build --release && cd ..
D="tests/real/new pdfs dont upload"
rust/target/release/bitonalpdf --detect-eval "$D"/*.pdf > det.tsv          # 6–7 s wall, byte-identical on a second run
python3 tests/detect-join.py det.tsv "$D"                                  # joins with "$D"/<name>.labels, prints the signal tables
python3 tests/codec-table.py crop.png ...                                  # codec table; --viewers crop.png for the viewer test
```

### 4.1 Inventory (task 1)
77 pages, 7 files. TSV (per page: size, embedded image codec/colour space/ppi, gutter decision, label) is `work/inventory.tsv`
in the ignored folder; summary:

| # | What it is (owner's description was "photos, diagrams, a coloured cover") | pages | page size (pt, before `/Rotate`) | embedded images | gutter logic (`--split auto`) |
|---|---|---:|---|---|---|
| f1 | Religion textbook, photographed spreads, many full-page paintings/photos, comic, map, orange box | 25 | 728×1032, `/Rotate 90`, 300 ppi | 23 RGB JPEG, 2 JBIG2 (1-bit) | 22 spread, 2 single (full-bleed pictures), 1 double-shaped without gutter |
| f2 | Journal book: coloured cover + 17 text pages, one small diagram | 18 | 842×595 A4, `/Rotate` | 5 RGB JPEG (cover + 4 text pages), 13 CCITT (1-bit) | 18 single (one book page per scan, the neighbour page shows as a sliver) |
| f3 | Book chapter: coloured cover + 8 text pages | 9 | 596×842 (p1), 842×596 | p1 2 RGB JPEG, p2–9 grey JPEG + a thin second grey image (56–192 px wide strip) | p1 single, 8 spread |
| f4 | Textbook chapter, blue/orange page tabs and headings | 5 | 728×1032, `/Rotate 90` | 3 RGB JPEG, 2 CCITT | 5 spread |
| f5 | Magazine article: cover, diagrams, pie chart, photos, ad column, flatbed scan at 200 ppi, A4 portrait | 7 | 595×842 | 7 RGB JPEG | 7 single |
| f6 | Excerpt, photographed A3 spreads on a dark table, one sepia picture | 4 | 1191×842 | 4 RGB JPEG, 150 ppi | 1 spread, **3 double-shaped with no confident gutter** (left unsplit by the rule, split by the document median; fixed in #46, section 6) |
| f7 | Textbook chapter, photographed spreads, jacket, orange headings, pale-green boxes, orange lamp glow at the bottom of every photo | 9 | 728×1032, `/Rotate 90` | 9 RGB JPEG | 1 single (p2), 8 spread (incl. the jacket, p1) |

Totals: 51 RGB JPEG pages, 15 CCITT, 8 grey JPEG (each with a strip image), 2 JBIG2, 1 RGB + RGB. The owner's description ("one coloured
single-page cover, the rest double pages") fits f3 (cover p1, then 8 spreads); f2 has a cover but its other 17 pages are **single** book pages
(each scan shows one page and a sliver of its neighbour), and f5 is a magazine of single A4 pages. Gutter calls overall:
44 spread, 29 single, 4 double-shaped without a gutter. **Colour is not in every page:** 17 pages are 1-bit and 8 are grey,
so any signal based on chroma is exactly 0 on 25 of 77 pages, whatever their content (see 4.5, RGB-only tables).

### 4.2 Ground truth (task 2)
Method: `pdftoppm -r 30` contact sheets to see the file, then every page at 50 dpi with a 10 % grid drawn over it, boxes
read off the grid (good to about ±0.03 of the page, ≈ 90 px at 300 dpi; the sheets stay in the ignored folder).
Files: `<name>.labels` next to each PDF (page, classes, certain 0/1, regions `class:x0,y0,x1,y1` as fractions of the
rendered page, note). Convention: **only non-text regions are boxed**, everything else on the page is text/margin/
surround; a page's class set contains every class that has a region, plus `text` when there is body text (captions
alone do not count).

| | pages having the class | regions boxed (uncertain) |
|---|---:|---:|
| text (body text) | 70 (41 text-only) | – |
| picture (photo, painting, engraving) | 15 | 18 (6) |
| diagram (flat colour / line art: comic, map, pie chart, box diagrams, QR code) | 10 | 12 (6) |
| coloured-text | 13 | 12 (9) |
| cover | 4 (f2 p1, f3 p1, f5 p1, f7 p1) | 4 (2) |
| blank | 0 | – |

100 % of the pages are labelled; 29 pages mix classes, 28 of them have region boxes (f7 p7 has only coloured headings and
orange highlighted sentences inline, no block to box). **19 of 77 pages carry an uncertain label** (`certain=0`):
f1 7 (pp. 1 comic, 2 pale-green box, 8 dark plate, 13 cartoon, 15 old map, 17 grey engraving, 19 orange box),
f3 p1 (box edge of the cover), f4 p1, f5 p7, f6 p4 (sepia print) and f7 8 of 9 pages. What was hard:
- **`diagram` vs `picture` is a judgement** for comics, an illustrated old map, an engraving and a colour cartoon. Nothing in the
  brief defines the boundary; I called flat-colour drawings `diagram`, photographs and paintings `picture`, the grey engraving `picture`.
- **`coloured-text` is a spectrum.** In f7 colour is a typographic style on every page (orange highlighted sentences, blue/orange
  headings, pale-green boxes). Only block-sized regions are boxed; f7 p7 (and the inline colour on other f7 pages) is page-level
  and uncertain. In f4 the page tabs and running heads are coloured but tiny; not boxed.
- **Full-bleed pictures** (f1 pp. 8, 10, 12) run to the page edge and into the scanner surround; the box follows the picture, not the paper.
- f1 p5 and p7 are one painting/picture pair across both pages of the spread, boxed as one region.
- A QR code (f7 p8) is neither: boxed as `diagram`, uncertain.
- A dark-blue page with white text (f7 p2, left) is called `coloured-text`, uncertain (it could be `cover`-like).
- Boxes are ±0.03; tile labels (4.5) therefore skip tiles that are not ≥ 75 % inside one box.

### 4.3 Baseline (task 3)
All rows: **all four flags** `--rotate --crop --split auto --deskew`, 300 dpi (text) / 150 dpi (images, bash), threshold 60, one run each, machine
otherwise idle. `text` = the Rust spike (own orientation detector, default), `text + Tess` = the spike with
`BITONAL_OSD=tesseract` (see the orientation finding below; the corrected baseline), `images` = `MODE=images bitonalpdf.sh` with the same flags.
Wall/CPU from `/usr/bin/time -l` (CPU = user + sys; bash: 4 pages at a time, includes Tesseract and ImageMagick children).
"Sizes" = number of different output page sizes (1 = the `real.sh` check "all pages one size" holds); the page-count check
is the output count against the number of spreads found, see the notes.

The #40 median correction (see the reference section above) is now merged in `bitonalpdf.sh` (`DOUBLE_GFRAC`), and `skewed.facts` was re-recorded: bash and the numbers below no longer differ by that line.

| file | mode | pages in → out | in | out | out / in | sizes | wall | CPU | peak RSS |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|
| f1 | text | 25 → 48 | 31.39 MB | 2.65 MB | 0.085 | 1 | 4.0 s | 29.8 s | 2048 MB |
| f1 | text + Tess | 25 → 50 | | 2.67 MB | 0.085 | 1 | 8.4 s | 59.4 s | 1602 MB |
| f1 | images | 25 → 50 | | 8.63 MB | 0.275 | 1 | 70.1 s | 250.9 s | 1796 MB |
| f2 | text | 18 → 18 | 7.57 MB | 1.12 MB | 0.148 | 1 | 2.1 s | 16.6 s | 1353 MB |
| f2 | text + Tess | 18 → 18 | | 1.12 MB | 0.148 | 1 | 4.3 s | 34.4 s | 1431 MB |
| f2 | images | 18 → 18 | | 4.21 MB | 0.557 | 1 | 19.3 s | 68.9 s | 256 MB |
| f3 | text | 9 → 17 | 1.44 MB | 0.54 MB | 0.376 | 1 | 0.9 s | 7.0 s | 1248 MB |
| f3 | text + Tess | 9 → 17 | | 0.54 MB | 0.376 | 1 | 1.7 s | 13.5 s | 1019 MB |
| f3 | images | 9 → 17 | | **2.12 MB (bash writes no file: "Not smaller")** | 1.472 | 1 | 17.0 s | 53.1 s | 300 MB |
| f4 | text | 5 → 10 | 4.18 MB | 0.47 MB | 0.111 | 1 | 1.2 s | 5.1 s | 1261 MB |
| f4 | text + Tess | 5 → 10 | | 0.47 MB | 0.111 | 1 | 2.5 s | 10.4 s | 873 MB |
| f4 | images | 5 → 10 | | 1.61 MB | 0.386 | 1 | 17.3 s | 44.2 s | 142 MB |
| f5 | text | 7 → 7 | 3.41 MB | 0.34 MB | 0.101 | 1 | 0.9 s | 5.1 s | 1318 MB |
| f5 | text + Tess | 7 → 7 | | 0.34 MB | 0.101 | 1 | 1.6 s | 10.1 s | 1131 MB |
| f5 | images | 7 → 7 | | 1.30 MB | 0.382 | 1 | 8.9 s | 31.7 s | 159 MB |
| f6 | text | 4 → 8 | 1.83 MB | 0.38 MB | 0.208 | 1 | 1.3 s | 4.8 s | 1526 MB |
| f6 | text + Tess | 4 → 8 | | 0.38 MB | 0.208 | 1 | 2.2 s | 8.5 s | 1245 MB |
| f6 | images | 4 → 8 | | 1.42 MB | 0.778 | 1 | 13.9 s | 53.2 s | 292 MB |
| f7 | text | 9 → 17 | 6.66 MB | 0.63 MB | 0.094 | 1 | 1.6 s | 11.2 s | 1836 MB |
| f7 | text + Tess | 9 → 18 | | 0.63 MB | 0.094 | 1 | 2.7 s | 19.9 s | 2068 MB |
| f7 | images | 9 → 18 | | 2.96 MB | 0.445 | 1 | 29.8 s | 91.7 s | 599 MB |

Sums over the 7 files: input 56.5 MB; text 6.1 MB (0.11), images 22.3 MB (0.39); wall 12.0 s (text) vs 176 s (images), CPU 80 s vs 594 s
(text with Tesseract OSD: 23.4 s wall, 156 s CPU). Every `real.sh`-style check holds (one page size in every output). The f3 images value
comes from a scratch copy of `bitonalpdf.sh` with only the "not smaller" test removed (the pipeline itself is untouched); the real script writes nothing there.
**Correction (#38):** the `images` rows carry the bash median bug #40 on f1 and f7 (spread-wide canvas): with the one-line fix, f1 is 8.02 MB and f7 2.56 MB
(7 % and 14 % smaller), the other five are unchanged, sum 21.25 MB; section 5.6 uses the corrected numbers.

Things the table hides, each measured:
1. **The own orientation detector (#30) is wrong on 12 of these 77 pages; Tesseract on none.** Truth = upright as read from the contact sheets
   (no page is on its side or upside down); the detector turned f1 p5 and p22 by 270° (a picture spread and a JBIG2 page), f2 p10, p15, p16 by 180°,
   f7 p1, p4, p5, p6, p7, p8 by 180° and f7 p2 by 90°. Tesseract returned 0° on all 77. Those 12 pages come out upside down or sideways in the `text` row above (seen
   on the contact sheet of f7). It also changes the split decisions (f1 48 vs 50 pages, f7 17 vs 18), which is why the `text + Tess` rows are the ones to compare with `images`
   (50 and 18 pages). So section 3's "186/188" was measured on five Latin documents rotated by `rot90`; on photographed pages with dark surroundings and on
   picture/colour pages it does not hold. Not investigated why (unverified: dark surround, orange glow, large pictures counted as text lines).
   **Fixed in #45** (section 3, "Photographed pages"): dark edges and pictures were read as text lines; after the fix 0 of the 77 are turned.
2. **Text mode destroys pictures, as expected:** f1's paintings and photos become black/white blotches, and some full-bleed picture pages come out nearly blank (seen on the contact
   sheet of f1 output pages 9–16; unverified why: the flatten divides a uniform area by its own blur), coloured elements become grey on white. This is the case the mixed mode is for.
3. **f6 (dark table, photographed A3): both modes split wrongly** (fixed in #46, section 6). 3 of 4 spreads have no confident gutter; the median fallback cuts them, giving 8 pages, several of them
   almost empty halves; a spine band remains in text mode; the deskew estimate on some slots hits the ±10° limit (10.14°, 9.78°). `f7` and `f1` show dark spine/edge bands in text mode too.
   These are baseline flaws of the existing pipeline on this material, not of a mixed mode.
4. **`MODE=images` can be larger than the input** (f3: 2.12 MB vs 1.44 MB, ratio 1.47; f6 0.78, f2 0.56): the input was already a moderately compressed JPEG.
5. Peak memory of the spike is 0.9–2.1 GB (f32 buffers × 10 threads), bash images 0.14–1.8 GB.

### 4.4 Codec table (task 4)
Crops at 300 dpi cut from the rendered pages by the label boxes (8 crops, 2 per type, from 5 files; not committed): photo-a (f1 p20, 1289×880),
photo-b (f5 p5, 1190×1332), diagram-a (f5 p4 pie chart, 1339×1332), diagram-b (f7 p2 circular word diagram, photographed, 1160×1122),
coloured-text-a (f7 p3 pale-green box, 1031×910), coloured-text-b (f5 p2 coloured script heading, 1711×701), cover-a (f5 p1, 2480×3331), cover-b (f2 p1, 2281×2666).
**Reference = the decoded crop, which is itself a scanner JPEG**, so all numbers are a second generation of loss. Cell = size / PSNR dB / SSIM.
Encoders: ImageMagick 7.1.2 (JPEG 4:2:0 explicit, PNG level 9, PNG-8 = `-colors 256` with `/Indexed`), OpenJPEG 2.x `opj_compress` (`-r` = ratio to raw, so its size is
about raw/ratio regardless of content), libwebp 1.6 `cwebp`, `avifenc -q 60 -s 6`. SSIM = 1 − 2 × the parenthesised value of `magick compare -metric SSIM`
(checked against an own Gaussian-window SSIM: 0.9645 vs 0.9645; the first number ImageMagick prints is not SSIM). ∞ = identical.

| setting | photo-a | photo-b | diagram-a | diagram-b | coloured-text-a | coloured-text-b | cover-a | cover-b |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| PNG lossless (Flate) | 1132 kB | 1159 kB | 936 kB | 925 kB | 879 kB | 537 kB | 6092 kB | 5981 kB |
| JPEG q90 4:2:0 | 207 / 43.0 / 0.982 | 229 / 46.6 / 0.988 | 204 / 47.3 / 0.989 | 225 / 41.5 / 0.985 | 247 / 41.5 / 0.987 | 121 / 46.0 / 0.993 | 1244 / 45.8 / 0.988 | 1190 / 42.2 / 0.984 |
| JPEG q75 4:2:0 | 119 / 39.4 / 0.965 | 130 / 43.3 / 0.977 | 113 / 43.8 / 0.978 | 134 / 38.0 / 0.972 | 159 / 36.7 / 0.972 | 77 / 42.5 / 0.988 | 720 / 42.2 / 0.977 | 724 / 38.9 / 0.969 |
| JPEG q50 4:2:0 | 73 / 36.7 / 0.942 | 79 / 40.3 / 0.958 | 64 / 40.8 / 0.960 | 88 / 35.5 / 0.957 | 112 / 33.4 / 0.954 | 55 / 40.0 / 0.981 | 452 / 39.4 / 0.959 | 480 / 36.2 / 0.948 |
| JPEG q75 4:4:4 | 139 / 40.2 / 0.967 | 147 / 44.0 / 0.978 | 129 / 44.5 / 0.979 | 167 / 40.2 / 0.979 | 166 / 36.7 / 0.972 | 94 / 43.5 / 0.989 | 810 / 43.1 / 0.979 | 875 / 40.6 / 0.975 |
| PNG-8, 256 colours (Flate `/Indexed`) | 426 / 35.9 / 0.940 | 487 / 39.2 / 0.947 | 360 / 43.1 / 0.975 | 391 / 37.7 / 0.978 | 313 / 43.8 / 0.993 | 173 / 44.0 / 0.989 | 2675 / 37.8 / 0.930 | 2266 / 35.2 / 0.928 |
| JPEG 2000 lossless | 749 / ∞ | 829 / ∞ | 655 / ∞ | 704 / ∞ | 580 / ∞ | 405 / ∞ | 4239 / ∞ | 3981 / ∞ |
| JPEG 2000 ratio 10 | 340 / 48.4 / 0.992 | 475 / 52.6 / 0.997 | 535 / 60.0 / 0.999 | 390 / 50.2 / 0.996 | 281 / 47.1 / 0.994 | 360 / 62.7 / 1.000 | 2478 / 52.5 / 0.997 | 1824 / 48.1 / 0.995 |
| JPEG 2000 ratio 30 | 113 / 41.1 / 0.966 | 158 / 45.0 / 0.981 | 178 / 47.1 / 0.987 | 130 / 41.4 / 0.978 | 94 / 34.9 / 0.961 | 120 / 48.6 / 0.995 | 826 / 44.6 / 0.982 | 608 / 39.9 / 0.970 |
| WebP q90 | 143 / 42.3 / 0.976 | 133 / 44.1 / 0.979 | 117 / 44.7 / 0.980 | 152 / 41.8 / 0.985 | 183 / 42.4 / 0.989 | 65 / 45.3 / 0.992 | 715 / 43.8 / 0.979 | 847 / 42.1 / 0.983 |
| WebP q75 | 59 / 37.8 / 0.946 | 53 / 40.1 / 0.951 | 40 / 40.8 / 0.956 | 72 / 37.5 / 0.968 | 102 / 37.1 / 0.975 | 33 / 40.8 / 0.984 | 298 / 39.9 / 0.956 | 426 / 37.2 / 0.955 |
| WebP q50 | 40 / 36.1 / 0.930 | 35 / 38.6 / 0.937 | 24 / 39.5 / 0.945 | 53 / 35.7 / 0.960 | 77 / 35.0 / 0.968 | 25 / 39.1 / 0.981 | 202 / 38.4 / 0.945 | 308 / 35.2 / 0.932 |
| WebP lossless | 791 / ∞ | 789 / ∞ | 635 / ∞ | 676 / ∞ | 539 / ∞ | 377 / ∞ | 4131 / ∞ | 4022 / ∞ |
| AVIF q60 | 77 / 40.2 / 0.966 | 73 / 43.2 / 0.974 | 62 / 43.9 / 0.977 | 74 / 39.2 / 0.976 | 88 / 36.7 / 0.974 | 40 / 43.4 / 0.989 | 379 / 42.2 / 0.975 | 445 / 39.5 / 0.972 |

(sizes in kB in the cells after the first row; encode time: JPEG 0.03–0.15 s, JPEG 2000 0.15–0.99 s, WebP 0.06–0.5 s (lossless 3 s on a cover), AVIF 0.1–0.6 s, PNG 1–12 s.)

**Which viewers render which filter** (`tests/codec-table.py --viewers`: the crop embedded *without re-encoding* by `img2pdf --imgsize 300dpi`, rendered at 1:1, each render compared with
the same viewer's render of the lossless Flate variant; run on photo-a and diagram-a, both gave the same picture):

| PDF filter | poppler `pdftoppm` | Ghostscript | macOS PDFKit (`qlmanage`, the Preview/Quick Look renderer) | hayro (this spike, via `--detect-eval`) | Chrome / Firefox pdf.js |
|---|---|---|---|---|---|
| `DCTDecode` (JPEG) | ok | ok | ok | ok | not tested |
| `FlateDecode` + PNG predictor (lossless) | ok, ∞ dB reference | ok | ok | ok | not tested |
| `FlateDecode` `/Indexed` (PNG-8) | ok, 37.5 / 44.0 dB vs lossless (photo/diagram) | 35.9 / 43.1 | 38.3 / 45.1 | ok (same image size and statistics) | not tested |
| `JPXDecode` (JPEG 2000, ratio 10) | ok, 49.6 / 60.2 dB | ok, 48.4 / 60.0 | ok, 49.9 / 60.9 | ok | not tested |
| WebP, AVIF | **no PDF filter exists** (the spec has none), so there is nothing to embed or to test; only usable as a decoded-and-recompressed intermediate | | | | |

The dB values equal the codec-only PSNR of the table within 1–2 dB, so all four renderers decode all four filters correctly. Chrome and Firefox could **not** be tried: headless Chrome
(`--screenshot` on a PDF) produced no file here, the desktop app's browser pane shows local files as static snapshots without page tools, and Firefox is not installed. Preview itself was not run (qlmanage uses the same PDFKit renderer, unverified for the interactive app). So "≥ 3 viewers" = poppler, Ghostscript, PDFKit (+ hayro).

### 4.5 Signal statistics (task 5)
`--detect-eval a.pdf [b.pdf …]` renders every page at 300 dpi (colour needed, so the extract path is not used), computes the flatten + hysteresis ink map of the output path, and prints one TSV line per page and
per 320 × 320 px tile (page 4299×3035 → 14×10 tiles; tiles less than half size at the border dropped): 77 page lines and 8 259 tile lines for the seven files, **6–7 s wall on 10 threads, byte-identical
on a second run** (`cmp`). Eleven signals; the constants (chroma 20/60 of 255, mid-tones 0.2–0.8, 64 grey bins, edge |∇luma| > 0.2, "big" component ≥ 2000 px) are measurement settings, not rules:

| signal | definition |
|---|---|
| `hasler` | Hasler–Süsstrunk colourfulness on 8-bit RGB: σ(rg, yb) + 0.3 μ(rg, yb) |
| `nongrey20`, `nongrey60` | share of pixels with max(R,G,B) − min(R,G,B) ≥ 20 / ≥ 60 |
| `lum_std` | standard deviation of Rec.601 luma (0–1) |
| `midtone` | share of pixels with luma in 0.2–0.8 (grey-level histogram spread) |
| `entropy` | Shannon entropy of the 64-bin luma histogram, bits |
| `ink` | ink share after flatten + hysteresis (#20) |
| `cc_per_mpx` | 8-connected ink components per megapixel (component counted in the tile holding its centroid) |
| `cc_med_area` | median component area, px |
| `cc_big_frac` | share of ink pixels lying in components ≥ 2000 px |
| `edge` | share of pixels with central-difference gradient magnitude > 0.1 (luma 0–1) |

`tests/detect-join.py` joins the TSV with the labels. Tile label = the region class covering ≥ 75 % of the tile; `rest` = no region touches it (text, margins, blank paper and the **scanner
surround / dark table**); tiles partly inside a region are dropped (550 tiles). Counts: 424 picture, 100 diagram, 166 coloured-text, 289 cover, 6 730 rest tiles. Page label: the class is on the page, compared with the **41 pages
that are body text only** (no picture/diagram/coloured region; from 5 files). Score per signal and class pair: **AUC** (direction-free, max(AUC, 1−AUC)) and **best-cut balanced accuracy** (the best single threshold, direction free; the cut itself is
not reported: it would be a rule), then `[min–max per-file AUC, number of files with both classes]`. Cell = AUC / balanced accuracy.

**Tiles, class vs rest**, all pages:

| signal | picture vs rest | diagram vs rest | coloured-text vs rest | cover vs rest |
|---|---:|---:|---:|---:|
| n | 424 / 6730 | 100 / 6730 | 166 / 6730 | 289 / 6730 |
| hasler | 0.97 / 0.91 [0.97–1.00, 3f] | 0.85 / 0.79 [0.77–0.95, 3f] | 0.91 / 0.86 [0.72–0.99, 4f] | 0.95 / 0.89 [0.81–1.00, 4f] |
| nongrey20 | 0.94 / 0.91 [0.93–1.00, 3f] | 0.79 / 0.79 [0.66–0.91, 3f] | 0.80 / 0.79 [0.67–1.00, 4f] | 0.88 / 0.85 [0.68–1.00, 4f] |
| nongrey60 | 0.90 / 0.89 [0.90–1.00, 3f] | 0.78 / 0.78 [0.63–0.95, 3f] | 0.77 / 0.76 [0.67–1.00, 4f] | 0.77 / 0.76 [0.67–1.00, 4f] |
| lum_std | 0.55 / 0.57 [0.56–0.66, 3f] | 0.51 / 0.59 [0.52–0.67, 3f] | 0.55 / 0.64 [0.50–0.67, 4f] | 0.58 / 0.61 [0.58–0.75, 4f] |
| midtone | 0.87 / 0.85 [0.55–0.97, 3f] | 0.76 / 0.70 [0.74–0.79, 3f] | 0.75 / 0.73 [0.75–0.93, 4f] | 0.85 / 0.81 [0.79–1.00, 4f] |
| entropy | 0.96 / 0.88 [0.85–1.00, 3f] | 0.79 / 0.75 [0.71–0.88, 3f] | 0.83 / 0.80 [0.69–0.79, 4f] | 0.92 / 0.86 [0.79–0.99, 4f] |
| ink | 0.73 / 0.71 [0.67–0.78, 3f] | 0.58 / 0.57 [0.60–0.89, 3f] | 0.52 / 0.59 [0.53–0.59, 4f] | 0.51 / 0.53 [0.56–0.65, 4f] |
| cc_per_mpx | 0.82 / 0.81 [0.64–0.90, 3f] | 0.59 / 0.60 [0.67–0.91, 3f] | 0.67 / 0.66 [0.68–0.85, 4f] | 0.52 / 0.56 [0.58–0.68, 4f] |
| cc_med_area | 0.67 / 0.79 [0.60–0.76, 3f] | 0.53 / 0.60 [0.57–0.70, 3f] | 0.55 / 0.66 [0.52–0.73, 4f] | 0.57 / 0.69 [0.52–0.77, 4f] |
| cc_big_frac | 0.72 / 0.78 [0.66–0.84, 3f] | 0.67 / 0.74 [0.66–0.69, 3f] | 0.58 / 0.57 [0.52–0.69, 4f] | 0.61 / 0.63 [0.50–0.82, 4f] |
| edge | 0.51 / 0.57 [0.52–0.64, 3f] | 0.63 / 0.62 [0.56–0.89, 3f] | 0.58 / 0.60 [0.58–0.70, 4f] | 0.66 / 0.68 [0.63–0.70, 4f] |

(A pooled AUC can lie outside the per-file range because the direction is chosen per file: e.g. `entropy` for coloured text.)

**Tiles, class pairs** (picture vs diagram n = 424/100, picture vs coloured-text 424/166, diagram vs coloured-text 100/166), all pages:

| signal | picture vs diagram | picture vs coloured-text | diagram vs coloured-text |
|---|---:|---:|---:|
| hasler | 0.68 / 0.65 | 0.74 / 0.71 | 0.52 / 0.65 |
| nongrey20 | 0.63 / 0.63 | 0.63 / 0.69 | 0.51 / 0.59 |
| nongrey60 | 0.65 / 0.63 | 0.59 / 0.64 | 0.53 / 0.60 |
| lum_std | 0.55 / 0.61 | 0.62 / 0.67 | 0.59 / 0.61 |
| midtone | 0.65 / 0.71 | 0.66 / 0.75 | 0.50 / 0.58 |
| entropy | 0.77 / 0.77 | 0.89 / 0.90 | 0.51 / 0.64 |
| ink | 0.66 / 0.65 | 0.76 / 0.78 | 0.60 / 0.64 |
| cc_per_mpx | 0.73 / 0.72 | 0.77 / 0.78 | 0.55 / 0.65 |
| cc_med_area | 0.84 / 0.85 | 0.78 / 0.79 | 0.53 / 0.61 |
| cc_big_frac | 0.71 / 0.75 | 0.82 / 0.85 | 0.78 / 0.81 |
| edge | 0.64 / 0.63 | 0.60 / 0.63 | 0.54 / 0.61 |

**Pages**, page has the class vs the 41 text-only pages (n = 15 picture, 10 diagram, 13 coloured-text, 4 cover):

| signal | picture | diagram | coloured-text | cover |
|---|---:|---:|---:|---:|
| hasler | 0.98 / 0.97 | 0.95 / 0.90 | 0.97 / 0.94 | 1.00 / 1.00 |
| nongrey20 | 0.98 / 0.95 | 0.96 / 0.93 | 0.97 / 0.94 | 1.00 / 1.00 |
| nongrey60 | 0.99 / 0.97 | 0.88 / 0.90 | 0.95 / 0.96 | 1.00 / 1.00 |
| lum_std | 0.50 / 0.66 | 0.54 / 0.66 | 0.54 / 0.67 | 0.66 / 0.74 |
| midtone | 0.91 / 0.90 | 0.74 / 0.72 | 0.65 / 0.72 | 0.98 / 0.96 |
| entropy | 0.95 / 0.89 | 0.84 / 0.80 | 0.86 / 0.81 | 1.00 / 1.00 |
| ink | 0.53 / 0.64 | 0.78 / 0.75 | 0.79 / 0.75 | 0.55 / 0.68 |
| cc_per_mpx | 0.53 / 0.71 | 0.74 / 0.75 | 0.67 / 0.72 | 0.70 / 0.74 |
| cc_med_area | 0.57 / 0.76 | 0.50 / 0.71 | 0.57 / 0.72 | 0.59 / 0.79 |
| cc_big_frac | 0.73 / 0.74 | 0.61 / 0.75 | 0.56 / 0.68 | 0.87 / 0.85 |
| edge | 0.69 / 0.71 | 0.83 / 0.84 | 0.84 / 0.82 | 0.98 / 0.99 |

**The same restricted to pages whose source has colour** (RGB JPEG pages only, 16 text-only pages instead of 41; tiles: rest n = 4 414). Nothing on a 1-bit or grey page can carry chroma, so the all-pages
numbers above are partly a measurement of the source encoding (25 of 77 pages):

| signal | tiles: picture vs rest | tiles: coloured-text vs rest | pages: picture vs text-only | pages: diagram vs text-only | pages: coloured-text vs text-only |
|---|---:|---:|---:|---:|---:|
| hasler | 0.95 / 0.90 | 0.86 / 0.79 | 0.96 / 0.97 | 0.86 / 0.90 | 0.92 / 0.92 |
| nongrey20 | 0.93 / 0.90 | 0.78 / 0.77 | 0.95 / 0.94 | 0.89 / 0.90 | 0.92 / 0.92 |
| nongrey60 | 0.90 / 0.88 | 0.77 / 0.75 | 0.98 / 0.97 | 0.86 / 0.90 | 0.94 / 0.96 |
| midtone | 0.82 / 0.81 | 0.68 / 0.67 | 0.80 / 0.84 | 0.55 / 0.65 | 0.73 / 0.81 |
| entropy | 0.93 / 0.86 | 0.75 / 0.75 | 0.86 / 0.81 | 0.59 / 0.71 | 0.64 / 0.71 |
| ink | 0.77 / 0.74 | 0.51 / 0.57 | 0.88 / 0.87 | 0.64 / 0.70 | 0.66 / 0.69 |
| cc_per_mpx | 0.88 / 0.84 | 0.73 / 0.70 | 0.93 / 0.93 | 0.53 / 0.64 | 0.66 / 0.73 |
| cc_med_area | 0.70 / 0.79 | 0.56 / 0.66 | 0.97 / 0.97 | 0.85 / 0.85 | 0.96 / 0.92 |
| cc_big_frac | 0.70 / 0.77 | 0.58 / 0.58 | 0.95 / 0.90 | 0.90 / 0.92 | 0.82 / 0.80 |
| edge | 0.55 / 0.59 | 0.60 / 0.63 | 0.52 / 0.61 | 0.79 / 0.81 | 0.76 / 0.72 |

Page level, picture vs diagram (13/6 pages) and picture vs coloured-text (13/7 pages), only-that-class pages: best AUC `cc_per_mpx` 0.86 / 0.89, `entropy` 0.87 / 0.84,
`cc_med_area` 0.86 / 0.88, `midtone` 0.82 / 0.98, `cc_big_frac` 0.77 / 0.95 (full table: `python3 tests/detect-join.py`).

Observations (numbers, not rules; small n):
- **Picture vs text/rest is separated by colour signals** (`hasler` 0.97 tiles, 0.98 pages; 0.95 / 0.96 on RGB pages only) and by `entropy`; luma spread and edge density barely separate it on tiles (0.51–0.55).
  A photographed *black-and-white* page would not be caught by colour; the grey engraving (f1 p17) is in the picture class and the only grey picture.
- **Diagram and coloured text are close to each other** (AUC 0.50–0.55 on all signals except `lum_std` and `ink` 0.59–0.60 and `cc_big_frac` 0.78). Diagram vs text-only pages is best by colour (0.95) and `edge` (0.83).
- **Picture vs diagram** is best separated by median component size (0.84 tiles, 0.86 pages) and `entropy`, not by colour (0.63–0.68) — which is a statement about how I drew the border (4.2), with only 100 diagram tiles from 4 files.
- **The signals lean on the source.** All-pages tile AUC for coloured text vs rest drops 0.91 → 0.86 for `hasler` when 1-bit/grey pages are removed, `cc_med_area` for page-level picture vs text-only rises 0.57 → 0.97 (a
  colour-scan artefact: JPEG noise on RGB text pages makes many tiny components, 1-bit pages do not), i.e. `cc_*` values depend on the encoding of the page, not only on the content.
- **Scanner surround counts as `rest`.** f1, f4, f6, f7 are photographed on dark tables; f7 has an orange lamp glow on every page. Those tiles have colour or extreme luma and are labelled `rest`. That makes the
  colour numbers *pessimistic* for picture vs rest on those files; a signal computed inside the crop box would look better. Not measured here (would need the crop from pass A in this eval).
- **Per-file spread is large for the weak signals** (e.g. `entropy` coloured-text vs rest 0.69–0.79 per file, 0.83 pooled; diagram vs coloured-text `lum_std` 0.55–0.99): with 3–4 files per class pair, that is a property of the files.

### 4.6 What did not work, what is missing
Did not work / cost time:
- `magick montage -label` produced nothing readable (no default font found here); sheets in tile order instead, a grid drawn with an explicit font path (`/System/Library/Fonts/Helvetica.ttc`) for the box reading.
- `stat` on the symlinks I used for short names gave 63 B input sizes in the first baseline run (fixed from the real files; the bash "not smaller" test itself follows the link).
- `magick compare -metric SSIM` on ImageMagick 7.1.2 prints a number that is *not* SSIM (598 … 2300 for JPEG q90 … q50); the parenthesised value is (1−SSIM)/2 (4.4).
- First viewer test compared each render with the source crop: PSNR 17–30 dB even for a lossless PNG, because page geometry/resampling dominates; the same viewer's lossless render as reference gave codec-only numbers.
- `img2pdf` without `--imgsize 300dpi` sets page sizes from image metadata that differ per format (508×347 vs 4029×2750 px at 300 dpi), which made the first render comparison meaningless.
- Headless Chrome screenshots of a PDF produced no file; the app's browser pane cannot be driven on local PDF files (4.4).
- The per-signal thresholds at the best cut are optimistic by construction (chosen on the data they are scored on, no hold-out; leave-one-file-out was not done, the per-file AUC ranges are the only check).

Still missing (for #38 / later):
- **A picture-heavy public-domain source.** All seven files are the owner's; a book with figures under a clear licence is still needed for anything that is committed (tests, fixtures).
- **Vector/rendered pages with figures.** All 77 pages are scans; the second source kind of section 3's warning (rendered text) is not covered by this label set. Text-only vector pages exist locally
  (`ren-side`, `flerspaltet`) but carry no picture/diagram labels.
- Labels are by one person (me), boxes ±0.03 of a page, 19 pages uncertain; no second labeller.
- Signals were computed on the whole page image including the surround and on the 300-dpi render; per-region signals inside the crop box, a grey-scale-only source, and colour signals at lower resolution were not measured.
- `blank` has zero examples; no blank pages exist in this material.
- Error cost (picture treated as text vs the reverse) is not measured here; the baseline gives the two extremes (text mode: pictures destroyed, ~0.11 of input; images mode: ~0.39 of input, 1.47 on f3).

### 4.7 Observations for #38 (not proposals)
- Output/input is 0.085–0.38 for text and 0.28–1.47 for images; the images output is 3.3–4.7× the text output on every file (f1 3.3×, f2 3.8×, f3 3.9×, f4 3.5×, f5 3.8×, f6 3.7×, f7 4.7×).
- Chroma signals are cheap and separate colour pictures from body text well on colour scans; they say nothing on 1-bit/grey pages (25 of 77 here), where the tool has no colour to keep anyway.
- The hard pairs are diagram vs coloured text and picture vs diagram; the labels for those are the least certain.
- Detection would also have to survive the pipeline's own weak spots seen in 4.3: wrong orientation (12 pages; fixed in #45), wrong splits on the dark-table photographs (f6; fixed in #46, section 6), and the flatten blanking full-bleed pictures.

## 5. B4 decision: mixed output (#38)

Decisions on #38's questions, from the section 4 data plus four bounded measurements made here (5.0). Tags: **[measured]**,
**[estimate]** (derived from measured parts, or a judgement), **[unknown]**; **[labels?]** = the number moves with the 19 uncertain
pages or the ±0.03 boxes of 4.2; **[scans]** = all 77 pages are scans of one owner's books (no rendered page with figures, no hold-out
source, one labeller). Nothing in the pipeline changed; `rust/src/main.rs` is untouched.

### 5.0 New measurements
Reproduce (outputs stay in the ignored folder; `D="tests/real/new pdfs dont upload"`, `W="$D/work"`, `B=rust/target/release/bitonalpdf`):
```
for s in A:geom B:slots; do for i in 1 2 3 4 5 6 7; do         # pass-A box/gutter and slots per page, prefixed with fN
  BITONAL_TIMING=1 $B --crop --split auto $W/f$i.pdf /tmp/x.pdf 2>&1 >/dev/null | grep "^${s%:*} page" | sed "s/^${s%:*} page/f$i/"
done > $W/m3/${s#*:}.txt; done
# $W/m3/bitonalpdf-fix40.sh = bitonalpdf.sh + #40's one-line fix - the "not smaller" test; MODE=images, all four flags -> $W/m3/base40/fN.images.pdf
python3 tests/mixed-eval.py $W/det.tsv "$D" $W/m3/geom.txt $W/m3/slots.txt $W/base $W/m3/base40 --sizes   # ~70 s, same output twice
python3 tests/codec-table.py $W/crops/*.png        # now with the rows of 5.2/5.3
python3 tests/codec-table.py --mrc-viewers         # synthetic PDF, no scan content
```
- **Pass-A geometry without `--rotate`** (all 77 pages are upright, 4.3 item 1), so boxes and labels share one frame; its split plan gives the
  same output page counts as the `text + Tess` and `images` baselines (50/18/17/10/7/8/18).
- **Images baseline without #40** (`$W/m3/base40`: scratch copy of `bitonalpdf.sh` with #40's verified one-line fix and without the "not
  smaller" test): 21.25 MB instead of 22.27 MB (4.3 correction). All sizes below are **image-stream bytes** (`pdfimages -list`, 0.1 kB):
  text 5.93 MB (the files are 6.15 MB), images 20.55 MB; per output page 46.3 kB vs 160.5 kB.
- `tests/mixed-eval.py`: the four non-text classes pooled into one class `keep` (5.1); cuts chosen **leave-one-file-out** (cut from six files,
  applied to the seventh); flagged 320-px tiles grouped 8-connected into bounding boxes + ½ tile; per labelled region the share covered by
  the boxes; sizes per output page from the two baselines, and region sizes as JPEG q65 4:2:0 at 150 dpi (MODE=images' setting) of every box.
- `tests/codec-table.py`: rows at MODE=images' setting, the real text-mode 1-bit output, one-colour mask, MRC-lite; `--mrc-viewers` renders
  the layer constructs a region mode would need.

### 5.1 Detection
**Decision:** one class, `keep` (= picture ∪ diagram ∪ coloured-text ∪ cover) vs text, decided **per output page**; no automatic region
detection. Automatic detection is only a *suggestion* (5.2), never the default action.

- **Class borders the signals cannot carry** [measured, labels?]: diagram vs coloured text (tile AUC 0.50–0.55 on every signal except
  `cc_big_frac` 0.78, 100/166 tiles from 3–4 files, 4.5); picture vs diagram (best 0.84, `cc_med_area`, with the least certain labels of 4.2);
  grey picture vs text (one example, f1 p17) [unknown]; inline coloured sentences vs a coloured box (a labelling convention on f7, not a
  signal). So nothing downstream may depend on *which* non-text class a region is: one class, one treatment, one codec (5.5).
- **Page level, page `hasler`, cut = lowest keep page of the other six files:** 34 of 36 keep pages caught (misses f1 p1 and p2, both uncertain
  labels), **10 of 41 text-only pages flagged, all 10 on RGB pages = 10 of 16 RGB text-only pages** [measured, scans]. In-sample: 36/36 and
  15/16. The page AUC of 0.98 in 4.5 is mostly the source encoding: all 25 1-bit/grey pages are text-only here.
- **Tile level, `hasler`, tiles inside the pass-A crop box, leave-one-file-out** [measured, labels?, scans]:

| R (tile recall target) | keep tiles caught | FP rate (rest tiles) | regions < 90 % covered (of 46) | < 50 % covered | text-only pages with an FP box (of 41) |
|---:|---:|---:|---:|---:|---:|
| 0.5 | 461/903 | 0.006 | 22 | 15 | 0 |
| 0.7 | 620/903 | 0.028 | 17 | 9 | 8 |
| 0.8 | 715/903 | 0.099 | 13 | 8 | 14 |
| 0.9 | 808/903 | 0.179 | 11 | 3 | 16 |
| 0.99 | 895/903 | 0.480 | 5 | **0** | **16** |

  There is no usable operating point: losing no region costs an FP box on every RGB text page (all 16), and a low FP rate loses a third of
  the regions. Why, per file (tile `hasler` inside the crop box): the lowest 5 % of keep tiles are 1–4 on f1/f3/f5/f7 (white paper inside a
  diagram or cover box, the grey engraving), while the top 5 % of rest tiles reach 5–30 (f7 99th percentile 78; unverified why: inline orange
  sentences and the lamp glow on f7, paper tint under photo light on f1/f6). Without the crop-box restriction the FP rate at R 0.99 is 0.51 instead of 0.48, so the
  dark surround is not the main source. Adding `entropy` (for grey pictures) flags 23 text pages instead of 16 and saves 2 regions at R 0.95.
- **The pass-A crop box cuts what a colour mode must keep** [measured, labels?]: 8 of 46 labelled regions lose 17–45 % of their area to it
  (covers worst: f7 p1 45 %, f2 p1 32 %, f3 p1 27 %; f5 diagrams 28–31 %). Any mode that keeps pictures inherits this (#46 fixed only the splits; the crop box is unchanged).
- Level: page (output slot) because the region-level gain is small even with perfect regions (5.4) and the region detector cannot reach it.

### 5.2 Error cost, safe default, override
| error | size effect | damage | tag |
|---|---|---|---|
| text page kept in colour (FP, page level) | +114 kB per output page (160.5 vs 46.3 kB, ×3.5); per file 3.0–4.1× | none: text stays readable at 150 dpi JPEG (MODE=images' look) | measured |
| text area kept as a region (FP, region level) | 11.9 MB of FP boxes at the safe point, 5.5 MB at R 0.8 (vs 5.93 MB for the whole text file) | none | measured, labels? |
| colour page output as text (FN) | −114 kB per output page | photos/covers as the real text-mode 1-bit output: SSIM 0.27–0.50, PSNR 3–7 dB (vs MODE=images' JPEG q65 150 dpi: SSIM 0.89–0.94); diagrams 0.79–0.85 ; full-bleed pictures come out nearly blank (4.3 item 2) | measured |
| region missed (FN, region level) | − its JPEG bytes | at R 0.8: 8 of 46 regions < 50 % covered (f1 p1 diagram, p2 coloured text, p17 picture, f5 p3 diagrams, f7 p3/p6/p8), i.e. binarised | measured, labels? |

The damage of a miss is irreversible in the output (the figure is gone; only the input keeps it), the cost of a false alarm is bytes. So:
- **Default: unchanged.** `MODE=text` stays text; nothing is coloured unless asked. [estimate: a judgement, from the table]
  **Superseded by section 9 (#74):** with information over size, the default is every RGB-source page as a JPEG page, printed.
- **Override / the mechanism:** `--colour-pages LIST` keeps the listed pages as MODE=images pages inside a text-mode PDF. LIST names input pages,
  with an optional half (`12` = both halves, `12a`/`12b` = left/right, as bash names its slots): colouring whole input pages instead of the
  labelled halves costs 12.72 vs 10.87 MB (2.14× vs 1.83× text; f7 2.47 vs 1.87 MB) [measured, labels?].
- **`--colour-pages auto`**: the recall-first page detector (tiles in the crop box, R 0.99 cut from this suite) chooses the list and **prints
  it** (like the gutter warning), so the user can correct it. On this material it colours 91 of 128 output pages (42 needed) [measured, scans].

### 5.3 Coloured text
**Decision:** a page with coloured text is a colour page (JPEG, page level) when the user lists it; no special coloured-text treatment now.
Mask + colour is the attractive option on paper but needs a signal that does not exist (it must tell a coloured-text page from a picture or
diagram page, the pair that fails in 5.1). [measured sizes, estimate legibility]

Measured on the two coloured-text crops (`tests/codec-table.py`, bytes / PSNR / SSIM against the crop) [measured, scans; legibility = estimate]:

| crop | JPEG q65 150 dpi (MODE=images) | 1-bit text mode (G4) | mask + 1 colour | MRC-lite (mask + colour over 75-dpi JPEG q50) |
|---|---:|---:|---:|---:|
| coloured-text-a (pale-green box) | 54.6 kB / 23.7 / 0.875 | 14.7 kB / 17.1 / 0.888 | 14.7 kB / 19.3 / 0.918 | 29.1 kB / 19.4 / 0.766 |
| coloured-text-b (coloured heading) | 26.7 kB / 34.2 / 0.964 | 3.2 kB / 16.5 / 0.856 | 3.2 kB / 16.8 / 0.868 | 11.9 kB / 26.5 / 0.904 |

Mask + one colour is 4–8× smaller than the JPEG and sharp (300-dpi G4), but drops the box fill (low PSNR on a); MRC-lite without
inpainting is worse than either on the box. Size alone favours mask + colour; the missing piece is the signal (5.9 item 2), and a wrong
call turns a picture into a one-colour silhouette (photo crops: SSIM 0.55–0.59 in that row).

Open point with spec: 5.9 item 2.

### 5.4 Structure
**Decision:** whole output page, one image per page, as today: either the G4 page (text) or the JPEG page (colour). No region layers, no MRC,
no JPX/JBIG2.
- **Region layers with perfect regions are only 5 % smaller than whole pages with a perfect page list:** text + region JPEGs of the labelled
  boxes 10.29 MB (1.73× text) vs 10.87 MB (1.83×) [measured parts, estimate sum: G4 under the boxes is not removed, overlapping boxes count
  twice; labels?]. With detected regions it is worse than MODE=images: 20.87 MB at the safe point (3.52× vs 3.46×), 14.12 MB at R 0.8 with 8
  regions lost [measured, scans].
- **Viewers:** DCT and CCITT pages render in poppler, Ghostscript, PDFKit and hayro (4.4; the text-mode output is CCITT); a page-level mixed
  PDF uses only those two filters, one image per page, on one page size (the text canvas; the JPEG page is 150 dpi on the same size in pt).
  - **The layer constructs a region mode would need also render** [measured, synthetic]: `tests/codec-table.py --mrc-viewers` (a JPEG region with a 1-bit `/ImageMask` on top, and a low-res JPEG with a full-res stencil `/Mask`) gave the expected colours at all 21 check points in poppler, Ghostscript and PDFKit. So the rejection in 5.4 is about size, not viewers.
- **pdf-writer without a new crate** writes the page (`image_xobject` + `Filter::DctDecode`, the same page/content code as the G4 page, and
  `image_mask(true)` if a region mode ever comes). It does **not** encode JPEG: the dependency tree has only decoders (`zune-jpeg`,
  `hayro-jpeg2000`) and a Flate encoder (`flate2`/`miniz_oxide`, via `png`). One new crate is needed for the JPEG page (5.5).

### 5.5 Codec
**Decision:** JPEG (DCTDecode) 4:2:0, **q65 at 150 dpi** = MODE=images' setting, for every colour page, whatever its content. [measured, scans]
- One codec, because detection cannot tell the content types apart (5.1), and the best per-type choice would differ only for diagram vs
  coloured text, the pair it fails on.
- JPEG costs 0.10–0.13 of lossless PNG at 38–44 dB (q75, 300 dpi, 4.4). PNG-8/`/Indexed` is 2–3× larger for about the same PSNR on diagrams
  (diagram-a 360 vs 113 kB) and much worse on photos (35.9 dB); JPEG 2000 at ratio 30 is about JPEG q75's size with +1–4 dB on photos/diagrams
  but −1.8 dB on coloured-text-a, 5–10× the encode time, and there is no pure-Rust encoder (OpenJPEG is C; `hayro-jpeg2000` decodes);
  WebP/AVIF have no PDF filter. The reference of 4.4 is itself a scanner JPEG, so all these are second-generation numbers.
- At MODE=images' own setting: JPEG q65 at 150 dpi is 25–55 kB per crop (~200 kB on a whole cover) at SSIM 0.88–0.96; q75 at 150 dpi costs +20–25 % for +0.5 dB. That is the current MODE=images look, now as a number.
- **New crate:** `jpeg-encoder` 0.7.1 (crates.io 2026-07, 7.9 M downloads), licence **(MIT OR Apache-2.0) AND IJG**: permissive, compatible with
  the AGPL-3.0; the IJG part asks for the sentence "this software is based in part on the work of the Independent JPEG Group" in the
  documentation of a binary distribution. Pure Rust, no system library. [measured: crates.io; not built here]

### 5.6 Price
**Size** (image-stream bytes, 7 files, 128 output pages; per file in `tests/mixed-eval.py` output) [measured, labels? for the label rows]:

| variant | size | × text | colour output pages |
|---|---:|---:|---:|
| MODE=text (G4, `text + Tess` baseline) | 5.93 MB | 1.00 | 0 |
| mixed, page list = labels, per half | **10.87 MB** | **1.83** | 42 |
| mixed, page list = labels, whole input pages | 12.72 MB | 2.14 | 62 |
| mixed, `auto` (safe, R 0.99) | 15.94 MB | 2.69 | 91 |
| mixed, detector at R 0.8 (loses 8 regions) | 14.89 MB | 2.51 | 84 |
| region layers, labelled boxes (not chosen, 5.4) | 10.29 MB | 1.73 | – |
| MODE=images (bash, #40 fixed) | 20.55 MB | 3.46 | 128 |

Per file with the label list: f1 4.57 MB (text 2.58, images 7.76), f2 1.32 (1.08 / 4.08), f3 0.71 (0.52 / 2.04), f4 0.54 (0.45 / 1.56),
f5 1.26 = images (every page has colour), f6 0.60 (0.37 / 1.38), f7 1.87 (0.60 / 2.47). **So mixed with a page list saves about half of
MODE=images (−47 %) and costs 1.8× text**, and all of that comes from pages the user wants in colour anyway.

**Time** [estimate, with a measured upper bound]: text mode (Rust) is 12.0 s wall / 80 s CPU for the seven files (own orientation) or
23.4 s / 156 s (Tesseract); MODE=images (bash, #40 fixed) 183 s wall / 611 s CPU. Mixed = the text path plus, per colour page, keeping the
RGB of the page (Probe converts to luma today) and one JPEG encode of a 150-dpi page (1–4 MP; not measured in Rust, ImageMagick needs
0.03–0.15 s for a 1–8 MP crop, 4.4). With a page list: + ≈ 0.1 s CPU per colour page, i.e. single-digit percent. With `auto`: plus the
detection, whose upper bound is `--detect-eval` itself (300-dpi render + 11 signals + components): 44 s CPU for 77 pages (0.57 s per input
page, measured in the audit run under load; 6–7 s wall idle), ≤ +28 % CPU on the Tesseract path; `hasler` alone is one pass over the RGB.

**Encode time, measured (#75):** `jpeg-encoder` 0.7.1, q65, 4:2:0, one A4 page at 150 dpi (1240×1754, synthetic text-like RGB, 344 kB): 14, 13, 13 ms
(3 runs, release, `cargo test --release -- --ignored --nocapture jpeg_encode_time`) [measured]. That is a tenth of the 0.03–0.15 s ImageMagick
needed per crop, so the encode is unlikely to be what the "≈ 0.1 s CPU per colour page" above is spent on (the render to RGB was not timed here). A real scan with more
texture may be slower; not measured on one.

### 5.7 Recommendation: go with reservations
**Go** for **page-level mixed output with a page list**: text mode, plus MODE=images pages where the user lists them (`--colour-pages`), and
`auto` as a printed suggestion. It halves MODE=images on this material (1.83× vs 3.46× text) with no new PDF construct and one new crate.
**Not worth it** now: automatic region detection, region layers, MRC, per-type codecs, coloured text as mask + colour. The numbers that decide
it: perfect regions beat perfect pages by only 5 % (5.4), and no tile cut is both safe and cheap (5.1).

Prerequisites before implementing:
1. ~~**Orientation** (#45)~~: done, 0 of 77 upright pages turned (section 3, "Photographed pages"). Photographed pages that really are
   turned are mostly left as they are by the own detector; use `BITONAL_OSD=tesseract` for those.
2. **Crop box vs pictures** and the dark-table splits (splits fixed in #46; the crop box is still open): the crop cuts 8 of 46 labelled regions by 17–45 %.
3. **#40** merged, so the bash reference (MODE=images) is right.
4. **A Rust JPEG page** (the images part of #9, or the first issue below) with the `jpeg-encoder` licence note.
5. **A licence-clean test source with figures and a rendered page** (#47): every number here is from seven scans and one labeller;
   `auto` must be re-measured there before anyone relies on it.

Implementation issues (to create when the prerequisites are met; not created here):
1. Rust: keep a page's RGB on demand (`page_gray`/`Probe`), JPEG page writer (`jpeg-encoder`, DCTDecode, 150 dpi on the text canvas),
   i.e. `MODE=images` in Rust. Tests: round trip through hayro, one page size across both kinds.
2. `--colour-pages LIST` with halves (`12`, `12a`, `12b`), mixed PDF, `real.sh`-style checks; target on f1–f7 with the label list:
   ≤ 1.9× text and page sizes equal.
3. `--colour-pages auto`: page detector of 5.1 (tiles in the crop box, R 0.99), printed list, a decision column in `--detect-eval`;
   re-measured on the #47 sources.
4. Conditional: coloured-text pages as G4 + one colour, only if 5.9 item 2 finds a signal.
5. Conditional: region layers, only if new material shows a region-vs-page gap well above 5 %.

What would change this: books where pictures are small parts of many text pages (region-vs-page gap ≫ 5 %); a detector with an FP rate
< 0.05 at no lost region on a held-out source; a user for whom sharp 300-dpi text on colour pages matters more than bytes.

### 5.8 What did not work
- **Recall-first tile cuts** (R 0.95/0.99) as first planned: FP rate 0.23–0.48, because label boxes contain white paper; tile recall was the
  wrong target, region coverage the right one.
- **Page-relative `hasler`** (tile minus the page's median tile) to cancel paper tint and lamp glow: no gain (on covers and full-bleed
  pages the median *is* the picture; the f7 glow is local).
- **Restricting tiles to the crop box** lowered the FP rate only from 0.51 to 0.48 at R 0.99.
- **`hasler` OR `entropy`** for grey pictures: 23 instead of 16 text pages with FP boxes, with one grey picture in the set.
- The first size tables used the section 4.3 images baseline, which carries #40 on f1 and f7 (+8 %/+16 %); rerun with the fix.
- Joining `--detect-eval` output with the file list failed on "å": macOS returns NFD names, the TSV had NFC; normalise before joining.
- `--detect-eval` took 9.4 s wall in the audit rerun (other jobs running) instead of 6–7 s; output byte-identical.
- The codec-table run takes ~9 min (large cover crops); the second run was not finished, so only the deterministic pipeline backs its reproducibility.

### 5.9 Open points (each with a spec)
1. **Page-level mixed in bash** (#38 allowed bash if cheap): `finish_slot` would choose text/JPEG per slot from the list, and the final
   `magick "$TMP"/p*.[tj]* -density "$DPI" "$OUT"` already takes both kinds, but it sets **one** density, so a 150-dpi JPEG page would get
   twice the page size. Spec: write the JPEG slots at the text canvas' size in pt (density per file, not on the command line), then check with
   `pdfimages -list` that G4 stays CCITT and JPEG stays DCT (no recompression), and with `real.sh` that all pages have one size. Not measured.
2. **Coloured text as G4 + one colour** needs (a) a page signal for "colour only in text/flat boxes" vs "picture/diagram": the 4.5 page
   numbers (picture vs coloured-text `midtone` 0.98, `cc_big_frac` 0.95 on 7 vs 13 pages, uncertain labels) are too small to trust; measure
   leave-one-file-out on the #47 sources; (b) the number of ink colours per page (hue clusters of ink pixels; f7 has orange, blue and pale
   green); (c) the pale-box case (colour is the background, not the ink).
3. **Chrome/Firefox (pdf.js)** were not tried (4.4); a page-level mixed PDF uses only DCT and CCITT, so this is low risk, but check once with
   the first mixed output.
4. **Grey pictures** (1 example): nothing detects them; they need the page list.
5. **Multi-run best-of for an uncertain gutter/deskew (#45 and #46 are fixed now):** run 2–3 parameter variants and pick the output by an
   automatic score (gutter found without falling back, one page size across the document, ink fraction in the expected range). Only for
   pages that already hit "no confident gutter" today; cheap in Rust because pass 1 (measuring) can be shared across variants. Risk: a bad
   score function picks whichever variant games the score, not whichever is actually best — this is a safety net for genuinely ambiguous
   pages, not a substitute for fixing the detectors.

## 6. Splits on photographed spreads (#46)

### Cause (measured on f6, per column)
Not a dark band: **flattening turns the dark table white** (column darkness 0.00 over the table). What is left of the
table is a white area, the book edge and a short gap between the edge and the page. On f6 p2 that gap (x 1132–1221 of
4961, 23–25 %) lay inside the gutter's centre window (20–80 % of the *photo*) and was the widest ink-free run, so the
page got a "trusted" gutter at the table edge. With only one trusted page, its position became the document median, and
all four spreads were cut at 24 %. The result: a sliver of table (per-slot deskew 9.8° and 10.1°, the ±10° search edge)
plus an uncut spread. On p1/p3/p4 the real spine (~57 %) had a little shadow (ink in 2 of 48 bands), so there was no
ink-free run there. The valley fallback then chose the low-ink margin next to the table, whose outer side (the white table)
has no ink, so it was rejected. The same happened on f1 p6/p11/p22/p23 (gutter at 859–962 px, 20–22 %).

### Fix
The centre window is 20–80 % of the **text box's columns** (pass A's `content_box`), not of the photo. Widths, valley
sides and the left/right ink check stay fractions of the page. A first version cropped the profile to the box, which
made the width limit (`GUTTER_MAX_WIDTH_FRAC`) box-relative. That turned the blank facing page of f3 p2 (723 px, just
under 22 % of the page) into a valley cut through the left page's last line, so it was dropped. `cargo test`: `gutter_ignores_table_gap`
(synthetic spread, asserts the whole-photo search takes the gap and the box search the spine).

### Result (all four flags)
| file | pages | spreads with a changed gutter | max per-slot deskew | output |
|---|---|---|---|---|
| f1 | 50 → 50 | p6, p11, p22, p23: 20–22 % → 53–58 % | 1.5° (same) | the 4 spreads were a sliver + an uncut spread, now two pages each (looked at) |
| f2–f5, f7 | same | none | same | byte-identical |
| f6 | 8 → 8 | all 4: 24 % (median of one bad page) → 55–60 % | **10.14° → 3.33°** | 8 single book pages, no slivers (looked at) |

No page is left for review in any file (f6's 3 unsplit pages are gone; the median fallback isn't needed). Parity:
`BIN=… SIZE_TOL=3 tests/real.sh` gives the same failures as main. `sidste` is identical. On `skewed`, output p1 (already
a known mismatch, the #40 page, which is a near-blank half with the neighbour page's line ends in both) moves its cut
51 px from a run to a valley. It looks the same at 60 dpi, but a speck now counts as footer ink. Pass-A gutters of
the six older scans are otherwise unchanged.

### Not fixed / not needed
- The "darker than text is not text" cap (#45) was the first candidate, but it does not apply here: after flattening
  the table is not dark. The spine-shadow signal and a gutter score (issue tasks 2) weren't needed, and neither was
  a deskew cap: the 10° slots were the table slivers.
- **Dark spine/edge bands remain** on some split pages (f1 output p13, also on main, and p43, which is a page that
  main didn't produce correctly at all). They sit between the cut and the text. That is a crop question for the slot,
  not a gutter one. Measured and partly fixed in #53 (section 7); the rest is #55.
- The single-page median box for portrait pages among spreads (#46 comment) wasn't hit by any file and wasn't measured.

## 7. Dark edge bands in the slot (#53)

### Measurement (f1 output p13, 300 dpi, after the #46 fix)
Per column: nothing to x 190, then a band at 200–290 (ink 0.2–0.46 of the rows), a gap, a dotted rule at 320–340,
a gap to ~500 where the text starts. The band runs the whole height (rows 115–2858) but is **slanted/bowed**: no single
column has ink in more than 26 of 48 row bands. It is not a spine shadow in the middle but the book edge on the
outer side of the slot, and on f1 it is on nearly every left slot (22 of 50 output pages), also on f3 (7 pages) and
f7 (6 pages), a black edge line or rules next to it.

### What did not work
A column rule (a run of columns with ink in ≥ 80 % of the row bands): the slant makes every column too thin, even with
pinholes closed sideways. It also changed f4/f5, which have no band.

### What was built
`clear_edge_bands`: close 3 px pinholes, take 8-connected components, and whiten the ink of every component that is at
least 60 % of the slot height, narrower than 30 % of its width and lies in the outer 35 % of it. Text never makes a
component that tall. The slot size is unchanged (whitening, not cropping, so all pages stay one size).

### Result (all four flags)
| | before | after |
|---|---|---|
| pages with such a tall edge piece (100 dpi scan) | f1 22, f2 1, f3 7, f7 6 | f1 1, f2 1 |
| f2, f4, f5, f6 | | byte-identical |
| f1 p13, f3 p9, f7 p7 | band / edge line | gone, text and picture intact (looked at) |
| f1 p19, f2 p1 | picture frame, cover | unchanged (not bands) |
| `cargo test` | 7/7 | 8/8 (`edge_band_cleared_text_kept`) |
| `BIN=… SIZE_TOL=3 tests/real.sh` | same failures as main | unchanged |

### Not fixed
- **f1 p43**: the thin rules go, but the wedge at the top and the arc at the bottom are joined to the page edge, so
  the component is wider than 30 % of the slot and is left alone. The text of p43 is not touched.
- Whitening leaves the box where it was; text is not centred or aligned across pages. Order of passes, cropping vs
  whitening and alignment are #55.

## 8. Order of passes and a per-slot box (#55)

Question: in what order and in how many passes should rotate → crop → split → deskew → align → crop run, so that the dark
edge between the cut and the text goes away and the text sits at the same place on every page? **Status: the order is
decided and **the default with `--crop`** (#60, the knob `BITONAL_SLOTBOX` is gone). What to keep is decided per connected
component (`keep_box`, v10 below).** It meets the goals; the one open defect is a page number cut off before `keep_box` (see the end).

Measured by `runner` (numbers) and looked at by `checker` (pages), all four flags, f1–f7 and `tests/real/`. The scripts
are in the session scratchpad, not committed: `suite.sh` = run + `tests/measure.sh` + `tests/edge-bands.py`, plus a
per-column profile at 300 dpi. |L−R| = left minus right ink margin in mm (`measure.sh` ink box × page width). std x0/y0 =
spread of the ink box position over the pages, ‰.

### Order (decided)
1. **Pass A, per spread, unchanged:** get → orientation → flatten → ink map → spread box → gutter. The spread box is now only
   a *pre-crop*. It removes the surround cheaply (this is the owner's "crop to the white", which already exists), and it
   gives the gutter its search window (#46). Deskewing the whole spread first stays rejected: the halves skew in opposite
   directions (section 2).
2. **Plan, unchanged:** medians and split. The canvas is **not** fixed here any more.
3. **Pass B, per slot:** cut → deskew angle → rotate onto the slot's **own enlarged size** (w·cos+h·sin, plus a 3 % white
   rim, so no corner is cut and `content_box`'s 1.5 % edge rule never bites) → threshold → `clear_edge_bands` →
   **measure the box on the deskewed, cleaned bits** → keep that box packed 1 bit/px (about 1 MB per slot).
4. **Compose, new, no decode:** once every slot is known, one canvas, each slot placed by an integer shift (centred
   horizontally, top-aligned: the owner's choice), then G4 in parallel.

This is Scan Tailor's order (select content per page after deskew, then margins), with no third pass: keeping the bits
costs nothing in time (skewed 6.07 → 6.15 s, sidste 2.37 → 2.31 s wall). Re-decoding would add about 100 %.
**Crop vs whiten:** both. Whiten what `clear_edge_bands` finds, then crop, and let the canvas keep all pages one size.
Without `--crop` the path is untouched (byte-identical).

### Variants of the box in step 3 (what did not work)
| | f1 >2 mm (max) | f3 >2 mm (max) | f4 >2 mm (max) | f7 >2 mm (max) | std y0 f1 / f3 | page f1 (pt) | lost content (checker) |
|---|---|---|---|---|---|---|---|
| main | 24 (12.4) | 14 (27.1) | 6 (33.2) | 3 (4.2) | 26.9 / 59.2 | 575×724 | – |
| v1 crop to `content_box`, band rule off | 13 (2.8) | 0 (1.3) | 0 (1.4) | 2 (2.3) | 7.1 / 2.4 | 576×736 | **bottom page numbers on 8 pages** (ryg p2, skewed p8/13/15/17/20, flerspaltet p9, f1 p3), f2 p2 "147", f1 p11 margin labels; long lines cut on f3 p2/p3, f7 p3 (title), flerspaltet p1 |
| band rule on | 6 (7.4) | 1 (2.7) | 0 (0.5) | 4 (2.7) | 9.4 / 12.6 | 524×689 | headings and first lines on most f4/f7 pages, running heads + page numbers f1 p1–4/43/45 (all at the pre-crop's top/bottom edge, inside the 10 % band zone) |
| v2 = v1 + band rule on columns only + every inked gutter-side column | 14 (2.8) | 1 (2.7) | 1 (17.0) | 7 (2.7) | 7.1 / 9.9 | 576×736 | column rule removed nothing on f1 (the rules sit at 10–15 % of the slot, outside the zone); spine scraps pulled in (f4 p10, f6 p1) |
| v3 = v1 + gutter side tightened only to pass A's ink-free run | 26 (12.6) | 8 (32.0) | 4 (31.7) | 2 (10.4) | 7.0 / 2.4 | 576×736 | the lenient-grey gutter run is much narrower than the real white space, so centring breaks |
| v4: position from `content_box`, crop to all remaining ink | 28 (80.4) | 2 (65.1) | 7 (34.3) | 13 (84.1) | 19.9 / 40.0 | **770×792** | footers back (ryg ok, skewed 45/46), but stray ink far from the text (rules not whitened, scraps at the cut) widens the canvas for every page, +34 % on f1 and +46 % on f7 |

Also measured on every variant: one page size per file, `cargo test` 8/8, `tests/edge-bands.py` f1 1 → 0 (v1, v4). f2's
cover stays flagged (not a band). By eye, the bands on f1 p19/p23/p43/p45 are **unchanged in every variant**: joined to
the page edge, or thin rules shorter than 60 % of the slot, so `clear_edge_bands` leaves them. v1 centred well (checker:
within about 5 px), but the top margin still jumped 20–50 px where a stray dash or a partial line sat above the heading.

**Lesson:** `content_box` finds a *text block*. It drops sparse things by design (the minimum density, the 8 % near
window, the band rule). That was harmless in pass A, where the spread geometry left slack around the box. As an exact
crop it loses page numbers and long lines. "All ink" goes the other way: it keeps every scrap and lets one of them set the
page size. Neither is "what to keep".

### The keep rule per component (v5–v9, in the code)
`keep_box` labels the connected components of the cleaned, deskewed slot and decides for each one (l = median letter
height inside the text block, d = 4 l, `BITONAL_KEEP_D`):
- **seeds:** components that meet the text block (`content_box`, band rule off);
- **join, repeated until nothing changes:** anything within d of the kept box; anything straight above/below it (inside
  its columns) at any distance if it is more than a speck (l²/8 px): page numbers, running heads, footnotes; a letter-sized
  piece above/below the text block within 12 l sideways (a page number centred under a one-column page);
- **never:** big sparse pieces (< 10 % filled, > 100 l²) reaching out of the block (book edges, the L on f7 p3); pieces
  larger than 3 l within 2 px of a *cut* side of the slot (a side where the pre-crop reached the page edge is not a cut:
  a cover runs off the page); thin pieces longer than 25 % of the slot (rules; f7 p4's 160 px design bar is shorter).
  The last two do not apply when the piece reaches the middle half of the block (a full-bleed picture).
- Everything not kept is whitened, the box is the kept ink + the content_box pad, and compose centres it and aligns its top.

| | f1 >2 mm (max) | f3 | f4 | f5 | f7 >2 mm (max) | std y0 f1 / f3 | page f1 (pt) | lost content (checker) |
|---|---|---|---|---|---|---|---|---|
| main | 24 (12.4) | 14 (27.1) | 6 (33.2) | 0 (0.6) | 3 (4.2) | 26.9 / 59.2 | 575×724 | – |
| v5: seeds + join within d | 6 (22.0) | 0 | 0 | 0 | 3 (11.9) | – | 583×736 | not looked at; footer flags lost on the same pages as v1 (ryg p2, skewed p8/13/15/17/20), for d = 2, 4 and 8 l alike |
| v6: + straight above/below within 20 % | 6 | 0 | 1 | 0 | 3 | 4.1 / 13.0 | 583×736 | f1 p3 "14", flerspaltet p9 "8", f7 p4 design bar (rule rule), f5 p1 cover strip (edge rule at the page edge); L-shaped edges kept as seeds |
| v7: + frame rule, cut sides only, rule ≥ 25 %, any distance above/below | 5 | 0 | 1 | 0 | 4 | 2.0 / 13.8 | 583×736 | f1 p3 "14", flerspaltet p9 "8" (both touch the pre-crop edge) |
| v8: + small pieces at a cut kept | 4 (20.2) | 0 (0.5) | 0 (0.5) | 0 (0.4) | 5 (11.9) | **0.9** / 15.5 | 583×736 | f1 p3 "14", flerspaltet p9 "8" (62 px beside a one-column page), f1 p18 clipped "29" |
| **v9: + letters above/below the block within 12 l** | 4 (20.2) | 0 (0.5) | 0 (0.5) | 0 (0.4) | 5 (11.9) | **0.9** / 15.5 | 583×736 | **f1 p3 "14", f1 p18 clipped "29"**; skewed p1's clipped line ends of the *neighbour* page (the #40 page) |

v9, all files: one page size per file, within +1.5 % (w) / +1.7 % (h) of main on f1, narrower on skewed; `tests/edge-bands.py`
0 on every file (main: f1 p19, f2 p1); `cargo test` 8/8; time +2–5 % (skewed 5.69 → 5.98 s). `tests/real.sh`: page count and
footers as main except skewed p34/35/43/44 (main's "footer" there was a cut-off partial line, not a page number) and flerspaltet
p1 (title page, all content present). Checker, about 40 pages: every page number, running head, footnote, margin label,
design bar, heading, line and picture present except the two above; top of the text at 18–24 px on every page of a series
(main 39–96 px); horizontal position follows the layout. The remaining |L−R| outliers are content on one side (BOKS labels,
the OPLYSNING side label, a neighbour-page slice already in main), not scraps. Bands: removed or reduced on most pages;
f1 p43, f2 p3/p5, f6 p1 keep a bottom curl or left bar as in main (joined to text or not thin enough), none is new.

### v10: page numbers beside the running head (#60), made default
The running head is inside the text block, so the header/footer join never saw a number on its line. A second, single pass
after the join has converged adds a letter-sized piece (at least l/2 tall) whose rows are within 1.5 l of the top or the bottom
of the kept text lines (measured from kept pieces at least l/2 tall, so a dash above the head does not move the line), from up
to `KEEP_HEAD_D` = 24 l sideways (f1 p3: 18 l; the header/footer zone's 12 l was too short). Guard: not when 3 or more other
narrow pieces (≤ 3 l wide, rule segments included) stand in its columns, spread over more than `KEEP_RULE_FRAC` of the slot
height: that is a dotted rule. Two things that did not work and why: (1) letting the pass iterate, the kept number enlarged
the box and a rule dot 3.5 l beyond it joined by the ordinary near rule, and the rest of the rule followed (f1 p5, p11);
(2) measuring the head line from the kept box's top, a thin dash above the head put the line 130 px too high (f1 p3).
The pass only applies the head-line rule, no near/zone joins.

Reproduced (`tests/slotbox-suite.sh`, now the default): f1 4 pages > 2 mm (max 20.2), f3/f4/f5 0, f7 5 (11.9), std y0 f1 0.9 /
f3 15.5, page f1 583×736, edge bands 0 on every file, `cargo test` 12/12 (new: `keep_box` page number far below / on the cut
edge / thin edge rule dropped / head number vs dotted column, `compose` equal margins), `tests/synth.sh` all PASS,
`tests/real.sh`: page count and footers as main except skewed p34/35/43/44 and flerspaltet p1 (as v9), facts re-recorded
after a checker look. Checker against main: f1 p3 "14", p5 "16", p24, p28, p38, p46 numbers now present; every page number,
head, label, title of the list in #60 present; no edge rule or blob that main lacks; f1 p1/p11/p13/p43's left dotted rule
is unchanged from v9. f6 p2 gets one 1 px, 15 px tall tick beside the page number. f5 p1: the masthead strip cut off at the
top of the cover is dropped (a big piece at a cut side, as in v9).

**Still open:** f1 p18 "29" and p24 "35" are cut off (p18 at the right edge, p24 at the bottom) already in the plan box, before
`keep_box` sees the slot; main clips them the same way. Widening the crop/split plan box is a separate change.

### Covers: the edge rule is off when many pieces touch a cut (#66)

`keep_box` whitened masthead, date line and the circle text on f5 p1 as `edge+big` (a big piece within 2 px of a cut side):
on a cover the text runs off the slot, so it touches the cut. New rule: when **more than 16 big pieces** (`KEEP_EDGE_MAX`)
touch a cut side, `edge+big` is off for that slot (a book edge is a few long pieces, content running off is many). Frames
and rules are unchanged. The 16 was measured on the seven files: 26 on f5 p1, at most 12 anywhere else. It is a rule of
thumb and can fail on covers with fewer such pieces; those stay whitened. Effect on f1–f7 and `tests/real`: only f5 differs
from main (`tests/verify.sh`), `BITONAL_WHITENED=1` lists the same rows on all other files. Result on f5 p1, judged by `checker` against the source: date line partly readable (was unreadable), masthead still
unreadable (a few clipped black bars), circle text still missing (that area is white in the output, as in main). So the rule
lets the pieces through but does not by itself make masthead and circle legible; #66 stays open for that. Not this issue: the
cover photo is rendered badly by the threshold (identical in main).

### Where the f5 p1 circle text goes (#70)

Investigated, not fixed. Bits dumped for f5 p1 (canvas 2649×3255 at 300 dpi, canvas px, not the slot px of the issue) after `hyst`,
after `clear_edge_bands` and after `keep_box`: the three dumps are identical in the circle area, so neither `clear_edge_bands` nor
`keep_box` touches it (`keep_box` cannot whiten what is not there). In the lower-right rectangle x 1620–2540, y 2720–3100 the
ink after `hyst` is 4193 of 350 600 px (1.2 %, speckle) and the grey input to `hyst` has mean 232/255, 5th percentile 196, 0.36 % below 100:
the right circle's light-purple fill and its white text are already near-white before the threshold. The hard vertical edge at
x ≈ 1615 is where the darker left circle ends and the lighter one starts, so the left circle's text stops there. The cause is
therefore the flatten (pass B, `BLUR_SIGMA`) plus the threshold, not the keep rule, `compose` or the box. Not checked: which of
flatten and threshold alone would keep the text, and whether the rectangle's coordinates map exactly onto the issue's range.
The `BITONAL_WHITENED` list proves `keep_box` only, not the whole pipeline. The dump code was temporary and is not in the tree.

### Covers: flatten, not the threshold, whitens the circles (#71)

**Cause measured, changed for covers.** The #70 rectangle (x 1620–2540, y 2720–3100) is in the pixels of the deskewed slot
`c` (2649×3255), not of the composed canvas (2586×3193, offset keep − (31, 37)): with that mapping the dump reproduces #70
exactly (4193 ink px after `hyst`, mean 232). Three dumps in `finish`, all sampled on `c`'s pixel grid (temporary code, not in
the tree), f5 p1, all four flags:

| stage | mean /255 | p5 / p50 / p95 | < 0.45 | < 0.60 | ink after `hyst` |
|---|---|---|---|---|---|
| D1 after `load` (before flatten) | 114.0 | 64 / 109 / 240 | 86.3 % | 92.5 % | 327 239 (93.6 %), `hyst` on it through the same crop + rotate |
| D2 after `flatten` (page) | 232.2 | 194 / 238 / 255 | 0.56 % | 1.17 % | – |
| D3 before `hyst` (slot) | 232.2 | 196 / 238 / 255 | 0.55 % | 1.16 % | 4193 (1.20 %) |
| `BITONAL_SLOTFLAT=1`: D1 / D3 | 115.7 / 230.1 | – | 85.4 / 1.1 % | – | 324 322 / 7046 |

So the fill is not light: its luma is ≈ 0.43 (78 % of the rectangle in the 102–127 bin), the text is ≈ 1.0 (5 % in 230–255).
`flatten` divides by a σ = 30 px blur; the circles are far wider, so the fill becomes ≈ 1.0 and the white text (> 1.0) is
clipped to 1.0 with it. That is by construction (`min(1.0)` in `flatten`) and no threshold after it can bring it back: the
CLI threshold on the whole file gives 6830 ink px at 70 % and 33 812 (9.7 %) at 80 %, still no text. The threshold alone on
the unflattened grey makes the fill ink and leaves the text white: all three circles readable, but the photo nearly all black.
Flattening the slot after the rotation (SLOTFLAT) changes nothing essential (same σ).

**Blur width** (offline on the unflattened slot, Python, white border outside the slot, not the Rust `gauss` clamp):
σ 30 / 120 / 300 / 1000 px → slot ink 9.1 / 22.1 / 35.9 / 66.2 %, rectangle 1.2 / 4.8 / 19.0 / 82.1 %. By eye only σ 1000 makes
all circle text readable; 300 reads parts of it.

**What was built:** `keep_box` also returns its cover signal (the #66 rule: more than `KEEP_EDGE_MAX` big pieces touch a
cut side; only f5 p1 on f1–f7 and `tests/real`). For such a slot `finish` re-decodes the page, applies the same crop + deskew
to the unflattened grey, flattens it with `COVER_BLUR_SIGMA` = 1000 px (light falloff only), thresholds, clears bands and runs
`keep_box` again on that. Text pages never see it. New test `cover_flatten_keeps_fill_around_white_text`. Not tried: a
local/adaptive threshold other than the blur width (flatten + fixed threshold already is one, window σ); a grey cover page
(needs a grey/JPEG page writer, section 5.7 issue 1; estimated from the D1 slot at 150 dpi: Flate 1.30 MB, JPEG q65 luma
177 kB, against 51.6 kB for the whole G4 page now).

**Result** (`BASE_BIN=<main> tests/verify.sh`, raw output in the PR): cargo test 13/13, `tests/synth.sh` all PASS, `tests/real.sh`
ok, suite edge bands 0 on every file, f5 page size unchanged (620.64×766.32 pt). Only f5 differs from main, and in it only
p1 (the images of p2–p7 are byte-identical). Every other file is byte-identical, so its size is too. f5: 342 557 → 336 842 B
(p1's image 57.2 → 51.6 kB). f5 p1: 4193 → 287 149 ink px in the rectangle, page ink 8.6 → 68.9 %. Time: f5 1.35–1.41 →
3.06–3.15 s wall (3 runs each); pass B for p1 2266 ms against ~850 ms for the other pages (re-decode plus threshold, bands and
components on a mostly black slot); only a cover slot pays that. Nothing from the #60 list can be lost, since no other page
changed (f5 p1 has no page number).

`checker` against source and main, f5 p1 (100 dpi):
- The right circle is readable in all three lines (main: missing). "ANDET UDKAST" has speckle and a white wedge at the circle's
  edge.
- The middle circle, the headline circle with its subtitle, and the logo are readable, clean white on black (main: partly,
  hollow letters).
- The date line is partly readable, better than main. "Nr. 1" has a blob, and a vertical line crosses "September".
- The photo is recognisable, but posterised into large black areas (main: the boy blown out to white).
- Nothing present in main is missing.

**Still open:**
- The masthead "MUSIKLÆREREN" is unreadable in both main and new. Probable cause is the pre-crop box, which cuts covers by
  17–45 % (section 5.1); not verified.
- The cover rule is tuned on one page (26 big pieces against at most 12 elsewhere). A cover below the count keeps the old
  rendering, and there is no hold-out (#47).
- Readability is `checker`'s judgement at 100 dpi, not a number.
- `BITONAL_WHITENED=1` lists both `keep_box` passes for a cover slot.
- The text/fill colours in the source were not measured (luma only).

## 9. B4c: colour policy with information over size (#74)

Question: what keeps colour or grey, and who decides (user, automatic, automatic with a printed suggestion), now that
**keeping information matters more than the smallest file**. §5 decided the same with bytes as the cost. Tags as in §5:
**[measured]**, **[estimate]**, **[unknown]**, **[labels?]**, **[scans]**. Nothing in the pipeline changed.

### 9.0 Measurements
Reproduce (`D="tests/real/new pdfs dont upload"`, `W="$D/work"`; outputs in `$W/m3/`, git-ignored):
```
python3 tests/mixed-eval.py $W/det.tsv "$D" $W/m3/geom.txt $W/m3/slots.txt $W/base $W/m3/base40 --policies   # ~21 s (M1, M2)
python3 tests/ink-colours.py $W/det.tsv "$D" $W/m3/geom.txt                                                   # ~15 s (M3)
```
- **M1** (`--policies`): size and region outcome per page-list policy. Region outcome over the 46 labelled regions plus f7 p7
  (keep page without a box, counted as one page-sized coloured-text region) = 47: *kept* (every output slot it touches is a
  JPEG page), *partial*, *lost* (a picture or cover left as 1-bit), *degraded* (a diagram or coloured text left as 1-bit,
  SSIM 0.79–0.85 in §5.2). The crop-box loss (8 of 46 regions cut by 17–45 %, §5.1) hits every policy alike and is not in these counts.
  Cuts leave-one-file-out as §5.1. The existing rows reproduce §5.6 exactly (5.93 / 10.87 / 12.72 / 15.94 / 14.89 MB).
- **M2** (in `--policies`): every MODE=images page JPEG re-encoded q65 as colour 4:2:0 and as luma, from the same page JPEG.
- **M3** (`tests/ink-colours.py`, new): per RGB-source input page (inside the pass-A crop box, 150 dpi) and per labelled box:
  number of hue clusters of colour pixels (chroma ≥ 60), hue concentration, colour area, luma spread of the colour pixels,
  distance to a 4-colour k-means fit (flat vs continuous colour), tinted share of light pixels (the pale box of §5.9 2c).
  Question: pages whose only non-text content is coloured text (P, 7) vs pages with a picture, diagram or cover (N, 29);
  text-only RGB pages (T, 16) shown at the same cut.
- **M5 not done** (text legibility on colour pages, JPEG 150 dpi vs G4 300 dpi, by Tesseract word agreement). It could only
  reopen MRC/region layers, which need a region detector that does not exist (§5.1); several sources are themselves
  150–200 ppi JPEGs (f5, f6), so 300 dpi gains little there; and OCR agreement on Danish scans is a noisy proxy. [unknown]

### 9.1 The candidates
Level × treatment, where numbers exist (✔ in §4–5 or 9.0, ◐ crop or single point only, ✗ none):

| | 1-bit G4 | grey JPEG | colour JPEG q65/150 | mask + colours | MRC | #71 cover threshold |
|---|---|---|---|---|---|---|
| output slot | ✔ | ✔ (M2) | ✔ | ◐ 2 crops (§5.3) | ✗ | ◐ f5 p1 only |
| whole input page | – | ✔ (M2) | ✔ | ✗ | ✗ | – |
| region | – | ✗ | ✔ (§5.4) | ✗ | ◐ MRC-lite crop, viewers ok | – |
| document mode | ✔ | ✔ (M2) | ✔ | – | – | – |

Page-level policies (M1) [measured, labels?, scans]; "text slots in colour" = JPEG output pages with no keep content (of 66):

| policy | decided by | size | × text | JPEG pages | text slots in colour | kept / lost / degraded (of 47) | not kept | grey instead of colour |
|---|---|---:|---:|---:|---:|---|---|---:|
| D0 text only (today) | user list | 5.93 MB | 1.00 | 0 | 0 | 0 / 22 / 25 | every region | – |
| label list (reference, a perfect list) | – | 10.87 MB | 1.83 | 42 | 0 | 47 / 0 / 0 | – | 1.74× |
| D1 `auto`, tiles in crop box, R 0.99 | automatic | 15.94 MB | 2.69 | 91 | 29 | 47 / 0 / 0 | – | 2.54× |
| D1′ tiles R 0.8 | automatic | 14.89 MB | 2.51 | 84 | 24 | 45 / 0 / 2 | f1 p1 diagram?, f1 p2 coloured text? | 2.37× |
| D2 page `hasler` (cut = lowest keep page of the other six files) | automatic | 14.28 MB | 2.41 | 75 | 17 | 45 / 0 / 2 | same two | 2.28× |
| **D3 every page whose source is RGB** | the source | 15.94 MB | 2.69 | 91 | 29 | 47 / 0 / 0 | – | 2.54× |
| D4 document mode: D2 share ≥ 0.5 → all JPEG, else D2 | automatic | 16.86 MB | 2.84 | 99 | 37 | 47 / 0 / 0 | – | 2.69× |
| D4 with share ≥ 0.8 | automatic | 14.28 MB | 2.41 | 75 | 17 | 45 / 0 / 2 | as D2 | 2.28× |
| D5 input page 1 only | position | 7.16 MB | 1.21 | 11 | 2 | 6 / 18 / 23 | all but the covers | 1.19× |
| D2 + D5 | automatic | 14.51 MB | 2.44 | 77 | 17 | 46 / 0 / 1 | f1 p2 coloured text? | 2.31× |

(? = uncertain label. D2 shares per file: f1 0.60, f2 0.28, f3 0.11, f4 0.60, f5–f7 1.00; with n = 7 files the document
cut is a sanity check, not an estimate.)

- **D1 and D3 choose exactly the same 91 output pages** [measured]: at R 0.99 the tile detector flags every RGB-source slot
  on this material. D3 gets the same result with no detector. Whether they differ elsewhere is [unknown] (#47).
- **The #66 cover signal** catches 1 of 4 covers (f5 p1, §8) and **page 1** is a cover in 4 of 7 files (f2, f3, f5, f7);
  neither is a policy on its own, and D2 + D5 only recovers f1 p1.
- **Grey JPEG saves 4 %** against colour (per file 0.94–0.99, re-encoded colour / original 1.02) [measured]. At 4:2:0 the
  chroma is a small part of a JPEG; grey pays with the hue (a pie chart's legend) for almost no bytes. Grey only makes sense
  where the source is grey, and there the colour JPEG is grey anyway.
- **The #71 cover threshold** stays what it is: the text-mode rendering of a cover. As a colour treatment it has no role
  once covers are JPEG pages.

Per candidate, how it fails (FN = a keep page left 1-bit, FP = a text page as JPEG) and whether the user can undo it:

| candidate | FN | FP | needs | undo |
|---|---|---|---|---|
| D0 | every figure (22 lost, 25 degraded) | – | nothing | `--colour-pages LIST` (#76) |
| D1 | pages without one flagged tile; none here | 29 text slots, +114 kB each, no damage | #75, #76, detector + a decision column in `--detect-eval` | printed list, LIST / `none` |
| D2 | f1 p1, p2 (pale colour: no pixel with chroma ≥ 60, M3) | 17 text slots | #75, #76, `hasler` per page | printed list |
| D3 | a figure on a 1-bit or grey source page (none here; the grey engraving f1 p17 is on an RGB page) | 29 text slots | #75, #76, the source's colour space | printed list |
| D4 | a file with few figures and low share keeps only D2's pages | whole files as JPEG (f5–f7 here, 37 text slots) | as D2 | a per-file flag |
| mask + colours | – | a picture becomes a silhouette (SSIM 0.55–0.59, §5.3) | a signal (M3: none), a new page type | only if printed |
| MRC / regions | a third of the regions at a low FP rate (§5.1) | FP boxes on all 16 RGB text pages | region detector, layers | no (not a page list) |

### 9.2 Ink colours and flat colour (M3; §5.9 item 2)
[measured, labels?, scans] Each signal alone, leave-one-file-out. *Safe cut* = no N page of the six training files passes
(the §5.1 logic: a picture must never become a silhouette); *balanced* = best balanced accuracy on the six.

| signal | AUC P vs N | per file [min–max, files] | safe cut: P / N / T passed | balanced: P / N / T |
|---|---:|---|---:|---:|
| hue clusters k | 0.64 | 0.67–0.96, 3 | 0/7 / 0/29 / 0/16 | 5/7 / 14/29 / 13/16 |
| hue concentration (top two 30° windows) | 0.65 | 0.55–0.96, 3 | 0/7 / 1/29 / 0/16 | 2/7 / 18/29 / 14/16 |
| colour area | 0.80 | 0.80–0.93, 3 | 0/7 / 1/29 / 8/16 | 6/7 / 10/29 / 16/16 |
| luma spread of colour pixels | 0.51 | 0.70–0.96, 3 | 0/7 / 3/29 / 2/16 | 0/7 / 6/29 / 9/16 |
| 4-colour fit distance | 0.57 | 0.60–0.96, 3 | 0/7 / 1/29 / 0/16 | 1/7 / 18/29 / 9/16 |
| tinted paper | 0.82 | 0.75–0.93, 3 | 1/7 / 1/29 / 8/16 | 3/7 / 8/29 / 16/16 |

Labelled boxes (12 coloured text vs 34 picture/diagram/cover, signals inside the box): AUC 0.57–0.72, safe cut 0/12 on every signal.

- **No signal gates mask + colours.** The safe cut passes 0–1 of 7 coloured-text pages and still lets a picture through;
  the balanced cut turns 6–18 of 29 figure pages into silhouettes.
- **Number of ink colours:** P pages have k = 0–2, N pages 0–3, text-only pages 0–3. f7 shows blue (205°) and orange
  (15–25°) as §5.9 said; the pale green box has chroma below 60 and shows only as tint.
- **Why** [measured values, the reading is an estimate]: flatness works on the flatbed scan (f5: coloured heading 0.015,
  pictures 0.06–0.13) but not on the photographed files, where coloured-text pages sit at 0.09–0.14 like pictures. A warm hue
  at 15–45° appears on nearly every photographed page (paper tint under the photo light, the f7 lamp glow; not verified).
  f1 p1 and p2, D2's two misses, have no pixel at chroma ≥ 60 at all.
- **Observation, not tested:** a flat diagram (f5 p4 pie chart, fit 0.015) would suit mask + k colours as well as coloured
  text does, so the useful border may be flat vs continuous rather than coloured text vs the rest. That grouping was made after
  seeing the numbers; it needs the #47 sources.

### 9.3 What the new priority changes in §5, and what not
- **§5.2 default: changes.** §5.2's asymmetry (a miss is irreversible, a false alarm costs bytes) already pointed recall-first;
  the text-only default was a judgement on bytes. With information first, the default becomes a recall-first page list that
  is **always printed**, with `--colour-pages LIST` and `none` as the override.
- **§5.7: `auto` moves** from a printed suggestion to the default, and prerequisite 2 (the crop box cuts 8 of 46 regions,
  covers 27–45 %) rises from a flaw to an information loss in every policy: #76's crop decision for listed pages is now the
  largest remaining loss.
- **§5.3 mask + colour: weaker.** It drops the fill of coloured boxes (a loss) and M3 found no signal to gate it.
- **Unchanged:** §5.1 (no usable tile cut, one class), §5.4 (region layers/MRC rejected on a 5 % gain; information would only
  reopen them if 150-dpi JPEG text proves to lose content, M5), §5.5 codec (JPEG q65 150 dpi; grey not worth it, M2).
- **Not tried, and why not worth it here: HDR-style fusion** (several synthetic exposures of one scan, merged). The input is one
  8-bit capture, so synthetic exposures add no information; for JPEG pages there is nothing to recover. The 1-bit analogue is
  real, though: §8's #71 shows one blur width cannot keep both white-on-fill text (σ 1000) and photo tones (σ 30). Choosing per
  component between thresholds at several blur widths is exposure fusion for bits; it belongs to text mode (B2, covers), not
  to this policy. Not measured.

### 9.4 Recommendation
**D3: a slot is a JPEG page when its source page is RGB, otherwise G4; the list is printed; `--colour-pages LIST|none`
overrides.** 2.69× text, all 47 regions kept, 29 of 66 text slots in colour, no detector [measured, labels?, scans].
It is the simplest rule that loses nothing here, it is predictable (the user can tell from the source why a page is in colour),
and it inherits no detector error.

Alternatives:
1. **D2 page `hasler`:** 2.41× (−10 % against D3), 12 fewer text slots in colour, but degrades f1 p1 and p2 (pale colour,
   both uncertain labels). The pick if size must still count a little.
2. **D1 `auto`:** identical to D3 here and needs the detector; only worth it if #47 shows RGB-source text pages to be common
   *and* the detector to skip them without losing figures (§5.7: FP rate < 0.05 at no lost region on a held-out source).

Where D3 fails: an all-RGB colour scan of a plain text book becomes MODE=images (3.5×, no loss); a figure on a 1-bit or
grey source page stays 1-bit (the list fixes it). Unverifiable here: seven scans of one owner's books, one labeller (me),
19 uncertain pages, no rendered pages with figures, no back covers, one grey picture, one cover that fires #66; #47 is missing.

### 9.5 Issues this implies
- **B4d (#75):** unchanged, no luma page needed (M2).
- **B4e (#76):** add `none`, and the default list comes from the policy (D3), printed like the gutter warning; the crop
  decision for listed pages is the top priority there (covers lose 27–45 %).
- **B4f (#78):** default policy D3 (+ printed list), after #75/#76; re-measure D1/D2/D3 on the #47 sources before release.
- **B2j (#79):** the per-component blur-width choice of 9.3 (text mode, covers).
- §5.7 conditional issues 3 (`auto` detector) and 4 (coloured text as G4 + colour) stay conditional; issue 4 now also needs
  the flat-vs-continuous signal of 9.2 on non-photographed sources.
