#!/usr/bin/env python3
"""Dark edge pieces in output PDFs (#53, #55): per page, connected ink pieces at least 60 % of the page height, narrower
than 30 % of the width, in the outer 35 % (the same rule as clear_edge_bands in rust/src/main.rs, measured on the
finished page at 100 dpi). Prints the flagged pages per file and a summary line; exit code 0 always (a measurement).
Usage: tests/edge-bands.py out1.pdf [out2.pdf ...]      Needs pdftoppm, numpy, Pillow, scipy."""
import glob, os, subprocess, sys, tempfile
import numpy as np
from PIL import Image
from scipy import ndimage as ndi

for f in sys.argv[1:]:
    with tempfile.TemporaryDirectory() as t:
        subprocess.run(["pdftoppm", "-r", "100", "-gray", f, t + "/p"], check=True)
        pages = sorted(glob.glob(t + "/p-*.pgm"))
        flagged = []
        for i, pf in enumerate(pages, 1):
            im = np.array(Image.open(pf)) < 128
            h, w = im.shape
            lab, _ = ndi.label(ndi.binary_dilation(im), structure=np.ones((3, 3)))
            for s in ndi.find_objects(lab):
                y0, y1, x0, x1 = s[0].start, s[0].stop, s[1].start, s[1].stop
                if y1 - y0 >= 0.6 * h and x1 - x0 < 0.3 * w and (x1 < 0.35 * w or x0 > 0.65 * w):
                    flagged.append((i, x0, x1, y0, y1))
        for p in flagged:
            print(f"{os.path.basename(f)} page {p[0]}: x {p[1]}-{p[2]} y {p[3]}-{p[4]}")
        print(f"{os.path.basename(f)}: {len({p[0] for p in flagged})} of {len(pages)} pages with a dark edge piece")
