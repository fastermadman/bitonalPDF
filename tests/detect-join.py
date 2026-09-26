#!/usr/bin/env python3
"""Join `bitonalpdf --detect-eval` output with hand labels and score every signal (#43, docs/rust-port.md section 4).

Usage: tests/detect-join.py det.tsv <folder with X.pdf and X.labels> [--tsv joined.tsv]
Labels (`X.labels`, tab-separated, git-ignored with the PDFs): page, classes, certain, regions, note. A region is
`class:x0,y0,x1,y1` (fractions of the rendered page), '?' after the box = uncertain. No thresholds or rules here:
per signal and class pair it prints AUC (direction-free, max(AUC, 1-AUC)) and best-cut balanced accuracy.
"""
import sys, os, collections
from bisect import bisect_right

det, folder = sys.argv[1], sys.argv[2]
out_tsv = sys.argv[sys.argv.index('--tsv') + 1] if '--tsv' in sys.argv else None

def read_labels(name):
    L = {}
    for line in open(os.path.join(folder, name[:-4] + '.labels'), encoding='utf-8'):
        if line.startswith('#') or not line.strip():
            continue
        p, cls, cert, regs, *_ = line.rstrip('\n').split('\t') + ['']
        R = []
        for r in filter(None, regs.split(';')):
            k, b = r.split(':')
            unsure = b.endswith('?')
            R.append((k, *map(float, b.rstrip('?').split(',')), not unsure))
        L[int(p)] = (set(cls.split(',')), cert == '1', R)
    return L

def tile_label(x0, y0, x1, y1, W, H, regs):
    """Class of the region covering >= 75 % of the tile; 'rest' if no region touches it; None (dropped) otherwise."""
    t = (x0 / W, y0 / H, x1 / W, y1 / H)
    area = (t[2] - t[0]) * (t[3] - t[1])
    best, any_touch = None, False
    for k, a, b, c, d, _ in regs:
        w = min(t[2], c) - max(t[0], a); h = min(t[3], d) - max(t[1], b)
        ov = max(w, 0) * max(h, 0)
        any_touch |= ov > 0
        if ov / area >= 0.75:
            best = k
    return best if best else (None if any_touch else 'rest')

rows, hdr, labels, rgbpage = [], None, {}, {}
for line in open(det, encoding='utf-8'):
    f = line.rstrip('\n').split('\t')
    if line.startswith('#'):
        hdr = f[9:]
        continue
    name, page, scope = f[0], int(f[1]), f[2]
    x0, y0, x1, y1, W, H = map(int, f[3:9])
    sig = list(map(float, f[9:]))
    if name not in labels:
        labels[name] = read_labels(name)
    classes, cert, regs = labels[name][page]
    if scope == 'page':
        rgbpage[(name, page)] = sig[0] > 0
    if scope == 'page':
        lab = 'text-only' if classes == {'text'} else None
        rows.append((name, page, 'page', lab, classes, cert, sig, None))
    else:
        rows.append((name, page, 'tile', tile_label(x0, y0, x1, y1, W, H, regs), classes, cert, sig, None))

rows = [r[:7] + (rgbpage[(r[0], r[1])],) for r in rows]

def auc_ba(pos, neg):
    """AUC (Mann-Whitney) and best-cut balanced accuracy of one signal, direction chosen to favour the score."""
    allv = sorted([(v, 1) for v in pos] + [(v, 0) for v in neg])
    n1, n0 = len(pos), len(neg)
    rank, i, s = 1, 0, 0.0
    while i < len(allv):
        j = i
        while j < len(allv) and allv[j][0] == allv[i][0]:
            j += 1
        avg = (2 * rank + (j - i) - 1) / 2
        s += avg * sum(l for _, l in allv[i:j])
        rank += j - i
        i = j
    auc = (s - n1 * (n1 + 1) / 2) / (n1 * n0)
    best, sp, sn = 0.0, sorted(pos), sorted(neg)
    for c in sorted(set(v for v, _ in allv)):  # predict pos when value > c, or the reverse
        tpr = 1 - bisect_right(sp, c) / n1
        tnr = bisect_right(sn, c) / n0
        best = max(best, (tpr + tnr) / 2, (2 - tpr - tnr) / 2)
    return max(auc, 1 - auc), best

def matrix(title, scope, pairs, key, rgb_only):
    """One table: rows = signals, columns = class pairs, cell = pooled AUC / best-cut balanced accuracy [per-file AUC range]."""
    R = [r for r in rows if r[2] == scope and (r[7] or not rgb_only)]
    print(f'\n### {title}{" (RGB-source pages only)" if rgb_only else ""}\n')
    sets = []
    for a, b in pairs:
        pa = [r for r in R if key(r, a)]; pb = [r for r in R if key(r, b)]
        sets.append((a, b, pa, pb))
    print('| signal | ' + ' | '.join(f'{a} vs {b}' for a, b, *_ in sets) + ' |')
    print('|---|' + '---:|' * len(sets))
    print('| n | ' + ' | '.join(f'{len(pa)} / {len(pb)}' for _, _, pa, pb in sets) + ' |')
    for i, sname in enumerate(hdr):
        cells = []
        for a, b, pa, pb in sets:
            if len(pa) < 3 or len(pb) < 3:
                cells.append('-'); continue
            u, ba = auc_ba([r[6][i] for r in pa], [r[6][i] for r in pb])
            per = []
            for fl in sorted({r[0] for r in pa} & {r[0] for r in pb}):
                x = [r[6][i] for r in pa if r[0] == fl]; y = [r[6][i] for r in pb if r[0] == fl]
                if len(x) >= 3 and len(y) >= 3:
                    per.append(auc_ba(x, y)[0])
            rng = f' [{min(per):.2f}-{max(per):.2f}, {len(per)}f]' if len(per) > 1 else ''
            cells.append(f'{u:.2f} / {ba:.2f}{rng}')
        print(f'| {sname} | ' + ' | '.join(cells) + ' |')

for rgb_only in (False, True):
    matrix('Tiles: class vs rest (rest = no region touches the tile: text, margins, scanner surround)', 'tile',
           [(c, 'rest') for c in ['picture', 'diagram', 'coloured-text', 'cover']], lambda r, c: r[3] == c, rgb_only)
    matrix('Tiles: class pairs', 'tile',
           [('picture', 'diagram'), ('picture', 'coloured-text'), ('diagram', 'coloured-text')], lambda r, c: r[3] == c, rgb_only)
    matrix('Pages: pages having the class vs pages with body text only', 'page',
           [(c, 'text-only') for c in ['picture', 'diagram', 'coloured-text', 'cover']],
           lambda r, c: (r[3] == 'text-only') if c == 'text-only' else (c in r[4]), rgb_only)
    matrix('Pages: has the one class and neither of the others', 'page',
           [('picture', 'diagram'), ('picture', 'coloured-text')],
           lambda r, c: c in r[4] and not ({'picture', 'diagram', 'coloured-text'} - {c}) & r[4], rgb_only)

counts = collections.Counter((r[2], r[3]) for r in rows)
print('\nCounts:', dict(counts))
if out_tsv:
    with open(out_tsv, 'w') as f:
        f.write('file\tpage\tscope\tlabel\tclasses\tcertain\t' + '\t'.join(hdr) + '\n')
        for n, p, sc, lab, cl, ce, sg, _ in rows:
            f.write(f'{n}\t{p}\t{sc}\t{lab}\t{",".join(sorted(cl))}\t{int(ce)}\t' + '\t'.join(map(str, sg)) + '\n')
