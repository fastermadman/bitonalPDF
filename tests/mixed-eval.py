#!/usr/bin/env python3
"""Detection at a safe operating point, error cost and size of a mixed output (#38, docs/rust-port.md section 5).

Usage: tests/mixed-eval.py det.tsv <folder with X.pdf, X.labels> geom.txt slots.txt <text dir> <images dir> [--sizes]
  det.tsv    `bitonalpdf --detect-eval` output (section 4.5)
  geom.txt   pass-A lines of `BITONAL_TIMING=1 bitonalpdf --crop --split auto fN.pdf` ("fN P: ... box Some([..]) ... gutter ..")
  slots.txt  pass-B lines of the same run ("fN P: ... deskew [a, b]": one angle per output slot)
  text dir / images dir: fN.texttess.pdf / fN.images.pdf (same output page count as the run above)
  --sizes    also render every page at 150 dpi and JPEG (q65 4:2:0, MODE=images' setting) the labelled and the detected
             boxes; renders and JPEGs go to <folder>/work/m3/ (git-ignored), only numbers are printed.
fN = the PDFs in alphabetical order. Classes picture/diagram/coloured-text/cover are pooled into `keep` (section 5.1).
Cuts are chosen recall-first on six files and applied to the seventh (leave-one-file-out); in-sample cuts are also shown.
"""
import sys, os, re, subprocess, collections, unicodedata

det, folder, geomf, slotf, tdir, idir = sys.argv[1:7]
SIZES = '--sizes' in sys.argv
KEEP = {'picture', 'diagram', 'coloured-text', 'cover'}
TILE = 320
names = sorted(f for f in os.listdir(folder) if f.endswith('.pdf'))
fid = {unicodedata.normalize('NFC', n): f'f{i + 1}' for i, n in enumerate(names)}  # macOS file names are NFD

def labels(name):
    L = {}
    for line in open(os.path.join(folder, name[:-4] + '.labels'), encoding='utf-8'):
        if line.startswith('#') or not line.strip():
            continue
        p, cls, cert, regs, *_ = line.rstrip('\n').split('\t') + ['']
        R = [(r.split(':')[0], *map(float, r.split(':')[1].rstrip('?').split(','))) for r in filter(None, regs.split(';'))]
        L[int(p)] = (set(cls.split(',')), cert == '1', [r for r in R if r[0] in KEEP])
    return L
LAB = {fid[unicodedata.normalize('NFC', n)]: labels(n) for n in names}

G = {}  # (f, page) -> crop box px, gutter x or None
for l in open(geomf):
    m = re.match(r'(f\d) (\d+):.*box Some\(\[(\d+), (\d+), (\d+), (\d+)\]\).*gutter (Some\(\((\d+),|None)', l)
    G[(m[1], int(m[2]))] = ([int(m[i]) for i in range(3, 7)], int(m[8]) if m[8] else None)
NSLOT = {(m[1], int(m[2])): m[3].count(',') + 1 for m in (re.match(r'(f\d) (\d+):.*deskew \[(.*)\]', l) for l in open(slotf))}

pages, tiles = {}, collections.defaultdict(list)  # page signals; tiles per page: (x0,y0,x1,y1,hasler,entropy)
for line in open(det, encoding='utf-8'):
    if line.startswith('#'):
        continue
    f = line.rstrip('\n').split('\t')
    k = (fid[unicodedata.normalize('NFC', f[0])], int(f[1]))
    x0, y0, x1, y1, W, H = map(int, f[3:9])
    s = list(map(float, f[9:]))
    if f[2] == 'page':
        pages[k] = (W, H, s[0], s[0] > 0)  # hasler; RGB source (hasler > 0, as detect-join.py)
    else:
        tiles[k].append((x0, y0, x1, y1, s[0], s[5]))

def ov(a, b):
    return max(0, min(a[2], b[2]) - max(a[0], b[0])) * max(0, min(a[3], b[3]) - max(a[1], b[1]))

def px(k, r):
    W, H = pages[k][:2]
    return (r[1] * W, r[2] * H, r[3] * W, r[4] * H)

def tile_class(k, t):
    """keep if >= 75 % inside one keep region, rest if no region touches it, None (dropped) otherwise (as detect-join.py)."""
    a = (t[2] - t[0]) * (t[3] - t[1])
    o = [ov(t, px(k, r)) for r in LAB[k[0]][k[1]][2]]
    return 'keep' if any(v >= 0.75 * a for v in o) else (None if any(o) else 'rest')

def in_crop(k, t):
    b = G[k][0]
    cx, cy = (t[0] + t[2]) / 2, (t[1] + t[3]) / 2
    return b[0] <= cx < b[2] and b[1] <= cy < b[3]

files = sorted({k[0] for k in pages})
def cut_at(vals, recall):
    """Largest cut c with share(v >= c) >= recall."""
    v = sorted(vals)
    return v[min(len(v) - 1, int((1 - recall) * len(v)))] if v else float('inf')

def fmt(n, d):
    return f'{n}/{d}' if d else '-'

# ---------- 1. page level: page has a keep class vs text-only page ----------
print('## Page level: flag the page when page hasler >= cut (cut = lowest keep page in the training files)\n')
print('| subset | in-sample: keep caught / FP text pages | leave-one-file-out: keep caught / FP text pages | held-out misses |')
print('|---|---:|---:|---|')
for sub, ok in (('all pages', lambda k: True), ('RGB-source pages', lambda k: pages[k][3]),
                ('certain labels only', lambda k: LAB[k[0]][k[1]][1])):
    P = {k: bool(LAB[k[0]][k[1]][0] & KEEP) for k in pages if ok(k)}
    ins = min(pages[k][2] for k in P if P[k])
    tp = sum(1 for k in P if P[k] and pages[k][2] >= ins); fp = sum(1 for k in P if not P[k] and pages[k][2] >= ins)
    lt = lf = 0; miss = []
    for f in files:
        c = min((pages[k][2] for k in P if P[k] and k[0] != f), default=float('inf'))
        for k in P:
            if k[0] == f:
                hit = pages[k][2] >= c
                lt += P[k] and hit; lf += (not P[k]) and hit
                if P[k] and not hit:
                    miss.append(f'{k[0]} p{k[1]}')
    npos, nneg = sum(P.values()), sum(1 for v in P.values() if not v)
    print(f'| {sub} | {fmt(tp, npos)} / {fmt(fp, nneg)} | {fmt(lt, npos)} / {fmt(lf, nneg)} | {", ".join(miss) or "none"} |')

# ---------- 2. tile level ----------
def flags(k, cuts, crop_only):
    ch, ce = cuts
    return [t for t in tiles[k] if (not crop_only or in_crop(k, t)) and (t[4] >= ch or t[5] >= ce)]

def boxes(k, fl):
    """Flagged tiles -> 8-connected groups on the tile grid -> bounding box + half a tile, clipped to the page."""
    W, H = pages[k][:2]
    cells = {(t[0] // TILE, t[1] // TILE): t for t in fl}
    seen, out = set(), []
    for c in cells:
        if c in seen:
            continue
        st, grp = [c], []
        seen.add(c)
        while st:
            a = st.pop(); grp.append(cells[a])
            for dx in (-1, 0, 1):
                for dy in (-1, 0, 1):
                    n = (a[0] + dx, a[1] + dy)
                    if n in cells and n not in seen:
                        seen.add(n); st.append(n)
        m = TILE // 2
        out.append((max(0, min(t[0] for t in grp) - m), max(0, min(t[1] for t in grp) - m),
                    min(W, max(t[2] for t in grp) + m), min(H, max(t[3] for t in grp) + m)))
    return out

def coverage(k, bx, r, step=10):
    """Share of region r (px) covered by the union of boxes, on a step-px grid."""
    x0, y0, x1, y1 = r
    n = hit = 0
    for y in range(int(y0) + step // 2, int(y1), step):
        for x in range(int(x0) + step // 2, int(x1), step):
            n += 1; hit += any(b[0] <= x < b[2] and b[1] <= y < b[3] for b in bx)
    return hit / n if n else 1.0

def train_cuts(train, recall, sig, crop_only):
    pos = [t for k in train for t in tiles[k] if (not crop_only or in_crop(k, t)) and tile_class(k, t) == 'keep']
    ch = cut_at([t[4] for t in pos], recall) if 'hasler' in sig else float('inf')
    ce = cut_at([t[5] for t in pos], recall) if 'entropy' in sig else float('inf')
    return ch, ce

REGS = [(k, r) for k in pages for r in LAB[k[0]][k[1]][2]]
print(f'\n{len(REGS)} labelled keep regions; area outside the pass-A crop box: '
      + ', '.join(f'{k[0]} p{k[1]} {r[0]} {1 - ov(px(k, r), G[k][0]) / ov(px(k, r), px(k, r)):.0%}'
                  for k, r in REGS if ov(px(k, r), G[k][0]) < 0.9 * ov(px(k, r), px(k, r))) or 'none > 10 %')

DET = {}  # (setting) -> page -> boxes (leave-one-file-out)
print('\n## Tiles: leave-one-file-out, flag tile when signal >= cut (cut = recall R on the keep tiles of the other six files)\n')
print('| signal | tiles | R | keep tiles caught | FP rest tiles | FP rate | regions < 90 %% covered (of %d) | regions < 50 %% | pages with any FP box (of %d text-only) |'
      % (len(REGS), sum(1 for k in pages if not LAB[k[0]][k[1]][0] & KEEP)))
print('|---|---|---:|---:|---:|---:|---:|---:|---:|')
# Tile recall is not the goal: keep boxes contain colourless tiles (white paper inside a diagram or cover box), and the
# bounding box of a tile group fills such holes. The operating point is chosen by region coverage (below), not by R.
for sig in ('hasler', 'hasler|entropy'):
    for crop_only in (False, True):
        for R in ((0.5, 0.7, 0.8, 0.9, 0.95, 0.99) if sig == 'hasler' and crop_only else (0.8, 0.95)):
            tp = npos = fp = nneg = 0
            per = {}
            for f in files:
                cuts = train_cuts([k for k in pages if k[0] != f], R, sig, crop_only)
                for k in pages:
                    if k[0] != f:
                        continue
                    fl = flags(k, cuts, crop_only)
                    fs = {id(t) for t in fl}
                    for t in tiles[k]:
                        if crop_only and not in_crop(k, t):
                            continue
                        c = tile_class(k, t)
                        if c == 'keep':
                            npos += 1; tp += id(t) in fs
                        elif c == 'rest':
                            nneg += 1; fp += id(t) in fs
                    per[k] = boxes(k, fl)
            DET[(sig, crop_only, R)] = per
            cov = [coverage(k, per[k], px(k, r)) for k, r in REGS]
            txt = [k for k in pages if not LAB[k[0]][k[1]][0] & KEEP and per[k]]
            print(f'| {sig} | {"in crop box" if crop_only else "all"} | {R} | {fmt(tp, npos)} | {fp} | {fp / nneg:.3f} | '
                  f'{sum(c < 0.9 for c in cov)} | {sum(c < 0.5 for c in cov)} | {len(txt)} |')

def missed(setting):
    per = DET[setting]
    return [(k, r[0], coverage(k, per[k], px(k, r)), LAB[k[0]][k[1]][1]) for k, r in REGS if coverage(k, per[k], px(k, r)) < 0.9]

# Two operating points for the size tables: safe (no region < 50 % covered) and balanced (FP rate ~0.1).
SAFE, BAL = ('hasler', True, 0.99), ('hasler', True, 0.8)
SETTING = BAL
FILES_FP = collections.Counter()
for k in pages:
    for b in DET[SETTING][k]:
        FILES_FP[k[0]] += 1 - coverage(k, [px(k, r) for r in LAB[k[0]][k[1]][2]], b, 20)
print(f'\nDetected boxes outside labelled regions (in box units), {SETTING}: ' + ', '.join(f'{f} {v:.1f}' for f, v in sorted(FILES_FP.items())))
for s in (SAFE, BAL):
    print(f'\nRegions < 90 % covered, {s[0]}, in crop box, R {s[2]}: '
          + '; '.join(f'{k[0]} p{k[1]} {c} {v:.0%}{"" if cert else " (uncertain page)"}' for k, c, v, cert in missed(s)))

# ---------- 3. sizes ----------
def img_sizes(pdf):
    """Bytes of each page's image stream(s), from pdfimages -list (rounded to 0.1 K)."""
    out = collections.Counter()
    for l in subprocess.run(['pdfimages', '-list', pdf], capture_output=True, text=True).stdout.splitlines()[2:]:
        c = l.split()
        v = c[14]
        out[int(c[0])] += float(v[:-1]) * {'B': 1, 'K': 1e3, 'M': 1e6}[v[-1]] if v[-1] in 'BKM' else float(v)
    return out

def slots(f):
    """Output page -> (input page, box in px of the input page) for one file."""
    out, n = {}, 0
    for p in sorted(p for (g, p) in NSLOT if g == f):
        W, H = pages[(f, p)][:2]
        gx = G[(f, p)][1] or W // 2  # ponytail: no trusted gutter -> middle; the real plan uses the document median
        sl = [(0, 0, W, H)] if NSLOT[(f, p)] == 1 else [(0, 0, gx, H), (gx, 0, W, H)]
        for s in sl:
            n += 1; out[n] = ((f, p), s)
    return out

print('\n## Size, page level: each output page is either the text-mode page (G4) or the images-mode page (JPEG)\n')
print('| file | text | images | labels, per output page | labels, whole input page | detected, safe (R 0.99) | detected, balanced (R 0.8) | colour output pages labels / input page / safe / balanced / all |')
print('|---|---:|---:|---:|---:|---:|---:|---:|')
tot = collections.Counter()
for f in files:
    ts, is_ = img_sizes(f'{tdir}/{f}.texttess.pdf'), img_sizes(f'{idir}/{f}.images.pdf')
    sl = slots(f)
    assert len(sl) == len(ts) == len(is_), (f, len(sl), len(ts), len(is_))
    b = collections.Counter()
    for o, (k, s) in sl.items():
        cl, _, regs = LAB[f][k[1]]
        for key, hit in (('l', any(ov(px(k, r), s) > 0 for r in regs) or (bool(cl & KEEP) and not regs)), ('p', bool(cl & KEEP)),
                         ('s', any(ov(x, s) > 0 for x in DET[SAFE][k])), ('b', any(ov(x, s) > 0 for x in DET[BAL][k]))):
            b[key] += is_[o] if hit else ts[o]; b['n' + key] += hit
    b['t'], b['i'], b['n'] = sum(ts.values()), sum(is_.values()), len(sl)
    tot.update(b)
    print(f'| {f} | ' + ' | '.join(f'{b[c] / 1e6:.2f} MB' for c in 'tilpsb') + f' | {b["nl"]} / {b["np"]} / {b["ns"]} / {b["nb"]} / {len(sl)} |')
print(f'| sum | ' + ' | '.join(f'{tot[c] / 1e6:.2f} MB ({tot[c] / tot["t"]:.2f}x)' for c in 'tilpsb')
      + f' | {tot["nl"]} / {tot["np"]} / {tot["ns"]} / {tot["nb"]} / {tot["n"]} |')

if SIZES:
    work = os.path.join(folder, 'work', 'm3')
    os.makedirs(f'{work}/r150', exist_ok=True)
    def jpeg_bytes(k, b):
        png = f'{work}/r150/{k[0]}-{k[1]}.png'
        if not os.path.exists(png):
            subprocess.run(['pdftoppm', '-r', '150', '-f', str(k[1]), '-l', str(k[1]), '-png', '-singlefile',
                            f'{folder}/{names[int(k[0][1:]) - 1]}', png[:-4]], check=True)
        x0, y0, x1, y1 = (int(v / 2) for v in b)
        if x1 - x0 < 2 or y1 - y0 < 2:
            return 0
        j = f'{work}/tmp.jpg'
        subprocess.run(['magick', png, '-crop', f'{x1 - x0}x{y1 - y0}+{x0}+{y0}', '+repage', '-quality', '65',
                        '-sampling-factor', '4:2:0', j], check=True)
        return os.path.getsize(j)
    print('\n## Size, region level: text-mode file + JPEG q65 150 dpi of every keep box (G4 not reduced under the boxes)\n')
    print('| file (MB) | text | region JPEG, labels | detected safe | of which FP | detected balanced | of which FP |')
    print('|---|---:|---:|---:|---:|---:|---:|')
    S = collections.Counter()
    for f in files:
        t = sum(img_sizes(f'{tdir}/{f}.texttess.pdf').values())
        fk = [k for k in pages if k[0] == f]
        # pages with a keep class but no box (f7 p7: inline colour only) count as a whole-page box
        lb = sum(jpeg_bytes(k, px(k, r)) for k in fk for r in LAB[f][k[1]][2]) + \
             sum(jpeg_bytes(k, (0, 0, *pages[k][:2])) for k in fk if LAB[f][k[1]][0] & KEEP and not LAB[f][k[1]][2])
        row = {'t': t, 'l': lb}
        for key, st in (('s', SAFE), ('b', BAL)):
            for k in fk:
                for b in DET[st][k]:
                    j = jpeg_bytes(k, b)
                    row[key] = row.get(key, 0) + j
                    row['fp' + key] = row.get('fp' + key, 0) + j * (1 - coverage(k, [px(k, r) for r in LAB[f][k[1]][2]], b, 20))
        S.update(row)
        print(f'| {f} | ' + ' | '.join(f'{row.get(c, 0) / 1e6:.2f}' for c in ('t', 'l', 's', 'fps', 'b', 'fpb')) + ' |')
    print('| sum | ' + ' | '.join(f'{S[c] / 1e6:.2f}' for c in ('t', 'l', 's', 'fps', 'b', 'fpb')) + ' |')
    print('\nmixed = text + region JPEG: labels {:.2f} MB ({:.2f}x text), safe {:.2f} MB ({:.2f}x), balanced {:.2f} MB ({:.2f}x)'.format(
        *[v for c in 'lsb' for v in ((S['t'] + S[c]) / 1e6, (S['t'] + S[c]) / S['t'])]))
