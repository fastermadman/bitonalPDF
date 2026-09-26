#!/usr/bin/env python3
"""Codec table for #43 / #38 question 4: bytes, PSNR and SSIM of one crop per codec/quality setting.

Usage: tests/codec-table.py crop.png [crop2.png ...]   (300-dpi PNG crops of the content types; prints a TSV)
       tests/codec-table.py --viewers crop.png        (embeds the crop with each PDF filter, renders it in every viewer found)
Needs: magick, cwebp/dwebp, opj_compress/opj_decompress, avifenc, img2pdf; viewers: pdftoppm, gs, qlmanage (macOS PDFKit).
The crops are not committed (they come from copyrighted scans); see docs/rust-port.md section 4 for how they were cut.
"""
import os, subprocess, sys, tempfile, time, shutil

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

if __name__ == '__main__':
    if sys.argv[1] == '--viewers':
        viewers(sys.argv[2])
    else:
        table(sys.argv[1:])
