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
- **Pages with little text** (figures, blank facing pages, title pages): the confidence gate is the only protection and
  is untested. A whole-document vote (idea 3 in the issue: run on a few pages, apply the majority) was not needed for
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
