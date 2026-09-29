#!/usr/bin/env python3
"""Review list for BITONAL_WHITENED=1 (#63): reads the stderr TSV of `bitonalpdf --crop` (files or stdin). Shows rows that
are frame / edge+big / rule (keep_box's deliberate big removals) or letter-sized (letters >= 1) and outside the text block.
First a per-page line (rows, largest l, largest first), then the rows: header/footer zone first, then by size.
Usage: BITONAL_WHITENED=1 bitonalpdf --crop in.pdf out.pdf 2>&1 >/dev/null | tests/whitened.py      or   tests/whitened.py log1 log2"""
import sys
ZONE = 0.10  # top/bottom share of the slot height that counts as header/footer zone
BIG = {"frame", "edge+big", "rule"}
for f in sys.argv[1:] or ["-"]:
    rows = [l.rstrip("\n").split("\t") for l in (sys.stdin if f == "-" else open(f)) if l.startswith("whitened\t")]
    hit = [r for r in rows if r[10] in BIG or (float(r[8]) >= 1 and r[9] == "1")]
    zone = lambda r: len(r) > 11 and (int(r[4]) < ZONE * int(r[11]) or int(r[6]) > (1 - ZONE) * int(r[11]))
    pages = {}
    for r in hit:
        n, m = pages.get((int(r[1]), int(r[2])), (0, 0))
        pages[(int(r[1]), int(r[2]))] = (n + 1, max(m, float(r[8])))
    for (p, s), (n, m) in sorted(pages.items(), key=lambda kv: -kv[1][1]):
        print(f"{f}\tpage {p} slot {s}: {n} rows, largest l {m:.2f}")
    for r in sorted(hit, key=lambda r: (not zone(r), -float(r[8]))):
        print(f"{f}\t{'ZONE ' if zone(r) else ''}page {r[1]} slot {r[2]} x {r[3]}-{r[5]} y {r[4]}-{r[6]} px {r[7]} l {r[8]} {r[10]}")
    print(f"{f}: {len(hit)} to review of {len(rows)} whitened")
