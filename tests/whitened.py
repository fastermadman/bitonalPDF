#!/usr/bin/env python3
"""Review list for BITONAL_WHITENED=1 (#63): reads the stderr TSV of `bitonalpdf --crop` (files or stdin), prints rows that
are letter-sized or larger (letters >= 1) and outside the text block, and a count per input. Usage:
  BITONAL_WHITENED=1 bitonalpdf --crop in.pdf out.pdf 2>&1 >/dev/null | tests/whitened.py      or   tests/whitened.py log1 log2"""
import sys
for f in sys.argv[1:] or ["-"]:
    rows = [l.rstrip("\n").split("\t") for l in (sys.stdin if f == "-" else open(f)) if l.startswith("whitened\t")]
    hit = [r for r in rows if float(r[8]) >= 1 and r[9] == "1"]
    for r in hit:
        print(f"{f}\tpage {r[1]} slot {r[2]} x {r[3]}-{r[5]} y {r[4]}-{r[6]} px {r[7]} l {r[8]} {r[10]}")
    print(f"{f}: {len(hit)} to review of {len(rows)} whitened")
