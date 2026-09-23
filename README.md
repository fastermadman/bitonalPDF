# bitonalPDF

Shrink scanned PDFs on macOS without hurting the text. A 59 MB, 166-page book scan became 9 MB and
stayed fully readable. Preview's Quartz filters and Compressor often make such files *bigger*; this
never does (if the result isn't smaller, no file is written).

Each page is rasterised, its background is flattened (so shadows from microfilm or book spines don't
break the threshold), and it is stored as a 1-bit Group4 image. There is also a colour mode for books
with pictures.

| Mode | For | How | Typical result |
|---|---|---|---|
| `text` (default) | pure text scans: typewriter, print | 300 dpi, 1-bit Group4 | 55–85% smaller in tests |
| `images` | books with pictures/diagrams | 150 dpi colour JPEG | varies; skipped if not smaller |

`text` drops colour and greytones on purpose — don't use it on books with figures.
Both modes turn pages into images, which is fine for scans (they have no text layer to lose) but not
for born-digital PDFs.

## Install

macOS only. Needs [Homebrew](https://brew.sh) packages:

```bash
brew install poppler imagemagick
git clone https://github.com/FasterMadman/bitonalPDF && cd bitonalPDF
./build.sh            # builds bitonalPDF.app; drag it to ~/Applications
```

The app is not notarised, so macOS asks for confirmation the first time (right-click → Open).

## Use

**Droplet:** drop one or more PDFs on `bitonalPDF.app`, choose *Pure text* or *With pictures*, follow
the progress bar in the Dock. The result lands next to the original as `<name>.1bit.pdf` or
`<name>.shrunk.pdf`. The original is never touched.

**Terminal:**

```bash
./bitonalpdf.sh scan.pdf                    # text mode
MODE=images ./bitonalpdf.sh book.pdf        # colour mode
./bitonalpdf.sh scan.pdf out.pdf 65         # bolder text (threshold %, default 60)
```

## Known limits

- Not tested on double-page scans or badly rotated pages (they stay as they are).
- The threshold is fixed at 60 in the droplet; use the terminal to change it.

## License

[AGPL-3.0](LICENSE). `poppler` and `imagemagick` are run as separate programs, not linked.
