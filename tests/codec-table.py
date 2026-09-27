#!/usr/bin/env python3
"""Codec table for #43 / #38 question 4: bytes, PSNR and SSIM of one crop per codec/quality setting.

Usage: tests/codec-table.py crop.png [crop2.png ...]   (300-dpi PNG crops of the content types; prints a TSV,
                                                         codecs + the mixed/MRC-lite rows below)
       tests/codec-table.py --viewers crop.png        (embeds the crop with each PDF filter, renders it in every viewer found)
       tests/codec-table.py --mrc-viewers             (no crop: synthetic PDF with an ImageMask over a JPEG region and a
                                                         stencil /Mask at a different resolution, checked pixel-by-pixel
                                                         in every viewer found)
Needs: magick, cwebp/dwebp, opj_compress/opj_decompress, avifenc, img2pdf, numpy, Pillow; viewers: pdftoppm, gs,
qlmanage (macOS PDFKit); the mixed rows also need rust/target/release/bitonalpdf (build with cargo build --release).
The crops are not committed (they come from copyrighted scans); see docs/rust-port.md section 4 for how they were cut.
"""
import glob, os, subprocess, sys, tempfile, time, shutil, zlib
import numpy as np
from PIL import Image

BITONALPDF = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'rust', 'target', 'release', 'bitonalpdf')

def run(*a, **k):
    return subprocess.run(a, capture_output=True, text=True, **k)

def metric(ref, img, m):
    r = run('magick', 'compare', '-metric', m, ref, img, 'null:')
    out = (r.stderr or r.stdout).split()
    # ImageMagick 7 prints "<value> (<normalised>)"; for SSIM the normalised value is (1 - SSIM) / 2 (checked
    # against an own Gaussian-window SSIM: 0.01776 vs 0.01773), for PSNR the dB value is the first number (120 = identical).
    return 1 - 2 * float(out[1].strip('()')) if m == 'SSIM' else float(out[0].replace('inf', '999'))

# name -> (encode command builder, decode command builder); {i} input png, {o} encoded file, {d} decoded png
CODECS = {
    'JPEG q90 4:2:0': ('jpg', lambda i, o: ['magick', i, '-quality', '90', '-sampling-factor', '4:2:0', o]),
    'JPEG q75 4:2:0': ('jpg', lambda i, o: ['magick', i, '-quality', '75', '-sampling-factor', '4:2:0', o]),
    'JPEG q50 4:2:0': ('jpg', lambda i, o: ['magick', i, '-quality', '50', '-sampling-factor', '4:2:0', o]),
    'JPEG q75 4:4:4': ('jpg', lambda i, o: ['magick', i, '-quality', '75', '-sampling-factor', '4:4:4', o]),
    'PNG lossless (Flate)': ('png', lambda i, o: ['magick', i, '-define', 'png:compression-level=9', o]),
    'PNG-8 256 colours (Flate, /Indexed)': ('png', lambda i, o: ['magick', i, '-colors', '256', '-define', 'png:compression-level=9', 'PNG8:' + o]),
    'JPEG 2000 lossless': ('jp2', lambda i, o: ['opj_compress', '-i', i, '-o', o]),
    'JPEG 2000 ratio 10': ('jp2', lambda i, o: ['opj_compress', '-i', i, '-o', o, '-r', '10']),
    'JPEG 2000 ratio 30': ('jp2', lambda i, o: ['opj_compress', '-i', i, '-o', o, '-r', '30']),
    'WebP q90': ('webp', lambda i, o: ['cwebp', '-quiet', '-q', '90', i, '-o', o]),
    'WebP q75': ('webp', lambda i, o: ['cwebp', '-quiet', '-q', '75', i, '-o', o]),
    'WebP q50': ('webp', lambda i, o: ['cwebp', '-quiet', '-q', '50', i, '-o', o]),
    'WebP lossless': ('webp', lambda i, o: ['cwebp', '-quiet', '-lossless', i, '-o', o]),
    'AVIF q60': ('avif', lambda i, o: ['avifenc', '-q', '60', '-s', '6', i, o]),
}

def decode(ext, enc, dec):
    if ext == 'jp2':
        return run('opj_decompress', '-i', enc, '-o', dec)
    if ext == 'webp':
        return run('dwebp', enc, '-o', dec)
    return run('magick', enc, dec)

def table(crops):
    print('crop\tcodec\tbytes\tratio_vs_png\tenc_s\tpsnr_db\tssim')
    for c in crops:
        base = os.path.basename(c)[:-4]
        png = os.path.getsize(c)
        with tempfile.TemporaryDirectory() as t:
            src = os.path.join(t, 'in.ppm')  # opj_compress reads ppm, not png
            run('magick', c, '-alpha', 'off', src)
            for name, (ext, cmd) in CODECS.items():
                enc, dec = os.path.join(t, 'e.' + ext), os.path.join(t, 'd.png')
                inp = src if ext == 'jp2' else c
                t0 = time.time(); r = run(*cmd(inp, enc)); dt = time.time() - t0
                if not os.path.exists(enc):
                    print(f'{base}\t{name}\tFAILED\t\t\t\t{r.stderr.strip()[:60]}'); continue
                decode(ext, enc, dec)
                b = os.path.getsize(enc)
                print(f'{base}\t{name}\t{b}\t{b / png:.3f}\t{dt:.2f}\t{metric(c, dec, "PSNR"):.2f}\t{metric(c, dec, "SSIM"):.4f}')
                os.remove(enc)
            mixed_rows(c, base, png, t)

# crops live in .../work/crops/<name>.png; composites we want to look at go in the sibling .../work/m2/
def save_dir(crop):
    d = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(crop))), 'm2')
    os.makedirs(d, exist_ok=True)
    return d

def mixed_rows(c, base, png, t):
    """Mixed/MRC-lite rows for #38: bitonalpdf.sh MODE=images' own JPEG setting (1-2), the real 1-bit text-mode
    pipeline (3), and two mask-based composites built on top of it (4-5). Same crop c, same tmpdir t as the CODECS loop."""
    w, h = map(int, run('magick', 'identify', '-format', '%wx%h', c).stdout.split('x'))
    out_dir = save_dir(c)

    for q, keep in ((65, True), (75, False)):  # MODE=images uses q65; q75 is here only for comparison with the codec table above
        jpg = os.path.join(t, f'j{q}.jpg')
        t0 = time.time(); run('magick', c, '-resize', '50%', '-quality', str(q), '-sampling-factor', '4:2:0', jpg); dt = time.time() - t0
        d = os.path.join(t, f'jd{q}.png')
        run('magick', jpg, '-resize', f'{w}x{h}!', d)
        if keep:
            shutil.copy(d, os.path.join(out_dir, f'{base}-1.png'))
        b = os.path.getsize(jpg)
        print(f'{base}\tJPEG q{q} 150 dpi 4:2:0\t{b}\t{b / png:.3f}\t{dt:.2f}\t{metric(c, d, "PSNR"):.2f}\t{metric(c, d, "SSIM"):.4f}')

    # 3: the real pipeline. img2pdf embeds the PNG losslessly; bitonalpdf re-encodes it to 1-bit CCITT G4.
    in_pdf, out_pdf = os.path.join(t, 'in.pdf'), os.path.join(t, 'out.pdf')
    t0 = time.time()
    run('img2pdf', '--imgsize', '300dpi', '-o', in_pdf, c)
    run(BITONALPDF, in_pdf, out_pdf)
    dt = time.time() - t0
    g4_bytes = g4_png = None
    if os.path.exists(out_pdf):
        run('pdfimages', '-ccitt', out_pdf, os.path.join(t, 'raw'))
        ccitt = glob.glob(os.path.join(t, 'raw-*.ccitt'))
        run('pdfimages', '-png', out_pdf, os.path.join(t, 'p'))
        pngs = glob.glob(os.path.join(t, 'p-*.png'))
        if ccitt and pngs:
            g4_bytes, g4_png = os.path.getsize(ccitt[0]), pngs[0]
            pw, ph = map(int, run('magick', 'identify', '-format', '%wx%h', g4_png).stdout.split('x'))
            if (pw, ph) != (w, h):
                fixed = os.path.join(t, 'p-fixed.png')
                run('magick', g4_png, '-filter', 'point', '-resize', f'{w}x{h}!', fixed)
                print(f'# {base}: G4 decode {pw}x{ph} != crop {w}x{h}, resized with -filter point', file=sys.stderr)
                g4_png = fixed
    if g4_bytes is None:
        print(f'{base}\t1-bit text mode (G4)\tFAILED\t\t\t\tbitonalpdf produced no usable output'); return
    shutil.copy(g4_png, os.path.join(out_dir, f'{base}-3.png'))
    print(f'{base}\t1-bit text mode (G4)\t{g4_bytes}\t{g4_bytes / png:.3f}\t{dt:.2f}\t{metric(c, g4_png, "PSNR"):.2f}\t{metric(c, g4_png, "SSIM"):.4f}')

    # 4-5 paint the G4 bitmap (black = ink) in a single colour: the per-channel median of the crop's own ink pixels.
    crop_arr = np.array(Image.open(c).convert('RGB'))
    ink = np.array(Image.open(g4_png).convert('L')) < 128
    ink_colour = tuple(int(np.median(crop_arr[..., ch][ink])) for ch in range(3)) if ink.any() else (0, 0, 0)

    t0 = time.time()
    comp4 = np.full_like(crop_arr, 255); comp4[ink] = ink_colour
    comp4_path = os.path.join(t, 'comp4.png'); Image.fromarray(comp4).save(comp4_path)
    dt = time.time() - t0
    shutil.copy(comp4_path, os.path.join(out_dir, f'{base}-4.png'))
    b4 = g4_bytes + 3  # 3 bytes for the one RGB ink colour
    print(f'{base}\tmask + 1 colour\t{b4}\t{b4 / png:.3f}\t{dt:.2f}\t{metric(c, comp4_path, "PSNR"):.2f}\t{metric(c, comp4_path, "SSIM"):.4f}')

    # ponytail: one colour for the whole crop and no inpainting under the mask is a simplification of real MRC, not a design choice
    t0 = time.time()
    bg_jpg = os.path.join(t, 'bg.jpg')
    run('magick', c, '-resize', '25%', '-quality', '50', '-sampling-factor', '4:2:0', bg_jpg)
    bg_up = os.path.join(t, 'bg_up.png'); run('magick', bg_jpg, '-resize', f'{w}x{h}!', bg_up)
    comp5 = np.array(Image.open(bg_up).convert('RGB')); comp5[ink] = ink_colour
    comp5_path = os.path.join(t, 'comp5.png'); Image.fromarray(comp5).save(comp5_path)
    dt = time.time() - t0
    shutil.copy(comp5_path, os.path.join(out_dir, f'{base}-5.png'))
    b5 = g4_bytes + os.path.getsize(bg_jpg)
    label = 'MRC-lite: mask + 1 colour over 75-dpi JPEG q50 background'
    print(f'{base}\t{label}\t{b5}\t{b5 / png:.3f}\t{dt:.2f}\t{metric(c, comp5_path, "PSNR"):.2f}\t{metric(c, comp5_path, "SSIM"):.4f}')

def viewers(crop):
    """Embed the crop with each PDF-native filter (img2pdf, no re-encoding) and render it in every viewer found."""
    print('filter\tviewer\tresult\tpsnr_vs_that_viewers_lossless_render_db')
    with tempfile.TemporaryDirectory() as t:
        variants = {}
        run('magick', crop, '-define', 'png:compression-level=9', '-density', '300', f'{t}/b.png'); variants['FlateDecode (PNG, predictor)'] = f'{t}/b.png'
        run('magick', crop, '-quality', '75', '-sampling-factor', '4:2:0', '-density', '300', f'{t}/a.jpg'); variants['DCTDecode (JPEG q75)'] = f'{t}/a.jpg'
        run('magick', crop, '-colors', '256', '-density', '300', f'PNG8:{t}/c.png'); variants['FlateDecode /Indexed (PNG-8)'] = f'{t}/c.png'
        run('magick', crop, '-alpha', 'off', f'{t}/x.ppm'); run('opj_compress', '-i', f'{t}/x.ppm', '-o', f'{t}/d.jp2', '-r', '10'); variants['JPXDecode (JPEG 2000 ratio 10)'] = f'{t}/d.jp2'
        pdfs = {}
        for k, f in variants.items():
            out = f + '.pdf'
            run('img2pdf', '--pillow-limit-break', '--imgsize', '300dpi', '-o', out, f)
            pdfs[k] = out
        ref = {}  # per viewer: its render of the lossless Flate variant; every other filter is compared against that,
        # because a viewer's own resampling/page geometry would otherwise swamp the codec loss in the PSNR
        for k, pdf in pdfs.items():
            for viewer in ('pdftoppm', 'gs', 'qlmanage'):
                out = f'{t}/r-{viewer}.png'
                if os.path.exists(out): os.remove(out)
                if viewer == 'pdftoppm':
                    if shutil.which('pdftoppm'): run('pdftoppm', '-r', '300', '-png', '-singlefile', pdf, out[:-4])
                elif viewer == 'gs':
                    if shutil.which('gs'): run('gs', '-q', '-dNOPAUSE', '-dBATCH', '-sDEVICE=png16m', '-r300', f'-sOutputFile={out}', pdf)
                elif shutil.which('qlmanage'):
                    w = run('magick', 'identify', '-format', '%w', crop).stdout
                    run('qlmanage', '-t', '-s', w, '-o', t, pdf); q = os.path.join(t, os.path.basename(pdf) + '.png')
                    if os.path.exists(q): os.replace(q, out)
                if not os.path.exists(out):
                    print(f'{k}\t{viewer}\tno output\t'); continue
                std = run('magick', out, '-format', '%[fx:standard_deviation]', 'info:').stdout
                keep = f'{t}/k-{viewer}-{abs(hash(k))}.png'; shutil.copy(out, keep)
                if k.startswith('FlateDecode (PNG'):
                    ref[viewer] = keep
                r = ref.get(viewer)
                sz = run('magick', 'identify', '-format', '%wx%h', keep).stdout
                p = f'{metric(r, keep, "PSNR"):.2f}' if r else ''
                print(f'{k}\t{viewer}\trendered {sz}, pixel std {float(std):.3f}\t{p}')

def _pdf_dict(num, body):
    return f'{num} 0 obj\n{body}\nendobj\n'.encode()

def _pdf_stream(num, extra, data):
    d = ' '.join(f'/{k} {v}' for k, v in extra.items())
    return f'{num} 0 obj\n<< {d} /Length {len(data)} >>\nstream\n'.encode() + data + b'\nendstream\nendobj\n'

def _write_pdf(path, objects):
    """objects[i] is the complete 'N 0 obj ... endobj' block for object i+1; writes header + objects + xref + trailer."""
    out, offsets, pos = [b'%PDF-1.3\n'], [0], len(b'%PDF-1.3\n')
    for obj in objects:
        offsets.append(pos); out.append(obj); pos += len(obj)
    xref_pos = pos; n = len(objects) + 1
    xref = [f'xref\n0 {n}\n'.encode(), b'0000000000 65535 f \n'] + [f'{off:010d} 00000 n \n'.encode() for off in offsets[1:]]
    trailer = f'trailer\n<< /Size {n} /Root 1 0 R >>\nstartxref\n{xref_pos}\n%%EOF'.encode()
    with open(path, 'wb') as f:
        f.write(b''.join(out) + b''.join(xref) + trailer)

def _bars_mask(w, h, bars):
    """1bpp Flate stream for an /ImageMask: bars (row0,row1,col0,col1 rects) get bit 0 (painted), everything else bit 1
    (transparent) -- the default Decode [0 1] convention, verified below against pdftoppm/gs/qlmanage renders."""
    opaque = np.zeros((h, w), dtype=bool)
    for r0, r1, c0, c1 in bars:
        opaque[r0:r1, c0:c1] = True
    return zlib.compress(np.packbits(~opaque, axis=1).tobytes(), 9)

# page is 600x600 px at 300 dpi = 144x144 pt; region = the middle 300x300 px = 36..108 pt
BARS = [(280, 300, 0, 600), (50, 70, 0, 600)]  # one crossing the region (rows 150-449), one entirely above it

def _build_mrc_pdf(path, region_jpg, orange_jpg, mask):
    objs = [
        _pdf_dict(1, '<< /Type /Catalog /Pages 2 0 R >>'),
        _pdf_dict(2, '<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>'),
        _pdf_dict(3, '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 144 144] '
                      '/Resources << /XObject << /Im1 5 0 R /Mk1 6 0 R >> >> /Contents 8 0 R >>'),
        _pdf_dict(4, '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 144 144] '
                      '/Resources << /XObject << /Im2 7 0 R >> >> /Contents 9 0 R >>'),
        _pdf_stream(5, {'Type': '/XObject', 'Subtype': '/Image', 'Width': 300, 'Height': 300,
                         'ColorSpace': '/DeviceRGB', 'BitsPerComponent': 8, 'Filter': '/DCTDecode'}, region_jpg),
        _pdf_stream(6, {'Type': '/XObject', 'Subtype': '/Image', 'Width': 600, 'Height': 600,
                         'ImageMask': 'true', 'BitsPerComponent': 1, 'Filter': '/FlateDecode'}, mask),
        _pdf_stream(7, {'Type': '/XObject', 'Subtype': '/Image', 'Width': 150, 'Height': 150, 'ColorSpace': '/DeviceRGB',
                         'BitsPerComponent': 8, 'Filter': '/DCTDecode', 'Mask': '6 0 R'}, orange_jpg),
        _pdf_stream(8, {'Filter': '/FlateDecode'}, zlib.compress(b'q\n72 0 0 72 36 36 cm\n/Im1 Do\nQ\nq\n0 g\n144 0 0 144 0 0 cm\n/Mk1 Do\nQ\n', 9)),
        _pdf_stream(9, {'Filter': '/FlateDecode'}, zlib.compress(b'q\n144 0 0 144 0 0 cm\n/Im2 Do\nQ\n', 9)),
    ]
    _write_pdf(path, objs)

def _render(pdf, page, viewer, out, t):
    if viewer == 'pdftoppm':
        if shutil.which('pdftoppm'): run('pdftoppm', '-f', str(page), '-l', str(page), '-r', '300', '-png', '-singlefile', pdf, out[:-4])
    elif viewer == 'gs':
        if shutil.which('gs'): run('gs', '-q', '-dNOPAUSE', '-dBATCH', '-sDEVICE=png16m', '-r300',
                                    f'-dFirstPage={page}', f'-dLastPage={page}', f'-sOutputFile={out}', pdf)
    elif shutil.which('qlmanage') and shutil.which('gs'):
        one = os.path.join(t, f'p{page}.pdf')  # qlmanage only thumbnails page 1, so cut the page out first
        run('gs', '-q', '-dNOPAUSE', '-dBATCH', '-sDEVICE=pdfwrite', f'-dFirstPage={page}', f'-dLastPage={page}', f'-sOutputFile={one}', pdf)
        run('qlmanage', '-t', '-s', '600', '-o', t, one)
        q = os.path.join(t, os.path.basename(one) + '.png')
        if os.path.exists(q): os.replace(q, out)

def _px(im, row, col):  # (row, col) in the 600x600 design grid; scaled to whatever size the viewer actually rendered
    x = min(int(col * im.width / 600), im.width - 1); y = min(int(row * im.height / 600), im.height - 1)
    return im.getpixel((x, y))[:3]

def mrc_viewers():
    """Synthetic-only (no scan content) test of mixed content in one PDF: page 1 draws a JPEG region then an /ImageMask
    with bars over it (default Decode [0 1]: sample 0 = painted, per spec 8.9.6.3, checked against the renders below);
    page 2 is a small JPEG with a stencil /Mask at 4x that image's own resolution (mask and base image may differ)."""
    t = tempfile.mkdtemp()
    region_jpg_path, orange_jpg_path = os.path.join(t, 'region.jpg'), os.path.join(t, 'orange.jpg')
    run('magick', '-size', '150x300', 'xc:rgb(220,30,30)', '-size', '150x300', 'xc:rgb(30,60,200)', '+append',
        '-quality', '95', '-sampling-factor', '4:4:4', region_jpg_path)
    run('magick', '-size', '150x150', 'xc:rgb(230,120,20)', '-quality', '95', orange_jpg_path)
    mask = _bars_mask(600, 600, BARS)
    pdf = os.path.join(t, 'mrc.pdf')
    _build_mrc_pdf(pdf, open(region_jpg_path, 'rb').read(), open(orange_jpg_path, 'rb').read(), mask)
    print(f'synthetic PDF kept at {pdf}')

    checks = {
        1: [('bar over region', 290, 250, (0, 0, 0)), ('region between bars, red half', 400, 200, (220, 30, 30)),
            ('region between bars, blue half', 400, 400, (30, 60, 200)), ('white outside', 550, 550, (255, 255, 255)),
            ('bar outside region', 60, 300, (0, 0, 0))],
        2: [('bar', 290, 300, (230, 120, 20)), ('off-bar', 550, 550, (255, 255, 255))],
    }
    print('page\tviewer\tpoint\texpected\tgot\tresult')
    for page, points in checks.items():
        for viewer in ('pdftoppm', 'gs', 'qlmanage'):
            out = os.path.join(t, f'r{page}-{viewer}.png')
            _render(pdf, page, viewer, out, t)
            if not os.path.exists(out):
                print(f'{page}\t{viewer}\t-\t-\t-\tno output (tool missing?)'); continue
            im = Image.open(out).convert('RGB')
            for name, row, col, exp in points:
                got = _px(im, row, col)
                ok = all(abs(g - e) <= 30 for g, e in zip(got, exp))
                print(f'{page}\t{viewer}\t{name}\t{exp}\t{got}\t{"ok" if ok else "FAIL"}')
    print('rust/target/release/bitonalpdf has no render-to-PNG CLI; skipped (not adding one per the task).')

if __name__ == '__main__':
    if sys.argv[1] == '--viewers':
        viewers(sys.argv[2])
    elif sys.argv[1] == '--mrc-viewers':
        mrc_viewers()
    else:
        table(sys.argv[1:])
