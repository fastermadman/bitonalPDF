#!/usr/bin/env python3
"""Per-file summary for #55 (docs/rust-port.md section 8): run on the output of tests/measure.sh and tests/edge-bands.py.
Prints page count, page sizes, edge-band count, |L-R| margin difference in mm (max, pages > 2 mm), spread (std) of the ink
box's x0/y0 in per-mille, and the footer flags per page.
Usage: tests/page-metrics.py <name> <file.facts> <file.bands>"""
import sys, statistics as st
name, facts, bands = sys.argv[1:4]
rows = [l.split() for l in open(facts) if l.strip()]
sizes = {(r[1], r[2]) for r in rows}
d, x0s, y0s = [], [], []
for r in rows:
    w = float(r[1]); x0, y0, x1, y1 = map(int, r[3:7])
    if x1 <= x0: continue
    d.append((abs(x0 - (1000 - x1)) * w * 25.4 / 72 / 1000, int(r[0]))); x0s.append(x0); y0s.append(y0)
nb = open(bands).read().strip().splitlines()[-1].split(': ', 1)[1]
bad = [p for v, p in d if v > 2]
print(f"{name}: pages {len(rows)} sizes {len(sizes)} {sorted(sizes)} | bands: {nb} | max|L-R| {max(v for v, _ in d):.1f} mm, "
      f">2mm on {len(bad)} pages {bad} | std x0 {st.pstdev(x0s):.1f} y0 {st.pstdev(y0s):.1f} permille | foot {''.join(r[7] for r in rows)}")
