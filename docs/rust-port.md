# Rust port: decisions and measurements

Spike code: `rust/` (one file, `src/main.rs`). Build: `cd rust && cargo build --release`; the binary takes the
`bitonalpdf.sh` CLI, so `BIN=rust/target/release/bitonalpdf tests/synth.sh` works. Text mode only; `--rotate`,
`--crop`, `--split auto|N%`, `--deskew` exit with "not in the Rust spike yet" (#29). `BITONAL_TIMING=1` prints
per-page source (`image`/`render`) and stage times, and `BITONAL_RENDER=1` forces the render path.

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
  written": they need `--crop`/`--split`, #29).
- `tests/real.sh --split off`: runs through all six, FAIL on the facts because they were recorded with all four flags
  (sizes/page counts after crop+split). The per-page comparison against bash without flags is the one above.
- `cargo test`: round trip bitmap → G4 → PDF → hayro → extraction → same bitmap and page size. Shown to fail with the
  G4 polarity flipped and with the fit check broken.

### Dithered 1-bit pages (open, goes to #29)
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
