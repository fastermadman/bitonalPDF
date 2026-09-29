#!/usr/bin/env python3
"""Number of ink colours and flat-vs-continuous colour per page and per labelled box (#74, docs/rust-port.md section 5.9 item 2).

Usage: tests/ink-colours.py det.tsv <folder with X.pdf, X.labels> geom.txt
  det.tsv   `bitonalpdf --detect-eval` output (page size at 300 dpi, RGB source = page hasler > 0)
  geom.txt  pass-A lines as for tests/mixed-eval.py (the crop box; page signals are taken inside it, 4.5's surround problem)
Renders every page at 150 dpi to <folder>/work/m3/r150/ (git-ignored, same files as mixed-eval.py --sizes); prints numbers only.

Question (a): can a signal tell pages whose only non-text content is coloured text (mask + colours would do) from pages with a
picture, diagram or cover (mask + colours turns them into silhouettes)? Pages = input pages with an RGB source. Cuts are chosen
on six files and applied to the seventh (leave-one-file-out), each signal alone, direction chosen on the six files.
The constants below are measurement settings, not rules.
"""
import sys, os, re, subprocess, unicodedata, collections
import numpy as np
from PIL import Image

CHROMA = 60        # colour pixel: max(R,G,B) - min(R,G,B) >= 60 of 255 (nongrey60 of 4.5)
TINT = (20, 60)    # tinted paper: light pixel (luma >= 0.7) with chroma in [20, 60)
BINS = 36          # 10 degree hue bins
CLUSTER = 0.05     # a hue cluster = circular run of bins with >= 2 % of the colour mass each, counted when it holds >= 5 %
KMEANS = 4         # colours for the flat-colour fit
KEEP = {'picture', 'diagram', 'coloured-text', 'cover'}

det, folder, geomf = sys.argv[1:4]
names = sorted(f for f in os.listdir(folder) if f.endswith('.pdf'))
fid = {unicodedata.normalize('NFC', n): f'f{i + 1}' for i, n in enumerate(names)}
LAB = {}
for n in names:
    for line in open(os.path.join(folder, n[:-4] + '.labels'), encoding='utf-8'):
        if line.startswith('#') or not line.strip():
            continue
        p, cls, cert, regs, *_ = line.rstrip('\n').split('\t') + ['']
        R = [(r.split(':')[0], *map(float, r.split(':')[1].rstrip('?').split(','))) for r in filter(None, regs.split(';'))]
        LAB[(fid[unicodedata.normalize('NFC', n)], int(p))] = (set(cls.split(',')), cert == '1', R)
PG = {}
for line in open(det, encoding='utf-8'):
    f = line.rstrip('\n').split('\t')
    if not line.startswith('#') and f[2] == 'page':
        PG[(fid[unicodedata.normalize('NFC', f[0])], int(f[1]))] = (int(f[7]), int(f[8]), float(f[9]) > 0)
BOX = {}
for l in open(geomf):
    m = re.match(r'(f\d) (\d+):.*box Some\(\[(\d+), (\d+), (\d+), (\d+)\]\)', l)
    BOX[(m[1], int(m[2]))] = [int(m[i]) for i in range(3, 7)]

work = os.path.join(folder, 'work', 'm3', 'r150')
os.makedirs(work, exist_ok=True)
def render(k):
    png = f'{work}/{k[0]}-{k[1]}.png'
    if not os.path.exists(png):
        subprocess.run(['pdftoppm', '-r', '150', '-f', str(k[1]), '-l', str(k[1]), '-png', '-singlefile',
                        f'{folder}/{names[int(k[0][1:]) - 1]}', png[:-4]], check=True)
    return np.asarray(Image.open(png).convert('RGB'))

def kmeans_residual(x, k):
    """Mean RGB distance (0-1) of the colour pixels to the nearest of k centres; k-means++ with a fixed seed, 20 rounds."""
    rng = np.random.default_rng(0)
    if len(x) > 20000:
        x = x[rng.choice(len(x), 20000, replace=False)]
    c = [x[rng.integers(len(x))]]
    for _ in range(k - 1):
        d = np.min([((x - ci) ** 2).sum(1) for ci in c], 0)
        c.append(x[rng.choice(len(x), p=d / d.sum())] if d.sum() > 0 else x[0])
    c = np.array(c)
    for _ in range(20):
        a = np.argmin(((x[:, None] - c[None]) ** 2).sum(2), 1)
        c = np.array([x[a == j].mean(0) if (a == j).any() else c[j] for j in range(k)])
    return float(np.sqrt(((x - c[a]) ** 2).sum(1)).mean() / 255)

def signals(img):
    """img: HxWx3 uint8 -> dict of signals (see the table printed below)."""
    x = img.reshape(-1, 3).astype(np.float32)
    mx, mn = x.max(1), x.min(1)
    ch = mx - mn
    luma = (0.299 * x[:, 0] + 0.587 * x[:, 1] + 0.114 * x[:, 2]) / 255
    col = ch >= CHROMA
    light = luma >= 0.7
    s = {'col_area': col.mean(), 'tint': ((ch >= TINT[0]) & (ch < TINT[1]) & light).sum() / max(1, light.sum())}
    c = x[col]
    if len(c) < 50:
        return s | {'k': 0, 'top2': 1.0, 'lstd': 0.0, 'fit4': 0.0, 'hues': ''}
    r, g, b = c[:, 0], c[:, 1], c[:, 2]
    d = ch[col]
    h = np.where(mx[col] == r, (g - b) / d % 6, np.where(mx[col] == g, (b - r) / d + 2, (r - g) / d + 4)) * 60
    hist = np.bincount((h // (360 / BINS)).astype(int) % BINS, minlength=BINS) / len(c)
    sm = (np.roll(hist, 1) + 2 * hist + np.roll(hist, -1)) / 4
    on = sm >= 0.02
    runs, cur = [], []
    start = int(np.argmin(on)) if not on.all() else 0  # start the circular scan on an empty bin
    for i in range(BINS):
        j = (start + i) % BINS
        if on[j]:
            cur.append(j)
        elif cur:
            runs.append(cur); cur = []
    if cur:
        runs.append(cur)
    big = [r for r in runs if hist[r].sum() >= CLUSTER]
    w = np.array([hist[[(i - 1) % BINS, i, (i + 1) % BINS]].sum() for i in range(BINS)])
    i1 = int(np.argmax(w)); w2 = w.copy()
    for dj in range(-2, 3):
        w2[(i1 + dj) % BINS] = 0
    return s | {'k': len(big), 'top2': float(w[i1] + w2.max()), 'lstd': float(luma[col].std()),
                'fit4': kmeans_residual(c, KMEANS),
                'hues': ' '.join(f'{int((r[int(np.argmax(hist[r]))] + 0.5) * 360 / BINS)}°' for r in big)}

SIG = ['k', 'top2', 'col_area', 'lstd', 'fit4', 'tint']
S, RS = {}, []  # page -> signals; (page, class, signals) per labelled box
for k in sorted(PG):
    if not PG[k][2]:
        continue
    img = render(k)
    H, W = img.shape[:2]
    sx, sy = W / PG[k][0], H / PG[k][1]
    b = BOX.get(k, [0, 0, PG[k][0], PG[k][1]])
    S[k] = signals(img[int(b[1] * sy):int(b[3] * sy), int(b[0] * sx):int(b[2] * sx)])
    for r in LAB[k][2]:
        if r[0] in KEEP:
            crop = img[int(r[2] * H):int(r[4] * H), int(r[1] * W):int(r[3] * W)]
            if crop.size:
                RS.append((k, r[0], signals(crop)))

def group(k):
    c = LAB[k][0] & KEEP
    return 'P' if c == {'coloured-text'} else 'N' if c else 'T'

def auc(pos, neg):
    if not pos or not neg:
        return float('nan')
    a = sum((p > n) + 0.5 * (p == n) for p in pos for n in neg) / (len(pos) * len(neg))
    return a

def lofo(items, sig):
    """items: (file, group P/N/T, value). Per held-out file: direction from the AUC on the six others; safe cut = the most
    P-like N value of the six (strictly beyond it passes); balanced cut = best balanced accuracy on the six."""
    out = collections.Counter()
    for f in sorted({i[0] for i in items}):
        tr = [i for i in items if i[0] != f]
        P = [v for _, g, v in tr if g == 'P']; N = [v for _, g, v in tr if g == 'N']
        if not P or not N:
            out['skipped'] += 1
            continue
        up = auc(P, N) >= 0.5  # P larger than N
        sg = 1 if up else -1
        safe = max(sg * v for v in N)
        cands = sorted({sg * v for v in P + N})
        bal = max(cands, key=lambda c: (sum(sg * v >= c for v in P) / len(P) + sum(sg * v < c for v in N) / len(N)))
        for _, g, v in (i for i in items if i[0] == f):
            out[g + 'safe'] += sg * v > safe
            out[g + 'bal'] += sg * v >= bal
            out[g] += 1
    return out

def table(items_of, title):
    print(f'\n## {title}\n')
    print('| signal | AUC P vs N (pooled) | per-file AUC [min-max, files with both] | LOFO safe: P passed / N passed / T passed | LOFO balanced: P / N / T |')
    print('|---|---:|---|---:|---:|')
    for sig in SIG:
        it = items_of(sig)
        P = [v for _, g, v in it if g == 'P']; N = [v for _, g, v in it if g == 'N']
        a = auc(P, N)
        pf = [auc([v for ff, g, v in it if ff == f and g == 'P'], [v for ff, g, v in it if ff == f and g == 'N']) for f in sorted({i[0] for i in it})]
        pf = [max(x, 1 - x) for x in pf if x == x]
        o = lofo(it, sig)
        nT = f' / {o["Tsafe"]}/{o["T"]}' if o['T'] else ''
        nTb = f' / {o["Tbal"]}/{o["T"]}' if o['T'] else ''
        rng = f'[{min(pf):.2f}-{max(pf):.2f}, {len(pf)}f]' if pf else '-'
        print(f'| {sig} | {max(a, 1 - a):.2f} ({"P>N" if a >= 0.5 else "P<N"}) | {rng} | '
              f'{o["Psafe"]}/{o["P"]} / {o["Nsafe"]}/{o["N"]}{nT} | {o["Pbal"]}/{o["P"]} / {o["Nbal"]}/{o["N"]}{nTb} |'
              + (f' ({o["skipped"]} fold(s) without both classes skipped)' if o['skipped'] else ''))

cnt = collections.Counter(group(k) for k in S)
print(f'RGB-source input pages: {len(S)} (P = only coloured text {cnt["P"]}, N = picture/diagram/cover {cnt["N"]}, T = text only {cnt["T"]}); '
      f'1-bit/grey pages skipped: {len(PG) - len(S)}')
print(f'settings: colour pixel chroma >= {CHROMA}, tint = light pixel with chroma {TINT[0]}-{TINT[1] - 1}, {BINS} hue bins, '
      f'cluster >= {CLUSTER:.0%}, k-means k = {KMEANS}; page signals inside the pass-A crop box at 150 dpi')
print('\nsignals: k = hue clusters of colour pixels; top2 = colour mass in the best two 30° hue windows; col_area = colour pixel share; '
      'lstd = luma std of colour pixels; fit4 = mean RGB distance (0-1) of colour pixels to 4 k-means colours; tint = tinted share of light pixels')
table(lambda sig: [(k[0], group(k), S[k][sig]) for k in S], 'Pages: P (only coloured text) vs N (picture/diagram/cover); T = text-only pages, shown for the same cut')
table(lambda sig: [(k[0], 'P' if c == 'coloured-text' else 'N', s[sig]) for k, c, s in RS],
      'Labelled boxes: coloured-text (P) vs picture/diagram/cover (N), signals inside the box')

print('\n## Per page, P and N (? = uncertain label)\n')
print('| page | group | classes | k | hues | top2 | col_area | lstd | fit4 | tint |')
print('|---|---|---|---:|---|---:|---:|---:|---:|---:|')
for k in sorted(S, key=lambda k: (group(k), k[0], k[1])):
    if group(k) == 'T':
        continue
    s = S[k]
    print(f'| {k[0]} p{k[1]}{"" if LAB[k][1] else "?"} | {group(k)} | {",".join(sorted(LAB[k][0] & KEEP))} | {s["k"]} | {s["hues"]} | '
          f'{s["top2"]:.2f} | {s["col_area"]:.4f} | {s["lstd"]:.3f} | {s["fit4"]:.3f} | {s["tint"]:.3f} |')
print('\nk per group (pages): ' + '; '.join(f'{g}: ' + ', '.join(f'k={v} {n}' for v, n in sorted(collections.Counter(S[k]['k'] for k in S if group(k) == g).items()))
                                        for g in 'PNT'))
print('k per labelled box: ' + '; '.join(f'{c}: ' + ', '.join(f'k={v} {n}' for v, n in sorted(collections.Counter(s['k'] for _, cc, s in RS if cc == c).items()))
                                        for c in sorted(KEEP)))
