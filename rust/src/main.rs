// bitonalPDF Rust spike (#28 PDF I/O, #29 image ops): PDF in (embedded page image or render) -> rotate/crop/split/
// deskew -> 1-bit -> CCITT G4 PDF out. Same CLI as bitonalpdf.sh (text mode):
//   bitonalpdf [--rotate] [--crop] [--split auto|off|N%] [--deskew] input.pdf [output.pdf] [threshold%=60] [dpi=300]
// BITONAL_TIMING=1 prints per-page geometry and stage times (ms) to stderr; BITONAL_RENDER=1 always renders.
// A/B knobs for the pipeline-order questions of #29 (docs/rust-port.md section 2):
//   BITONAL_ADPI=<dpi>      analysis (crop/gutter) resolution, default = dpi (150 flips crop decisions, section 2)
//   BITONAL_SLOTFLAT=1      flatten each slot after crop/deskew like bash, instead of once on the full page
//   BITONAL_DESKEW_FIRST=1  estimate the skew on the whole page and straighten it before measuring crop/gutter
//   BITONAL_INKGUTTER=1     gutter profile from the 60 % ink map instead of the lenient grey level (threshold once)
//   BITONAL_KEEP_D=<n>      (#55, #60) with --crop, each slot keeps the text block and the components near it (keep_box:
//                           "near" = n letter heights, default 4), is cropped to them and centred / top-aligned with
//                           the others on one canvas (compose; docs/rust-port.md section 8)
//   BITONAL_WHITENED=1      (#63) with --crop, keep_box prints one TSV row per whitened component to stderr ("whitened" page slot
//                           x0 y0 x1 y1 pixels letters outside reason slot_h); tests/whitened.py filters it. Off: no change.
//                           (#84) plus one "reached" row per page number added from beyond the slot (page slot x0 y0 x1 y1
//                           pieces height_in_letters); (#87) one "line" row per clipped text line restored (page slot x0 y0 x1 y1
//                           class white_rows_in_letters cut), class = body | heading | headfoot.
//   BITONAL_OSD=tesseract   orientation by Tesseract OSD (as bash) instead of the own detector (#30); --osd-eval prints its accuracy
//                           (EVAL_UPRIGHT=1: the input pages are upright, no Tesseract truth; #45)
// --detect-eval a.pdf [b.pdf ...] prints candidate picture/colour signals per page and tile as TSV (#43, docs/rust-port.md section 4).
use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::hayro_syntax::page::Page;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, ImageData, InterpreterCache, InterpreterSettings,
    Paint, TransformExt, PathDrawMode, SoftMask, interpret_page,
};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderCache, RenderSettings, render};
use kurbo::{Affine, BezPath, Rect};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

// Tunables, same names and values as bitonalpdf.sh (see there and docs/lessons.md for why).
const OSD_MIN_CONFIDENCE: f32 = 1.0;
// Own orientation detector (#30): downsample factors for the axis and direction stages, tile size at the axis
// stage's resolution, minimum confidence (below it the page is left as it is).
const OSD_KA: usize = 8;
const OSD_KD: usize = 4;
const OSD_TILE: usize = 48;
const OSD_OWN_MIN_CONFIDENCE: f32 = 0.25;
// Darkness (0..1, flattened) above which a profile value is not text: text lines peak at ~0.2-0.3 (#45).
const OSD_DARK_MAX: f64 = 0.4;
const DOUBLE_AR_MIN: f64 = 1.15;
const GUTTER_INK_THRESH: f32 = 230.0;
const GUTTER_SEARCH_LO: f64 = 0.20;
const GUTTER_SEARCH_HI: f64 = 0.80;
const GUTTER_MIN_WIDTH_FRAC: f64 = 0.006;
const GUTTER_MAX_WIDTH_FRAC: f64 = 0.22;
const GUTTER_MIN_INK_FRAC: f64 = 0.03;
const GUTTER_EDGE_SHAVE: f64 = 0.03;
const GUTTER_MIN_INK_ROWS: u32 = 2;
const GUTTER_TRUST_FRAC: f64 = 0.08;
const GUTTER_VALLEY_RATIO: f64 = 0.5;
const SPLIT_SLIVER_FRAC: f64 = 0.5; // #88: a half the spread box reaches less than this share of the other half gets its own box (skewed p1 0.25, f7 p1 0.31; next 0.61)
const GRID_ROWS: usize = 48;
// Dark edge artefact (#53): after closing pinholes, a connected piece this tall (fraction of the slot height) and this
// narrow, in the outer zone (fraction of the width). Text never makes a component that tall.
const BAND_MIN_HEIGHT_FRAC: f64 = 0.6;
const BAND_MAX_WIDTH_FRAC: f64 = 0.3;
const BAND_ZONE_FRAC: f64 = 0.35;
const BAND_CLOSE: usize = 3; // px at the output dpi
const CROP_MIN_DENSITY: f64 = 0.03;
const CROP_MAX_DENSITY: f64 = 0.55;
const CROP_MIN_RUN: f64 = 0.005;
const CROP_RUN_GAP: f64 = 0.003;
const CROP_NEAR_DENSITY: f64 = 0.004;
const CROP_NEAR_FRAC: f64 = 0.08;
const CROP_BAND_FRAC: f64 = 0.10;
const CROP_EDGE_FRAC: f64 = 0.015;
const CROP_PAD_FRAC: f64 = 0.012;
// #91: a first/last run the band rule would drop is kept when it reads as text: not tall, and little of its ink in long horizontal segments.
const BAND_TEXT_LONG: f64 = 0.15; // #91: max share of ink in long segments (text 0.00-0.09, real bands >= 0.29)
const BAND_TEXT_SEG_MM: f64 = 3.8; // #91: min length of a "long" horizontal ink segment
const BAND_TEXT_MAX_H_MM: f64 = 5.9; // #91: max height of the kept run
const KEEP_FRAME_FILL: f64 = 0.1; // keep_box: a piece with less ink than this share of its box is a frame/edge candidate
const KEEP_ZONE_D: f64 = 12.0; // keep_box: sideways reach on the header/footer line, in letter heights
const KEEP_HEAD_D: f64 = 24.0; // keep_box: sideways reach on the block's top/bottom kept line, in letter heights (f1 p3: 18)
const KEEP_EDGE_MAX: usize = 16; // keep_box: more big pieces than this touching a cut side switch the edge rule off (cover, #66)
const KEEP_RULE_FRAC: f64 = 0.25; // keep_box: a thin piece longer than this share of the slot height is a rule (f7 p4's bar is shorter)
const KEEP_REACH_FRAC: f64 = 0.02; // #84: ring outside a slot's cut sides searched for page numbers; R and TIGHT_RIM use the same measure, max(w, h), so the ring fits in the rim
const KEEP_REACH_ISO: f64 = 3.0; // #84: a reached page number stands this many letter heights clear of other ring letters (f1 p24's clipped head: 1.2-2.5)
const KEEP_REACH_H: (f64, f64) = (1.2, 1.8); // #84: its tallest digit, in letter heights (f1 digits 1.33-1.61; head letters <= 1.05, cartoon/margin pieces >= 1.93)
const KEEP_LINE_GAP: f64 = 1.0; // #87: clipped line pieces link across this sideways gap, in letter heights (word spaces 0.52-0.64; 0.3 and 0.6 dropped words)
const KEEP_LINE_MIN: usize = 3; // #87: a restored line has at least this many letter pieces ...
const KEEP_LINE_INK: f64 = 0.8; // #87: ... carrying this share of its ink (f1 p43's speckle: 44 %)
const KEEP_LINE_DIA: f64 = 0.5; // #87: a diacritic piece is at most this tall and within this distance of the line, in letter heights
const KEEP_LINE_CLASS: (f64, f64) = (1.25, 4.0); // #87: logged class by white rows to the block: body line < 1.25 l <= heading < 4 l <= head/footer
const TIGHT_RIM: f32 = 0.03; // --crop: white rim around a slot before its box is measured (> CROP_EDGE_FRAC)
const BLUR_SIGMA: f32 = 30.0; // px at the output dpi, like bash's -blur 0x30
const COVER_BLUR_SIGMA: f32 = 1000.0; // px, a cover slot's flatten (#71): wider than f5 p1's circles, so fills stay grey
const SKEW_MAX_DEG: f32 = 10.0;

struct Gray {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

/// Grey image, 0 = black .. 1 = white.
#[derive(Clone)]
struct Img {
    w: usize,
    h: usize,
    px: Vec<f32>,
}

#[derive(Clone, Copy, PartialEq)]
enum Split {
    Off,
    Auto,
    Fixed(f64),
}

struct Cfg {
    rotate: bool,
    crop: bool,
    deskew: bool,
    split: Split,
    thresh: f32,
    dpi: f32,
    adpi: f32,
    slot_flat: bool,
    deskew_first: bool,
    ink_gutter: bool,
    keep_d: f64, // BITONAL_KEEP_D: keep_box's distance in median letter heights
    whitened: bool, // BITONAL_WHITENED=1 (#63): keep_box lists what it whitens on stderr
    timing: bool,
    tess: bool, // BITONAL_OSD=tesseract: the old detector
}

/// Pass A result for one page, in output-dpi pixels (after the quarter-turn `rot`, and after straightening by
/// `angle` when deskew-first).
#[derive(Default, Clone)]
struct Meta {
    rot: u32,
    angle: f32,
    w: i64,
    trim: Option<[i64; 4]>, // x0 y0 x1 y1
    cand: bool,
    gutter: Option<(i64, i64)>, // x, width
}

/// What pass B cuts from a page: nothing (whole page), one box, or a split at gx.
#[derive(Clone, Copy)]
enum Plan {
    Whole,
    Box([i64; 4]),
    Split([i64; 4], i64),
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env = |k: &str| std::env::var_os(k).is_some();
    let mut cfg = Cfg {
        rotate: false,
        crop: false,
        deskew: false,
        split: Split::Off,
        thresh: 60.0,
        dpi: 300.0,
        adpi: std::env::var("BITONAL_ADPI").ok().and_then(|s| s.parse().ok()).unwrap_or(f32::MAX),
        slot_flat: env("BITONAL_SLOTFLAT"),
        deskew_first: env("BITONAL_DESKEW_FIRST"),
        ink_gutter: env("BITONAL_INKGUTTER"),
        whitened: std::env::var("BITONAL_WHITENED").is_ok_and(|v| v == "1"),
        keep_d: std::env::var("BITONAL_KEEP_D").ok().and_then(|s| s.parse().ok()).unwrap_or(4.0),
        timing: env("BITONAL_TIMING"),
        tess: std::env::var("BITONAL_OSD").as_deref() == Ok("tesseract"),
    };
    let mut pos = vec![];
    let mut eval = false;
    let mut detect = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--rotate" => cfg.rotate = true,
            "--osd-eval" => eval = true,
            "--detect-eval" => detect = true,
            "--crop" => cfg.crop = true,
            "--deskew" => cfg.deskew = true,
            "--split" => {
                cfg.split = match it.next().map(String::as_str) {
                    Some("auto") => Split::Auto,
                    Some("off") => Split::Off,
                    Some(s) if s.ends_with('%') => {
                        Split::Fixed(s.trim_end_matches('%').parse().unwrap_or_else(|_| die("bad --split")))
                    }
                    _ => die("--split auto|off|N%"),
                }
            }
            s if s.starts_with("--") => die(&format!("{s}: unknown option")),
            _ => pos.push(a.clone()),
        }
    }
    if detect {
        return detect_eval(&pos);
    }
    let input = pos.first().unwrap_or_else(|| die("usage: bitonalpdf [--rotate] [--crop] [--split auto|off|N%] [--deskew] input.pdf [output.pdf] [threshold%] [dpi]"));
    let output = pos.get(1).cloned().unwrap_or_else(|| format!("{}.1bit.pdf", input.trim_end_matches(".pdf")));
    cfg.thresh = pos.get(2).map_or(60.0, |s| s.parse().unwrap_or_else(|_| die("bad threshold")));
    cfg.dpi = pos.get(3).map_or(300.0, |s| s.parse().unwrap_or_else(|_| die("bad dpi")));
    cfg.adpi = cfg.adpi.min(cfg.dpi);
    if &output == input {
        die("output must differ from input");
    }
    if cfg.rotate && cfg.tess && std::process::Command::new("tesseract").arg("--version").output().is_err() {
        eprintln!("BITONAL_OSD=tesseract needs tesseract — proceeding without rotation");
        cfg.rotate = false;
    }

    let data = Arc::new(std::fs::read(input).unwrap_or_else(|e| die(&format!("{input}: {e}"))));
    let n = Pdf::new(data.clone()).unwrap_or_else(|e| die(&format!("{input}: {e:?}"))).pages().len();
    if eval {
        return osd_eval(&data, n);
    }
    let t0 = Instant::now();

    // Pass A: geometry per page. Only needed for crop/split, or to find the page skew first.
    let geometry = cfg.crop || cfg.split != Split::Off || cfg.rotate || (cfg.deskew && cfg.deskew_first);
    let metas: Vec<Meta> = if geometry { par_pages(&data, n, |p, i| measure(p, i, &cfg)) } else { vec![Meta::default(); n] };
    let t1 = Instant::now();

    // Between passes: document medians, per-page plan, one canvas size (same rules as bitonalpdf.sh).
    let (plans, review) = plan(&metas, &cfg);
    let (mut tw, mut th) = (0, 0);
    for p in &plans {
        match *p {
            Plan::Box([x0, y0, x1, y1]) => (tw, th) = (tw.max(x1 - x0), th.max(y1 - y0)),
            Plan::Split([x0, y0, x1, y1], gx) => (tw, th) = (tw.max(gx - x0).max(x1 - gx), th.max(y1 - y0)),
            Plan::Whole => {}
        }
    }
    if cfg.timing {
        eprintln!("canvas {tw}x{th}");
    }

    // Pass B: cut, deskew, centre on the canvas, flatten + threshold, G4.
    let slots: Vec<(u32, u32, Vec<u8>, [usize; 2])> =
        par_pages(&data, n, |p, i| finish(p, i, &metas[i], plans[i], (tw, th), &cfg)).into_iter().flatten().collect();
    let pages: Vec<(u32, u32, Vec<u8>)> =
        if cfg.crop { compose(slots) } else { slots.into_iter().map(|(w, h, g, _)| (w, h, g)).collect() };
    let t2 = Instant::now();
    if cfg.timing {
        eprintln!("pass A {} ms, pass B {} ms", (t1 - t0).as_millis(), (t2 - t1).as_millis());
    }

    let pdf = write_pdf(&pages, cfg.dpi);
    let (in_len, out_len) = (data.len(), pdf.len());
    if out_len >= in_len {
        println!("Not smaller ({in_len} B is already small) — no file written");
        return;
    }
    std::fs::write(&output, pdf).unwrap_or_else(|e| die(&format!("{output}: {e}")));
    println!("{in_len} B -> {out_len} B: {output}");
    if !review.is_empty() {
        let l: Vec<String> = review.iter().map(|i| (i + 1).to_string()).collect();
        eprintln!("Warning: pages look like double pages but no confident gutter was found — left unsplit: {}", l.join(","));
        eprintln!("Check these by hand, or re-run with --split <N%> to force a gutter position.");
        std::process::exit(2);
    }
}

/// Run f on every page, one Pdf per thread (hayro's page/cache types are not Send), results in page order.
fn par_pages<T: Send>(data: &Arc<Vec<u8>>, n: usize, f: impl Fn(&Page, usize) -> T + Sync) -> Vec<T> {
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(n.max(1));
    let mut out: Vec<(usize, T)> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let pdf = Pdf::new(data.clone()).unwrap();
                    let mut done = vec![];
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= n {
                            break done;
                        }
                        done.push((i, f(&pdf.pages()[i], i)));
                    }
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    out.sort_by_key(|p| p.0);
    out.into_iter().map(|p| p.1).collect()
}

/// The page at the output dpi, turned by its OSD quarter turn.
fn load(page: &Page, cfg: &Cfg, rot: u32) -> Img {
    let (g, _) = page_gray(page, cfg.dpi, true);
    rot90(&Img { w: g.w as usize, h: g.h as usize, px: g.px.iter().map(|&v| v as f32 / 255.0).collect() }, rot)
}

/// Pass A: rotation, skew (deskew-first only), text box and gutter of one page. Measured on a copy
/// downsampled to the analysis dpi, results scaled back to output pixels.
fn measure(page: &Page, i: usize, cfg: &Cfg) -> Meta {
    let t0 = Instant::now();
    let (g, _) = page_gray(page, cfg.dpi, false);
    let mut g = Img { w: g.w as usize, h: g.h as usize, px: g.px.iter().map(|&v| v as f32 / 255.0).collect() };
    let rot = match (cfg.rotate, cfg.tess) {
        (false, _) => 0,
        (true, true) => osd(&g),
        (true, false) => match osd_own(&g, OSD_KA, OSD_KD, OSD_TILE) {
            (q, c) if c >= OSD_OWN_MIN_CONFIDENCE => q,
            _ => 0,
        },
    };
    g = rot90(&g, rot);
    let t1 = Instant::now();
    let k = (cfg.dpi / cfg.adpi).round().max(1.0) as usize;
    let a = downsample(&g, k);
    let mut flat = flatten(&a, BLUR_SIGMA / k as f32);
    let mut angle = 0.0;
    if cfg.deskew && cfg.deskew_first {
        angle = skew_angle(&flat.px.iter().map(|&v| v < 0.6).collect::<Vec<_>>(), flat.w, flat.h);
        flat = rotate(&flat, angle, flat.w, flat.h, 1.0);
    }
    let ink: Vec<bool> = flat.px.iter().map(|&v| v <= 0.6).collect();
    let (w, h) = (g.w as i64, g.h as i64);
    let kk = k as i64;
    let mut m = Meta { rot, angle, w, cand: w as f64 / h as f64 >= DOUBLE_AR_MIN, ..Default::default() };
    let mut bx = (0, flat.w);
    let (fw, fh) = (flat.w, flat.h);
    if cfg.crop || cfg.split != Split::Off {
        let [x0, y0, x1, y1] = content_box(&ink, flat.w, flat.h, true, (cfg.dpi / k as f32) as f64 / 25.4);
        m.trim = Some([x0 as i64 * kk, y0 as i64 * kk, (x1 as i64 * kk).min(w), (y1 as i64 * kk).min(h)]);
        bx = (x0, x1);
    }
    if m.cand {
        m.gutter = match cfg.split {
            Split::Fixed(f) => Some(((w as f64 * f / 100.0) as i64, 0)),
            Split::Auto => {
                let prof = if cfg.ink_gutter {
                    Img { w: flat.w, h: flat.h, px: ink.iter().map(|&b| if b { 0.0 } else { 1.0 }).collect() }
                } else {
                    flat
                };
                gutter(&prof, bx).map(|(x, gw)| (x as i64 * kk, gw as i64 * kk))
            }
            Split::Off => None,
        };
        // #88: the spread box follows the fuller page, so a sparse facing page (skewed p1: a chapter end, 4 lines and "34")
        // is cut to a sliver of line ends. That half's own box, on its ink alone, widens the trim's outer x.
        // ponytail: x only, y stays the spread's (skewed p1's "34" fits); widen y too if a sliver page's lines fall outside it.
        // SPLIT_SLIVER_FRAC is a rule of thumb from two cases, like KEEP_EDGE_MAX.
        if let (Some((gx, _)), Some(t)) = (m.gutter, m.trim.as_mut()) {
            let (l, r) = (gx - t[0], t[2] - gx);
            let ga = (gx / kk) as usize;
            let half = |a: usize, b: usize| {
                let v: Vec<bool> = (0..fh).flat_map(|y| ink[y * fw + a..y * fw + b].iter().copied()).collect();
                let [x0, _, x1, _] = content_box(&v, b - a, fh, true, (cfg.dpi / k as f32) as f64 / 25.4);
                ((a + x0) as i64 * kk, ((a + x1) as i64 * kk).min(w))
            };
            if l > 0 && ga > 0 && (l as f64) < SPLIT_SLIVER_FRAC * r as f64 {
                t[0] = t[0].min(half(0, ga).0);
            }
            if r > 0 && ga < fw && (r as f64) < SPLIT_SLIVER_FRAC * l as f64 {
                t[2] = t[2].max(half(ga, fw).1);
            }
        }
    }
    if cfg.timing {
        eprintln!("A page {}: get+osd {} ms, analysis {} ms; rot {} angle {:.2} box {:?} double {} gutter {:?}",
            i + 1, (t1 - t0).as_millis(), t1.elapsed().as_millis(), m.rot, m.angle, m.trim, m.cand, m.gutter);
    }
    m
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    (n > 0).then(|| if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
}

/// Port of the bash between-passes step: median single-page box, median gutter position, per-page plan.
/// Returns the plans and the pages that look double but got no trusted gutter.
fn plan(metas: &[Meta], cfg: &Cfg) -> (Vec<Plan>, Vec<usize>) {
    let singles: Vec<[i64; 4]> = metas.iter().filter(|m| !m.cand).filter_map(|m| m.trim).collect();
    let single = (cfg.crop && !singles.is_empty()).then(|| {
        // bash: integer median per edge
        let med = |k: usize| median(singles.iter().map(|b| b[k] as f64).collect()).unwrap().trunc() as i64;
        [med(0), med(1), med(2), med(3)]
    });
    let wide = |m: &Meta| m.gutter.is_some_and(|(_, gw)| gw as f64 > GUTTER_TRUST_FRAC * m.w as f64);
    // ponytail: real median; bash's median() truncates the fraction to int, so with an even count it gives 0 there
    let gfrac = if cfg.split == Split::Auto {
        median(metas.iter().filter(|m| m.cand && m.gutter.is_some() && !wide(m)).map(|m| m.gutter.unwrap().0 as f64 / m.w as f64).collect())
    } else {
        None
    };
    let mut review = vec![];
    let plans = metas
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let Some(t) = m.trim else { return Plan::Whole };
            if cfg.split != Split::Off && m.cand {
                let gx = match (m.gutter, gfrac) {
                    (Some((x, _)), _) if gfrac.is_none() || !wide(m) => Some(x),
                    (_, Some(f)) if cfg.split == Split::Auto => Some((f * m.w as f64) as i64),
                    _ => None,
                };
                if let Some(gx) = gx {
                    let [mut x0, y0, mut x1, y1] = t;
                    // a blank facing page: mirror the other half's width instead of collapsing it
                    if x0 >= gx {
                        x0 = (gx - (x1 - gx)).max(0);
                    }
                    if x1 <= gx {
                        x1 = (gx + (gx - x0)).min(m.w);
                    }
                    return Plan::Split([x0, y0, x1, y1], gx);
                }
                review.push(i);
            }
            match (cfg.crop, single) {
                (true, Some(s)) => Plan::Box(s),
                (true, None) => Plan::Box(t),
                _ => Plan::Whole,
            }
        })
        .collect();
    (plans, review)
}

/// Pass B: one or two finished slots (w, h, G4) for a page.
/// With --crop: instead of G4, the slot's ink box packed 1 bit/px plus the text block's anchor in it (centre x, top y).
fn finish(page: &Page, i: usize, m: &Meta, plan: Plan, (tw, th): (i64, i64), cfg: &Cfg) -> Vec<(u32, u32, Vec<u8>, [usize; 2])> {
    let t0 = Instant::now();
    let mut src = load(page, cfg, m.rot);
    let t1 = Instant::now();
    if !cfg.slot_flat {
        src = flatten(&src, BLUR_SIGMA);
    }
    if m.angle != 0.0 {
        src = rotate(&src, m.angle, src.w, src.h, 1.0);
    }
    let (w, h) = (src.w as i64, src.h as i64);
    let boxes = match plan {
        Plan::Whole => vec![[0, 0, w, h]],
        Plan::Box(b) => vec![b],
        Plan::Split([x0, y0, x1, y1], gx) => vec![[x0, y0, gx, y1], [gx, y0, x1, y1]],
    };
    let tight = cfg.crop;
    let extent = tw > 0 && !matches!(plan, Plan::Whole);
    let mut angles = vec![];
    let out = boxes
        .iter()
        .enumerate()
        .map(|(slot, &b)| {
            let s = crop(&src, b);
            // Skew per slot: on the ink of the flattened slot (bash: raw grey < 40 %).
            let angle = if cfg.deskew {
                let ink: Vec<bool> = if cfg.slot_flat { s.px.iter().map(|&v| v < 0.4).collect() } else { s.px.iter().map(|&v| v < 0.6).collect() };
                skew_angle(&ink, s.w, s.h)
            } else {
                0.0
            };
            angles.push(angle);
            let (cw, ch) = if tight {
                // own size, enlarged so no corner is cut, plus a white rim so content_box's edge rules never bite
                let (sn, cs) = angle.to_radians().abs().sin_cos();
                let (w, h) = (s.w as f32, s.h as f32);
                let rim = 2.0 * TIGHT_RIM * w.max(h);
                ((w * cs + h * sn + rim).ceil() as usize, (h * cs + w * sn + rim).ceil() as usize)
            } else if extent {
                (tw as usize, th as usize)
            } else {
                (s.w, s.h)
            };
            let mut c = rotate(&s, angle, cw, ch, 1.0);
            if cfg.slot_flat {
                c = flatten(&c, BLUR_SIGMA);
            }
            let mut bits = hyst(&c, cfg.thresh);
            clear_edge_bands(&mut bits, c.w, c.h);
            if tight {
                // What to keep, per component (section 8): the text block, and whatever lies near it; not what
                // touches the slot's border or is a thin tall rule, unless it reaches the block's core (a full-bleed
                // picture). Kept ink is cropped, centred and top-aligned by compose.
                let a = content_box(&bits, c.w, c.h, false, 0.0);
                let (sn, cs) = angle.to_radians().sin_cos();
                let (ox, oy) = ((c.w as i64 - s.w as i64) / 2, (c.h as i64 - s.h as i64) / 2);
                let (gcx, gcy) = (s.w as f32 / 2.0, s.h as f32 / 2.0);
                let (ccx, ccy) = (ox as f32 + gcx, oy as f32 + gcy);
                // same mapping as rotate(): output pixel -> source slot pixel, then "within 2 px of a side that is a cut"
                // (a side where the pre-crop box reached the page edge is not one: content may run off the page, f5 p1)
                let cut = [b[0] > 0, b[1] > 0, b[2] < w, b[3] < h];
                let near_edge = |x: usize, y: usize| {
                    let (u, v) = (x as f32 + 0.5 - ccx, y as f32 + 0.5 - ccy);
                    let (sx, sy) = (gcx + u * cs - v * sn - 0.5, gcy + u * sn + v * cs - 0.5);
                    (cut[0] && sx < 2.0) || (cut[1] && sy < 2.0) || (cut[2] && sx > s.w as f32 - 3.0) || (cut[3] && sy > s.h as f32 - 3.0)
                };
                let log = cfg.whitened.then_some((i + 1, slot + 1));
                let ([mut x0, mut y0, mut x1, mut y1], cover, (kl, kk, ktop, kbot)) = keep_box(&mut bits, c.w, c.h, a, &near_edge, cfg.keep_d, log);
                if cover {
                    // A cover (#71): the flatten turns a fill wider than BLUR_SIGMA white, and light text on it with it (f5 p1's
                    // circles). Threshold the unflattened slot again, flattened only against light falloff, and keep from that.
                    // ponytail: re-decodes the page for the (rare) cover slot instead of holding an unflattened copy of every page
                    let mut r = load(page, cfg, m.rot);
                    if m.angle != 0.0 {
                        r = rotate(&r, m.angle, r.w, r.h, 1.0);
                    }
                    let rc = flatten(&rotate(&crop(&r, b), angle, cw, ch, 1.0), COVER_BLUR_SIGMA);
                    bits = hyst(&rc, cfg.thresh);
                    clear_edge_bands(&mut bits, rc.w, rc.h);
                    let a = content_box(&bits, rc.w, rc.h, false, 0.0);
                    [x0, y0, x1, y1] = keep_box(&mut bits, rc.w, rc.h, a, &near_edge, cfg.keep_d, log).0;
                }
                if !cover && cut.contains(&true) {
                    // #84: a page number the plan box clipped (f1 p18, p24, p50). The slot grown by R on its cut sides (not the
                    // gutter: the facing page's head), same skew, canvas grown by R each side, so main canvas pixel (x, y) is ring
                    // pixel (x + R, y + R). Main's kept ink stays as it is; reach only adds whole pieces.
                    let tr = Instant::now();
                    let r = (KEEP_REACH_FRAC * s.w.max(s.h) as f64) as usize;
                    let split = matches!(plan, Plan::Split(..));
                    let grow = [cut[0] && !(split && slot == 1), cut[1], cut[2] && !(split && slot == 0), cut[3]];
                    let (pw, ph) = (s.w + 2 * r, s.h + 2 * r);
                    let mut p = Img { w: pw, h: ph, px: vec![1.0; pw * ph] };
                    for y in 0..ph {
                        let sy = b[1] + y as i64 - r as i64;
                        if sy < 0 || sy >= h || (sy < b[1] && !grow[1]) || (sy >= b[3] && !grow[3]) {
                            continue;
                        }
                        for x in 0..pw {
                            let sx = b[0] + x as i64 - r as i64;
                            if sx >= 0 && sx < w && (sx >= b[0] || grow[0]) && (sx < b[2] || grow[2]) {
                                p.px[y * pw + x] = src.px[sy as usize * src.w + sx as usize];
                            }
                        }
                    }
                    let mut rc = rotate(&p, angle, cw + 2 * r, ch + 2 * r, 1.0);
                    if cfg.slot_flat {
                        rc = flatten(&rc, BLUR_SIGMA);
                    }
                    let mut rb = hyst(&rc, cfg.thresh);
                    clear_edge_bands(&mut rb, rc.w, rc.h);
                    let ms = tr.elapsed().as_millis();
                    if cfg.timing {
                        eprintln!("R page {} slot {}: ring {} ms, R {} px, grow {:?}", i + 1, slot + 1, ms, r, grow);
                    }
                    if let Some(e) = reach(&rb, rc.w, rc.h, r, s.w, s.h, (ccx, ccy), (gcx, gcy), (sn, cs), grow, cut, a, (kl, kk, ktop, kbot), &mut bits, c.w, c.h, log) {
                        let (px, py) = ((CROP_PAD_FRAC * c.w as f64) as i64, (CROP_PAD_FRAC * c.h as f64) as i64);
                        x0 = x0.min((e[0] - px).max(0) as usize);
                        y0 = y0.min((e[1] - py).max(0) as usize);
                        x1 = x1.max((e[2] + px).min(c.w as i64) as usize);
                        y1 = y1.max((e[3] + py).min(c.h as i64) as usize);
                    }
                }
                let (w, h) = (x1 - x0, y1 - y0);
                let mut packed = vec![0u8; w.div_ceil(8) * h];
                for y in 0..h {
                    for x in 0..w {
                        if bits[(y0 + y) * c.w + x0 + x] {
                            packed[y * w.div_ceil(8) + x / 8] |= 0x80 >> (x % 8);
                        }
                    }
                }
                return (w as u32, h as u32, packed, [w / 2, 0]);
            }
            (c.w as u32, c.h as u32, encode_g4(&bits, c.w as u32), [0, 0])
        })
        .collect();
    if cfg.timing {
        eprintln!("B page {}: get {} ms, rest {} ms, deskew {:?}", i + 1, (t1 - t0).as_millis(), t1.elapsed().as_millis(), angles);
    }
    out
}

/// --crop (#55): which ink of a deskewed slot to keep, as connected components. Seeds: components that
/// meet the text block `a` ([x0,y0,x1,y1)). Then, repeatedly, any component within `dmul` median letter heights of
/// the kept box joins it (margin labels, a title running past the block, letter by letter), or at any distance when
/// it is straight above/below it and more than a speck (page numbers, running heads: often > 8 % below the block), or
/// when it is a letter above/below the text block within KEEP_ZONE_D letters sideways (a page number beside the head).
/// Never kept: big sparse pieces reaching out of the block (book edges, frames); pieces bigger than 3 letters within
/// 2 px of a cut side of the slot (`near_edge`), or thin and longer than KEEP_RULE_FRAC of it, unless they reach the
/// middle half of the block, or unless more than KEEP_EDGE_MAX big pieces touch a cut side (a cover: the edge rule is off,
/// #66; the threshold is a rule of thumb measured on f1-f7).
/// Everything not kept is whitened; returns the kept box, padded like content_box, and whether the slot is a cover (edge rule off).
// ponytail: distance to the kept box, not to each component: a big picture's box can pull in scraps beside it
fn keep_box(bits: &mut [bool], w: usize, h: usize, a: [usize; 4], near_edge: &dyn Fn(usize, usize) -> bool, dmul: f64, log: Option<(usize, usize)>) -> ([usize; 4], bool, (f64, [usize; 4], usize, usize)) {
    let (lab, comps) = components(bits, w, h);
    let n = comps.len();
    let mut bb = vec![[usize::MAX, usize::MAX, 0, 0]; n]; // x0 y0 x1 y1, exclusive
    let mut edge = vec![false; n];
    for y in 0..h {
        for x in 0..w {
            if bits[y * w + x] {
                let l = lab[y * w + x] as usize;
                let b = &mut bb[l];
                *b = [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)];
                if !edge[l] && near_edge(x, y) {
                    edge[l] = true;
                }
            }
        }
    }
    let gap = |a0: usize, a1: usize, b0: usize, b1: usize| if a1 <= b0 { b0 - a1 } else if b1 <= a0 { a0 - b1 } else { 0 };
    let meets = |b: &[usize; 4], r: &[usize; 4]| b[0] < r[2] && r[0] < b[2] && b[1] < r[3] && r[1] < b[3];
    let (qw, qh) = ((a[2] - a[0]) / 4, (a[3] - a[1]) / 4);
    let core = [a[0] + qw, a[1] + qh, a[2] - qw, a[3] - qh];
    let mut hs: Vec<usize> = (0..n).filter(|&i| comps[i].0 >= 20 && meets(&bb[i], &a)).map(|i| bb[i][3] - bb[i][1]).collect();
    hs.sort_unstable();
    let l = hs.get(hs.len() / 2).copied().unwrap_or(h / 100).max(1) as f64;
    let d = (dmul * l) as usize;
    // a book edge is a few long pieces; many big pieces touching a cut side are content running off it (a cover, f5 p1)
    // ponytail: a count, measured 26 on f5 p1 and at most 12 elsewhere on f1-f7; a cover with fewer stays whitened
    let big_edge = (0..n).filter(|&i| edge[i] && (bb[i][2] - bb[i][0]).max(bb[i][3] - bb[i][1]) as f64 > 3.0 * l).count();
    let runs_off = big_edge > KEEP_EDGE_MAX;
    let (ex0, ey0, ex1, ey1) = (a[0].saturating_sub(d), a[1].saturating_sub(d), a[2] + d, a[3] + d);
    let reason = |i: usize| -> Option<&'static str> {
        let b = &bb[i];
        let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
        if comps[i].0 == 0 {
            return None;
        }
        let frame = (comps[i].0 as f64) < KEEP_FRAME_FILL * (bw * bh) as f64 && (bw * bh) as f64 > 100.0 * l * l
            && !(b[0] >= ex0 && b[1] >= ey0 && b[2] <= ex1 && b[3] <= ey1);
        let rule = bh as f64 > KEEP_RULE_FRAC * h as f64 && bw * 8 < bh;
        let big = bw.max(bh) as f64 > 3.0 * l;
        if frame { Some("frame") } else if meets(b, &core) { Some("not joined") } else if edge[i] && big && !runs_off { Some("edge+big") } else if rule { Some("rule") } else { Some("not joined") }
    };
    let live: Vec<bool> = (0..n)
        .map(|i| {
            let b = &bb[i];
            let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
            // a big, sparse piece reaching out of the block is a book edge or frame (f7 p3's L), a table stays inside
            let frame = (comps[i].0 as f64) < KEEP_FRAME_FILL * (bw * bh) as f64 && (bw * bh) as f64 > 100.0 * l * l
                && !(b[0] >= ex0 && b[1] >= ey0 && b[2] <= ex1 && b[3] <= ey1);
            let rule = bh as f64 > KEEP_RULE_FRAC * h as f64 && bw * 8 < bh;
            // at a cut side only big pieces are junk: a page number can sit right on the pre-crop's edge (f1 p3, flerspaltet p9)
            let big = bw.max(bh) as f64 > 3.0 * l;
            comps[i].0 > 0 && !frame && (meets(b, &core) || !((edge[i] && big && !runs_off) || rule))
        })
        .collect();
    let mut kept = vec![false; n];
    let mut k = [usize::MAX, usize::MAX, 0, 0];
    let add = |i: usize, kept: &mut Vec<bool>, k: &mut [usize; 4]| {
        kept[i] = true;
        let b = &bb[i];
        *k = [k[0].min(b[0]), k[1].min(b[1]), k[2].max(b[2]), k[3].max(b[3])];
    };
    for i in 0..n {
        if live[i] && meets(&bb[i], &a) {
            add(i, &mut kept, &mut k);
        }
    }
    if k[2] == 0 {
        return (a, runs_off, (l, a, a[1], a[3])); // nothing in the block (blank slot): keep the block's box, whiten nothing
    }
    // pass 0 grows the kept box; pass 1 adds head-line letters once and does not iterate, or a rule beside a page number
    // would be pulled in by the enlarged box (f1 p5)
    for head_pass in [false, true] {
    loop {
        let mut grew = false;
        // top and bottom of the kept text lines: a dash or speck above the head must not move the head line
        let kl = (0..n).filter(|&j| kept[j] && (bb[j][3] - bb[j][1]) as f64 >= l / 2.0);
        let (top, bot) = kl.fold((usize::MAX, 0), |(t, u), j| (t.min(bb[j][1]), u.max(bb[j][3])));
        for i in 0..n {
            let b = bb[i];
            let (gx, gy) = (gap(b[0], b[2], k[0], k[2]), gap(b[1], b[3], k[1], k[3]));
            // above/below the kept box and within its columns (page number, running head, footnote): any distance
            // if it is more than a speck; beside it (where rules and edges are): only near. In the header/footer
            // zone (above or below the text block) a letter may also sit further out sideways: a page number beside
            // the head (f1 p3) or centred under a one-column last page (flerspaltet p9: 62 px beside, 886 px below).
            let letter = comps[i].0 as f64 >= l * l / 8.0 && ((b[2] - b[0]).max(b[3] - b[1]) as f64) <= 3.0 * l;
            let zone = b[3] <= a[1] || b[1] >= a[3];
            // the block's top/bottom kept line (the running head): a letter on it may sit KEEP_HEAD_D letters out sideways
            // (f1 p3 "14"), unless it is one of a dotted rule: >= 3 other narrow pieces in its columns (rule segments too) spread over KEEP_RULE_FRAC of h
            let headline = head_pass && letter && (b[3] - b[1]) as f64 >= l / 2.0 && gx as f64 <= KEEP_HEAD_D * l && ((b[1] as f64) < top as f64 + 1.5 * l || b[3] as f64 > bot as f64 - 1.5 * l)
                && {
                    let st: Vec<usize> = (0..n).filter(|&j| j != i && bb[j][0] < b[2] && b[0] < bb[j][2]
                        && comps[j].0 as f64 >= l * l / 8.0 && (bb[j][2] - bb[j][0]) as f64 <= 3.0 * l).collect();
                    let (lo, hi) = st.iter().fold((b[1], b[3]), |(lo, hi), &j| (lo.min(bb[j][1]), hi.max(bb[j][3])));
                    st.len() < 3 || (hi - lo) as f64 <= KEEP_RULE_FRAC * h as f64
                };
            if live[i] && !kept[i]
                && if head_pass { headline } else { (gx == 0 && comps[i].0 as f64 >= l * l / 8.0) || (gx <= d && gy <= d) || (zone && letter && gx as f64 <= KEEP_ZONE_D * l) }
            {
                add(i, &mut kept, &mut k);
                grew = true;
            }
        }
        if !grew || head_pass {
            break;
        }
    }
    }
    if let Some((page, slot)) = log {
        // TSV: page slot x0 y0 x1 y1 pixels letters(=longer side / l) outside(1 = not inside the text block) reason
        for i in (0..n).filter(|&i| !kept[i]) {
            if let Some(r) = reason(i) {
                let b = bb[i];
                let out = !(b[0] >= a[0] && b[1] >= a[1] && b[2] <= a[2] && b[3] <= a[3]);
                eprintln!("whitened\t{page}\t{slot}\t{}\t{}\t{}\t{}\t{}\t{:.2}\t{}\t{r}\t{h}", b[0], b[1], b[2], b[3], comps[i].0, (b[2] - b[0]).max(b[3] - b[1]) as f64 / l, out as u8);
            }
        }
    }
    for i in 0..w * h {
        if bits[i] && !kept[lab[i] as usize] {
            bits[i] = false;
        }
    }
    let (px, py) = ((CROP_PAD_FRAC * w as f64) as usize, (CROP_PAD_FRAC * h as f64) as usize);
    let kl = (0..n).filter(|&j| kept[j] && (bb[j][3] - bb[j][1]) as f64 >= l / 2.0);
    let (top, bot) = kl.fold((usize::MAX, 0), |(t, u), j| (t.min(bb[j][1]), u.max(bb[j][3])));
    ([k[0].saturating_sub(px), k[1].saturating_sub(py), (k[2] + px).min(w), (k[3] + py).min(h)], runs_off, (l, k, top, bot))
}

/// #84: page numbers beyond the plan box. `rb` is the slot's ink thresholded again on a canvas grown by r each side, over the
/// slot grown by r on its cut sides. Candidates: components with a pixel outside the slot, letter-sized, not within 2 px of a cut
/// side of the grown box, on the header/footer line (keep_box's headline or zone test; beside the kept box also its dotted-rule guard). They form
/// clusters (same line, gaps <= 0.3 letters); a cluster is added when it looks like one page number: at most 4 pieces, one of
/// them anchored in ink main kept, KEEP_REACH_ISO letters clear of other candidates (a clipped running head is a row of them),
/// tallest piece within KEEP_REACH_H, at least half a letter wide (not an edge sliver). Added pixels are set in `bits` (main
/// canvas); returns the added box in main canvas coordinates.
// ponytail: measured on f1-f7 + tests/real (6 clusters, all page numbers); a number with no digit reaching into the slot is not found
#[allow(clippy::too_many_arguments)]
fn reach(rb: &[bool], rw: usize, rh: usize, r: usize, sw: usize, sh: usize, (ccx, ccy): (f32, f32), (gcx, gcy): (f32, f32), (sn, cs): (f32, f32),
    grow: [bool; 4], cut: [bool; 4], a: [usize; 4], (l, k, top, bot): (f64, [usize; 4], usize, usize), bits: &mut [bool], mw: usize, mh: usize,
    log: Option<(usize, usize)>) -> Option<[i64; 4]> {
    let (lab, comps) = components(rb, rw, rh);
    let n = comps.len();
    let mut bb = vec![[i64::MAX, i64::MAX, i64::MIN, i64::MIN]; n]; // main canvas coords
    let (mut out, mut edge, mut seed) = (vec![false; n], vec![false; n], vec![false; n]);
    let rf = r as f32;
    let lim = |g: bool| if g { rf } else { 0.0 };
    for y in 0..rh {
        for x in 0..rw {
            if !rb[y * rw + x] {
                continue;
            }
            let i = lab[y * rw + x] as usize;
            let (mx, my) = (x as i64 - r as i64, y as i64 - r as i64);
            let b = &mut bb[i];
            *b = [b[0].min(mx), b[1].min(my), b[2].max(mx + 1), b[3].max(my + 1)];
            // same mapping as near_edge in finish: canvas pixel -> source slot pixel
            let (u, v) = (mx as f32 + 0.5 - ccx, my as f32 + 0.5 - ccy);
            let (sx, sy) = (gcx + u * cs - v * sn - 0.5, gcy + u * sn + v * cs - 0.5);
            if sx < -0.5 || sy < -0.5 || sx > sw as f32 - 0.5 || sy > sh as f32 - 0.5 {
                out[i] = true;
            } else if mx >= 0 && my >= 0 && (mx as usize) < mw && (my as usize) < mh && bits[my as usize * mw + mx as usize] {
                seed[i] = true;
            }
            if (cut[0] && sx < 2.0 - lim(grow[0])) || (cut[1] && sy < 2.0 - lim(grow[1]))
                || (cut[2] && sx > sw as f32 - 3.0 + lim(grow[2])) || (cut[3] && sy > sh as f32 - 3.0 + lim(grow[3])) {
                edge[i] = true;
            }
        }
    }
    let gap = |a0: i64, a1: i64, b0: i64, b1: i64| if a1 <= b0 { b0 - a1 } else if b1 <= a0 { a0 - b1 } else { 0 };
    let (k, a) = (k.map(|v| v as i64), a.map(|v| v as i64));
    let small = |j: usize| comps[j].0 as f64 >= l * l / 8.0 && (bb[j][2] - bb[j][0]) as f64 <= 3.0 * l;
    // ring letters on the header/footer line; a candidate is one that is also not part of a dotted rule
    let online: Vec<usize> = (0..n)
        .filter(|&i| {
            let b = bb[i];
            let bh = b[3] - b[1];
            let letter = comps[i].0 as f64 >= l * l / 8.0 && ((b[2] - b[0]).max(bh) as f64) <= 3.0 * l;
            let gx = gap(b[0], b[2], k[0], k[2]);
            let headline = bh as f64 >= l / 2.0 && gx as f64 <= KEEP_HEAD_D * l && ((b[1] as f64) < top as f64 + 1.5 * l || b[3] as f64 > bot as f64 - 1.5 * l);
            let zone = (b[3] <= a[1] || b[1] >= a[3]) && gx as f64 <= KEEP_ZONE_D * l;
            comps[i].0 > 0 && out[i] && !edge[i] && letter && (headline || zone)
        })
        .collect();
    let cand: Vec<usize> = online
        .iter()
        .copied()
        .filter(|&i| {
            let b = bb[i];
            // beside the kept box, not one of a dotted rule (as in keep_box); above/below it its columns hold the text
            gap(b[0], b[2], k[0], k[2]) == 0 || {
                let st: Vec<usize> = (0..n).filter(|&j| j != i && comps[j].0 > 0 && bb[j][0] < b[2] && b[0] < bb[j][2] && small(j)).collect();
                let (lo, hi) = st.iter().fold((b[1], b[3]), |(lo, hi), &j| (lo.min(bb[j][1]), hi.max(bb[j][3])));
                st.len() < 3 || (hi - lo) as f64 <= KEEP_RULE_FRAC * mh as f64
            }
        })
        .collect();
    // clusters: same line (half the smaller height overlaps), gap <= 0.3 letters
    let mut root: Vec<usize> = (0..cand.len()).collect();
    fn find(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            i = p[i];
        }
        i
    }
    for p in 0..cand.len() {
        for q in p + 1..cand.len() {
            let (s, t) = (bb[cand[p]], bb[cand[q]]);
            if gap(s[0], s[2], t[0], t[2]) as f64 <= 0.3 * l && (s[3].min(t[3]) - s[1].max(t[1])) * 2 >= (s[3] - s[1]).min(t[3] - t[1]) {
                let (rp, rq) = (find(&mut root, p), find(&mut root, q));
                root[rp] = rq;
            }
        }
    }
    let mut added: Option<[i64; 4]> = None;
    for c in 0..cand.len() {
        if find(&mut root, c) != c {
            continue;
        }
        let m: Vec<usize> = (0..cand.len()).filter(|&p| find(&mut root, p) == c).map(|p| cand[p]).collect();
        let e = m.iter().fold([i64::MAX, i64::MAX, i64::MIN, i64::MIN], |e, &i| [e[0].min(bb[i][0]), e[1].min(bb[i][1]), e[2].max(bb[i][2]), e[3].max(bb[i][3])]);
        let tall = m.iter().map(|&i| bb[i][3] - bb[i][1]).max().unwrap() as f64 / l;
        let iso = online.iter().filter(|j| !m.contains(j)).map(|&j| m.iter().map(|&i| gap(bb[i][0], bb[i][2], bb[j][0], bb[j][2]).max(gap(bb[i][1], bb[i][3], bb[j][1], bb[j][3]))).min().unwrap()).min();
        if m.len() > 4 || !m.iter().any(|&i| seed[i]) || iso.is_some_and(|d| (d as f64) < KEEP_REACH_ISO * l)
            || tall < KEEP_REACH_H.0 || tall > KEEP_REACH_H.1 || ((e[2] - e[0]) as f64) < 0.5 * l {
            continue;
        }
        if let Some((page, slot)) = log {
            eprintln!("reached\t{page}\t{slot}\t{}\t{}\t{}\t{}\t{}\t{tall:.2}", e[0], e[1], e[2], e[3], m.len());
        }
        added = Some(added.map_or(e, |f| [f[0].min(e[0]), f[1].min(e[1]), f[2].max(e[2]), f[3].max(e[3])]));
        for y in 0..rh {
            for x in 0..rw {
                let (mx, my) = (x as i64 - r as i64, y as i64 - r as i64);
                if rb[y * rw + x] && m.contains(&(lab[y * rw + x] as usize)) && mx >= 0 && my >= 0 && (mx as usize) < mw && (my as usize) < mh {
                    bits[my as usize * mw + mx as usize] = true;
                }
            }
        }
    }
    // #87: clipped text lines. Pool: ring pieces out of the slot, not at the grown box's cut, at most 3 letters big; two are
    // linked when they share rows and stand <= KEEP_LINE_GAP letters apart sideways. A linked component counts when it holds a
    // seed (a candidate anchored in ink main kept); seeded components whose row bands overlap by half the smaller height are one
    // text line (f1 p24's clipped ascenders join only through letters main kept). A line is added when >= KEEP_LINE_MIN letter
    // pieces carry >= KEEP_LINE_INK of its ink (not speckle). Then small pieces over/under it (i dots, the ring of a) join.
    let pool: Vec<usize> = (0..n).filter(|&i| comps[i].0 > 0 && out[i] && !edge[i] && ((bb[i][2] - bb[i][0]).max(bb[i][3] - bb[i][1]) as f64) <= 3.0 * l).collect();
    let mut grp: Vec<usize> = (0..n).collect();
    for (x, &p) in pool.iter().enumerate() {
        for &q in &pool[x + 1..] {
            let (s, t) = (bb[p], bb[q]);
            if gap(s[0], s[2], t[0], t[2]) as f64 <= KEEP_LINE_GAP * l && s[3].min(t[3]) > s[1].max(t[1]) {
                let (rp, rq) = (find(&mut grp, p), find(&mut grp, q));
                grp[rp] = rq;
            }
        }
    }
    let seeded: Vec<usize> = cand.iter().copied().filter(|&i| seed[i] && pool.contains(&i)).map(|i| find(&mut grp, i)).collect();
    let root: Vec<usize> = (0..n).map(|i| find(&mut grp, i)).collect();
    let mut grp: Vec<usize> = (0..n).map(|i| if pool.contains(&i) && seeded.contains(&root[i]) { root[i] } else { usize::MAX }).collect();
    let mut band: Vec<(usize, i64, i64)> = vec![]; // (component, top row, bottom row)
    for &i in pool.iter().filter(|&&i| grp[i] != usize::MAX) {
        match band.iter_mut().find(|b| b.0 == grp[i]) {
            Some(b) => { b.1 = b.1.min(bb[i][1]); b.2 = b.2.max(bb[i][3]); }
            None => band.push((grp[i], bb[i][1], bb[i][3])),
        }
    }
    let mut lp: Vec<usize> = (0..band.len()).collect();
    for x in 0..band.len() {
        for y in x + 1..band.len() {
            let (s, t) = (band[x], band[y]);
            if (s.2.min(t.2) - s.1.max(t.1)) * 2 >= (s.2 - s.1).min(t.2 - t.1) {
                let (rx, ry) = (find(&mut lp, x), find(&mut lp, y));
                lp[rx] = ry;
            }
        }
    }
    for g in grp.iter_mut() {
        if let Some(x) = band.iter().position(|b| b.0 == *g) {
            *g = band[find(&mut lp, x)].0;
        }
    }
    let mut on = vec![false; n];
    let mut lines: Vec<usize> = pool.iter().map(|&i| grp[i]).filter(|&g| g != usize::MAX).collect();
    lines.sort();
    lines.dedup();
    for line in lines {
        let m: Vec<usize> = pool.iter().copied().filter(|&i| grp[i] == line).collect();
        let letters: Vec<usize> = m.iter().copied().filter(|&i| comps[i].0 as f64 >= l * l / 8.0 && (bb[i][3] - bb[i][1]) as f64 >= l / 2.0).collect();
        let ink = |v: &[usize]| v.iter().map(|&i| comps[i].0 as f64).sum::<f64>();
        if letters.len() < KEEP_LINE_MIN || ink(&letters) < KEEP_LINE_INK * ink(&m) {
            continue;
        }
        for &i in &m {
            on[i] = true;
        }
        if let Some((page, slot)) = log {
            // class: white rows between the line and the next kept ink towards the block, after its own kept ink
            let e = m.iter().fold([i64::MAX, i64::MAX, i64::MIN, i64::MIN], |e, &i| [e[0].min(bb[i][0]), e[1].min(bb[i][1]), e[2].max(bb[i][2]), e[3].max(bb[i][3])]);
            let down = (e[1] + e[3]) / 2 < (k[1] + k[3]) / 2;
            let row = |y: i64| y >= 0 && (y as usize) < mh && (0..mw).any(|x| bits[y as usize * mw + x]);
            let mut y = if down { e[3] } else { e[1] - 1 };
            while row(y) {
                y += if down { 1 } else { -1 };
            }
            let y0 = y;
            while y >= 0 && (y as usize) < mh && !row(y) {
                y += if down { 1 } else { -1 };
            }
            let white = (y - y0).abs() as f64 / l;
            let class = if white < KEEP_LINE_CLASS.0 { "body" } else if white < KEEP_LINE_CLASS.1 { "heading" } else { "headfoot" };
            // an edge piece on the line within the sideways gap: the line runs past the ring
            let cut = (0..n).any(|j| comps[j].0 > 0 && out[j] && edge[j] && bb[j][3].min(e[3]) > bb[j][1].max(e[1]) && gap(bb[j][0], bb[j][2], e[0], e[2]) as f64 <= KEEP_LINE_GAP * l);
            eprintln!("line\t{page}\t{slot}\t{}\t{}\t{}\t{}\t{class}\t{white:.2}\t{}", e[0], e[1], e[2], e[3], if cut { "cut" } else { "" });
        }
    }
    let dia: Vec<usize> = pool.iter().copied().filter(|&p| !on[p] && ((bb[p][3] - bb[p][1]) as f64) <= KEEP_LINE_DIA * l
        && pool.iter().any(|&q| on[q] && bb[p][0] < bb[q][2] && bb[q][0] < bb[p][2] && gap(bb[p][1], bb[p][3], bb[q][1], bb[q][3]) as f64 <= KEEP_LINE_DIA * l)).collect();
    for p in dia {
        on[p] = true;
    }
    for y in 0..rh {
        for x in 0..rw {
            let (mx, my) = (x as i64 - r as i64, y as i64 - r as i64);
            if rb[y * rw + x] && on[lab[y * rw + x] as usize] && mx >= 0 && my >= 0 && (mx as usize) < mw && (my as usize) < mh {
                bits[my as usize * mw + mx as usize] = true;
                let e = [mx, my, mx + 1, my + 1];
                added = Some(added.map_or(e, |f| [f[0].min(e[0]), f[1].min(e[1]), f[2].max(e[2]), f[3].max(e[3])]));
            }
        }
    }
    added
}

/// --crop (#55): packed 1-bit ink boxes with their text-block anchor -> one canvas; every anchor lands on the
/// same centre x and top y, the canvas is just big enough for all of them plus a margin, then G4. Pure bit placement.
// ponytail: one slot with ink far from its text block (an edge band that was not whitened) widens every page
fn compose(slots: Vec<(u32, u32, Vec<u8>, [usize; 2])>) -> Vec<(u32, u32, Vec<u8>)> {
    let ext = |f: &dyn Fn(usize, usize, [usize; 2]) -> usize| slots.iter().map(|s| f(s.0 as usize, s.1 as usize, s.3)).max().unwrap_or(0);
    let half = ext(&|w, _, a| a[0].max(w - a[0]));
    let (top, bot) = (ext(&|_, _, a| a[1]), ext(&|_, h, a| h - a[1]));
    let (mx, my) = ((CROP_PAD_FRAC * 2.0 * half as f64) as usize, (CROP_PAD_FRAC * (top + bot) as f64) as usize);
    let (cw, ch) = (2 * half + 2 * mx, top + bot + 2 * my);
    let place = |(w, h, p, a): &(u32, u32, Vec<u8>, [usize; 2])| {
        let (w, h) = (*w as usize, *h as usize);
        let (ox, oy, stride) = (mx + half - a[0], my + top - a[1], w.div_ceil(8));
        let mut bits = vec![false; cw * ch];
        for y in 0..h {
            for x in 0..w {
                bits[(oy + y) * cw + ox + x] = p[y * stride + x / 8] & (0x80 >> (x % 8)) != 0;
            }
        }
        (cw as u32, ch as u32, encode_g4(&bits, cw as u32))
    };
    let k = slots.len().div_ceil(std::thread::available_parallelism().map_or(4, |n| n.get()));
    std::thread::scope(|s| {
        let hs: Vec<_> = slots.chunks(k.max(1)).map(|c| s.spawn(move || c.iter().map(place).collect::<Vec<_>>())).collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    })
}

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}

/// The page as 8-bit gray at `dpi`, pixel size rounded up like pdftoppm. A page that is nothing
/// but one raster image filling the page at (nearly) this resolution is decoded directly;
/// anything else is rendered.
fn page_gray(page: &Page, dpi: f32, smooth: bool) -> (Gray, &'static str) {
    let (pw, ph) = page.render_dimensions();
    let s = dpi / 72.0;
    let (w, h) = ((pw * s).ceil() as u32, (ph * s).ceil() as u32);
    let init = Affine::scale_non_uniform(w as f64 / pw as f64, h as f64 / ph as f64) * page.initial_transform(true).to_kurbo();

    let settings = InterpreterSettings::default();
    let mut probe = Probe { w, h, smooth, other: false, images: 0, got: None };
    let ic = InterpreterCache::new();
    let mut ctx = Context::new(init, Rect::new(0.0, 0.0, w as f64, h as f64), &ic, page.xref(), settings.clone());
    interpret_page(page, &mut ctx, &mut probe);
    if let (false, 1, Some(g), None) = (probe.other, probe.images, probe.got, std::env::var_os("BITONAL_RENDER")) {
        return (g, "image");
    }

    let rs = RenderSettings { x_scale: s, y_scale: s, width: Some(w as u16), height: Some(h as u16), bg_color: WHITE };
    let pix = render(page, &RenderCache::new(), &settings, &rs);
    // Background is opaque white, so premultiplied == straight RGB. Rec.601 luma like poppler's -gray.
    let px = pix.data().iter().map(|p| ((p.r as u32 * 299 + p.g as u32 * 587 + p.b as u32 * 114 + 500) / 1000) as u8).collect();
    (Gray { w, h, px }, "render")
}

/// An 8-bit RGB raster, 3 bytes per pixel.
// ponytail: unused until B4e (#76) picks the colour pages. Always rendered, no Probe shortcut: a source JPEG would be re-encoded anyway.
#[allow(dead_code)]
struct Rgb {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

/// The page rendered as RGB at `dpi`, same pixel size as `page_gray`. On demand: the text path never calls it.
#[allow(dead_code)]
fn page_rgb(page: &Page, dpi: f32) -> Rgb {
    let (pw, ph) = page.render_dimensions();
    let s = dpi / 72.0;
    let (w, h) = ((pw * s).ceil() as u32, (ph * s).ceil() as u32);
    let rs = RenderSettings { x_scale: s, y_scale: s, width: Some(w as u16), height: Some(h as u16), bg_color: WHITE };
    let pix = render(page, &RenderCache::new(), &InterpreterSettings::default(), &rs);
    // Opaque white background, so premultiplied == straight RGB.
    Rgb { w, h, px: pix.data().iter().flat_map(|p| [p.r, p.g, p.b]).collect() }
}

/// Device that only checks whether the page is exactly one unrotated raster image covering the
/// page within 1 % of the target size, and decodes it if so.
struct Probe {
    w: u32,
    h: u32,
    smooth: bool,
    other: bool,
    images: u32,
    got: Option<Gray>,
}

impl<'a> Device<'a> for Probe {
    fn set_soft_mask(&mut self, m: Option<SoftMask<'a>>) {
        self.other |= m.is_some();
    }
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {
        self.other = true;
    }
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(&mut self, _: &Glyph<'a>, _: Affine, _: Affine, _: &Paint<'a>, m: &GlyphDrawMode) {
        // ponytail: invisible OCR text (Tr 3) on a scan still counts as "other"; allow it when a real OCR'd scan needs it
        let _ = m;
        self.other = true;
    }
    fn draw_image(&mut self, image: Image<'a, '_>, t: Affine) {
        self.images += 1;
        let Image::Raster(r) = image else { return self.other = true };
        // t maps image pixels (y down) to device pixels: must be axis-aligned (any quarter turn or
        // flip), scale ~1, and cover the whole page.
        let [a, b, c, d, _, _] = t.as_coeffs();
        let (iw, ih) = (r.width() as f64, r.height() as f64);
        let one = |x: f64| (x.abs() - 1.0).abs() <= 0.01;
        let aligned = (b.abs() < 1e-6 && c.abs() < 1e-6 && one(a) && one(d))
            || (a.abs() < 1e-6 && d.abs() < 1e-6 && one(b) && one(c));
        let bb = t.transform_rect_bbox(kurbo::Rect::new(0.0, 0.0, iw, ih));
        let tol = 0.01 * self.w.max(self.h) as f64;
        let covers = bb.x0.abs() <= tol && bb.y0.abs() <= tol
            && (bb.x1 - self.w as f64).abs() <= tol && (bb.y1 - self.h as f64).abs() <= tol;
        let fits = aligned && covers;
        if !fits || self.images > 1 {
            return;
        }
        r.with_rgba(
            |img, alpha| {
                if alpha.is_some() {
                    return;
                }
                let (w, h) = (img.width(), img.height());
                if (w, h) != (r.width(), r.height()) {
                    return; // decoder corrected the size: let the renderer handle it
                }
                let px = match img {
                    ImageData::Luma(l) => l.data,
                    ImageData::Rgb(c) => c.data.chunks_exact(3)
                        .map(|p| ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114 + 500) / 1000) as u8)
                        .collect(),
                };
                let binary = px.iter().all(|&v| v == 0 || v == 255);
                let mut g = to_page(&Gray { w, h, px }, t, self.w, self.h);
                // A 1-bit scan a few px off the page size: poppler resamples it with interpolation, which turns a
                // dithered scanner band into grey that the flatten removes; 1:1 it survived as dots (#28, #29).
                // Output image only: measured on the smoothed copy, sidste side p4 lost its page numbers (band rule).
                if self.smooth && binary && (w, h) != (self.w, self.h) && (h, w) != (self.w, self.h) {
                    let f: Vec<f32> = g.px.iter().map(|&v| v as f32).collect();
                    g.px = gauss_exact(&f, g.w as usize, g.h as usize, 0.8).iter().map(|&v| v.round() as u8).collect();
                }
                self.got = Some(g);
            },
            None,
        );
    }
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// Nearest-neighbour map of the image onto the w x h page through t (image px -> page px).
/// For the transforms Probe accepts this is a pure rotation/flip, plus dropping or doubling at
/// most 1 % of rows/columns when the image is not exactly at the target size.
fn to_page(g: &Gray, t: Affine, w: u32, h: u32) -> Gray {
    let inv = t.inverse();
    let mut px = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let p = inv * kurbo::Point::new(x as f64 + 0.5, y as f64 + 0.5);
            let (u, v) = ((p.x.floor() as i64).clamp(0, g.w as i64 - 1), (p.y.floor() as i64).clamp(0, g.h as i64 - 1));
            px.push(g.px[v as usize * g.w as usize + u as usize]);
        }
    }
    Gray { w, h, px }
}

/// Divide by a blurred copy (background flattening), explicitly: no library compose semantics (#22).
fn flatten(g: &Img, sigma: f32) -> Img {
    let blur = gauss(&g.px, g.w, g.h, sigma);
    let px = g.px.iter().zip(&blur).map(|(s, b)| if *b <= 0.0 { 1.0 } else { (s / b).min(1.0) }).collect();
    Img { w: g.w, h: g.h, px }
}

/// 1 = ink. The #20 hysteresis rule on a flattened image: ink if < thresh, or < 75 % with a pixel < 45 %
/// within radius 2.
fn hyst(flat: &Img, thresh: f32) -> Vec<bool> {
    let (w, h) = (flat.w, flat.h);
    let (t, weak, seed) = (thresh / 100.0, 0.75f32.max(thresh / 100.0), 0.45);
    let seeds: Vec<bool> = flat.px.iter().map(|&v| v < seed).collect();
    let r = 2isize;
    let mut out = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let v = flat.px[y * w + x];
            out[y * w + x] = v < t
                || (v < weak
                    && (-r..=r).any(|dy| (-r..=r).any(|dx| {
                        let (yy, xx) = (y as isize + dy, x as isize + dx);
                        dx * dx + dy * dy <= r * r + 1 // ImageMagick Disk:2 includes the (±1,±2) pixels
                            && yy >= 0 && xx >= 0 && (yy as usize) < h && (xx as usize) < w
                            && seeds[yy as usize * w + xx as usize]
                    })));
        }
    }
    out
}

/// Area-average downsample by an integer factor (edge remainder dropped).
fn downsample(g: &Img, k: usize) -> Img {
    if k == 1 {
        return g.clone();
    }
    let (w, h) = (g.w / k, g.h / k);
    let mut px = vec![0.0; w * h];
    for y in 0..h * k {
        for x in 0..w * k {
            px[(y / k) * w + x / k] += g.px[y * g.w + x];
        }
    }
    let n = (k * k) as f32;
    px.iter_mut().for_each(|v| *v /= n);
    Img { w, h, px }
}

/// Quarter turn clockwise, `deg` in 0/90/180/270 (ImageMagick -rotate).
fn rot90(g: &Img, deg: u32) -> Img {
    let (w, h) = (g.w, g.h);
    match deg {
        90 => Img { w: h, h: w, px: (0..w * h).map(|i| { let (x, y) = (i % h, i / h); g.px[(h - 1 - x) * w + y] }).collect() },
        180 => Img { w, h, px: g.px.iter().rev().copied().collect() },
        270 => Img { w: h, h: w, px: (0..w * h).map(|i| { let (x, y) = (i % h, i / h); g.px[x * w + (w - 1 - y)] }).collect() },
        _ => g.clone(),
    }
}

/// Rotate g by `deg` (positive = the content turns counter-clockwise, undoing a clockwise skew of that angle)
/// about its centre and place it centred on a cw x ch canvas of `fill`, bilinear. deg 0 = exact integer shift.
fn rotate(g: &Img, deg: f32, cw: usize, ch: usize, fill: f32) -> Img {
    let (ox, oy) = ((cw as i64 - g.w as i64) / 2, (ch as i64 - g.h as i64) / 2);
    let mut px = vec![fill; cw * ch];
    if deg.abs() < 0.01 {
        for y in 0..g.h as i64 {
            let yy = y + oy;
            if yy < 0 || yy >= ch as i64 {
                continue;
            }
            for x in 0..g.w as i64 {
                let xx = x + ox;
                if xx >= 0 && xx < cw as i64 {
                    px[yy as usize * cw + xx as usize] = g.px[y as usize * g.w + x as usize];
                }
            }
        }
        return Img { w: cw, h: ch, px };
    }
    let (s, c) = deg.to_radians().sin_cos();
    let (gcx, gcy) = (g.w as f32 / 2.0, g.h as f32 / 2.0);
    let (ccx, ccy) = (ox as f32 + gcx, oy as f32 + gcy);
    let at = |x: i64, y: i64| if x < 0 || y < 0 || x >= g.w as i64 || y >= g.h as i64 { fill } else { g.px[y as usize * g.w + x as usize] };
    for y in 0..ch {
        for x in 0..cw {
            let (u, v) = (x as f32 + 0.5 - ccx, y as f32 + 0.5 - ccy);
            let (sx, sy) = (gcx + u * c - v * s - 0.5, gcy + u * s + v * c - 0.5);
            let (x0, y0) = (sx.floor(), sy.floor());
            let (fx, fy) = (sx - x0, sy - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            px[y * cw + x] = (at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx) * (1.0 - fy)
                + (at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx) * fy;
        }
    }
    Img { w: cw, h: ch, px }
}

/// A spine shadow, book edge or the rules next to it (#53) is one connected piece running most of the slot height,
/// often slanted or bowed; text never is. Whiten such pieces in the outer zone of the slot.
// ponytail: whitens the piece only; a crop or an alignment step (#55) would also move the text
fn clear_edge_bands(bits: &mut [bool], w: usize, h: usize) {
    let r = BAND_CLOSE;
    // close pinholes: dilate by r, sideways then down
    let mut d = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            d[y * w + x] = bits[y * w + x.saturating_sub(r)..(x + r + 1).min(w) + y * w].contains(&true);
        }
    }
    let mut c = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            c[y * w + x] = (y.saturating_sub(r)..(y + r + 1).min(h)).any(|k| d[k * w + x]);
        }
    }
    let (lab, comps) = components(&c, w, h);
    // bounding box per root: x0, x1, y0, y1
    let mut bb = vec![[usize::MAX, 0, usize::MAX, 0]; comps.len()];
    for y in 0..h {
        for x in 0..w {
            if c[y * w + x] {
                let b = &mut bb[lab[y * w + x] as usize];
                *b = [b[0].min(x), b[1].max(x), b[2].min(y), b[3].max(y)];
            }
        }
    }
    let (wf, hf) = (w as f64, h as f64);
    let hit: Vec<bool> = bb
        .iter()
        .zip(&comps)
        .map(|(b, k)| {
            k.0 > 0 && (b[3] - b[2] + 1) as f64 >= BAND_MIN_HEIGHT_FRAC * hf && ((b[1] - b[0] + 1) as f64) < BAND_MAX_WIDTH_FRAC * wf
                && ((b[1] as f64) < BAND_ZONE_FRAC * wf || (b[0] as f64) > (1.0 - BAND_ZONE_FRAC) * wf)
        })
        .collect();
    for i in 0..w * h {
        if bits[i] && c[i] && hit[lab[i] as usize] {
            bits[i] = false;
        }
    }
}

/// Sub-image [x0,x1) x [y0,y1), clamped to the image.
fn crop(g: &Img, [x0, y0, x1, y1]: [i64; 4]) -> Img {
    let cl = |v: i64, m: usize| v.clamp(0, m as i64) as usize;
    let (x0, x1, y0, y1) = (cl(x0, g.w), cl(x1, g.w), cl(y0, g.h), cl(y1, g.h));
    let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
    Img { w, h, px: (y0..y0 + h).flat_map(|y| g.px[y * g.w + x0..y * g.w + x0 + w].iter().copied()).collect() }
}

/// Skew of the text lines in degrees (positive = lines fall to the right), projection-profile method
/// (Postl): the angle whose row projection of the ink is sharpest (largest sum of squared differences between
/// neighbouring rows; plain squared counts plateau over ~0.1° for thick lines).
/// Coarse 0.25° steps over ±SKEW_MAX_DEG, then 0.025° around the best.
// ponytail: O(ink pixels x angles), ~80+20 angles; subsample the ink if big pages at high dpi get slow
fn skew_angle(ink: &[bool], w: usize, h: usize) -> f32 {
    let cx = w as f32 / 2.0;
    let pts: Vec<(f32, f32)> = (0..w * h).filter(|&i| ink[i]).map(|i| ((i % w) as f32 - cx, (i / w) as f32)).collect();
    if pts.len() < 100 {
        return 0.0;
    }
    let off = (cx * SKEW_MAX_DEG.to_radians().tan()).ceil() + 1.0;
    let mut bins = vec![0u32; h + 2 * off as usize + 2];
    let mut score = |deg: f32| {
        let t = deg.to_radians().tan();
        bins.iter_mut().for_each(|b| *b = 0);
        for &(x, y) in &pts {
            bins[(y - x * t + off) as usize] += 1;
        }
        bins.windows(2).map(|p| (p[1] as f64 - p[0] as f64).powi(2)).sum::<f64>()
    };
    // Middle of the best plateau: near the true angle the bins only change once a line end moves a whole pixel.
    let best = |score: &mut dyn FnMut(f32) -> f64, lo: f32, step: f32, n: i32| {
        let s: Vec<(f32, f64)> = (0..=n).map(|k| lo + k as f32 * step).map(|a| (a, score(a))).collect();
        let m = s.iter().map(|p| p.1).fold(f64::MIN, f64::max);
        let top: Vec<f32> = s.iter().filter(|p| p.1 == m).map(|p| p.0).collect();
        (top[0] + top[top.len() - 1]) / 2.0
    };
    let a = best(&mut score, -SKEW_MAX_DEG, 0.25, (2.0 * SKEW_MAX_DEG / 0.25) as i32);
    best(&mut score, a - 0.25, 0.025, 20)
}

/// Quantise a mean to 8 bit like ImageMagick's `-depth 8` does before the awk sees it.
fn q8(v: f64) -> f64 {
    (v * 255.0).round() / 255.0
}

/// Text-block bounds [x0, y0, x1, y1) of an ink map: rows first, then columns inside the row range
/// (port of bitonalpdf.sh content_box, see docs/lessons.md 1, 4, 6, 6b).
fn content_box(ink: &[bool], w: usize, h: usize, band: bool, px_per_mm: f64) -> [usize; 4] {
    let rows: Vec<f64> = (0..h).map(|y| q8(ink[y * w..(y + 1) * w].iter().filter(|&&b| b).count() as f64 / w as f64)).collect();
    let (max_h, seg) = ((BAND_TEXT_MAX_H_MM * px_per_mm).round() as i64, (BAND_TEXT_SEG_MM * px_per_mm).round() as usize);
    // #91: rows s0..=e1 look like text: short, and little of the ink in long horizontal segments
    let text = |s0: i64, e1: i64| {
        if e1 - s0 + 1 > max_h {
            return false;
        }
        let (mut ink_px, mut long_px) = (0usize, 0usize);
        for y in s0 as usize..=e1 as usize {
            let mut run = 0;
            for x in 0..=w {
                if x < w && ink[y * w + x] {
                    run += 1;
                    ink_px += 1;
                } else if run > 0 {
                    if run >= seg {
                        long_px += run;
                    }
                    run = 0;
                }
            }
        }
        ink_px > 0 && (long_px as f64) <= BAND_TEXT_LONG * ink_px as f64
    };
    let (y0, ht) = axis_box(&rows, band, &text);
    let sc = ht as f64 / h as f64;
    let mut cnt = vec![0usize; w];
    for y in y0..y0 + ht {
        for x in 0..w {
            cnt[x] += ink[y * w + x] as usize;
        }
    }
    let cols: Vec<f64> = cnt.iter().map(|&c| q8(c as f64 / ht as f64) * sc).collect();
    let (x0, wd) = axis_box(&cols, band, &|_, _| false);
    [x0, y0, x0 + wd, y0 + ht]
}

/// One axis of content_box: (first, length). Literal port of the awk, including its integer truncations.
fn axis_box(dens: &[f64], band_rule: bool, text: &dyn Fn(i64, i64) -> bool) -> (usize, usize) {
    let n = dens.len() as i64;
    let nf = n as f64;
    let ok = |i: i64, lo: f64| i >= 0 && i < n && dens[i as usize] >= lo && dens[i as usize] <= CROP_MAX_DENSITY;
    let gap = (CROP_RUN_GAP * nf) as i64;
    let minrun = (CROP_MIN_RUN * nf) as i64;
    let e0 = (CROP_EDGE_FRAC * nf) as i64;
    let fin = (nf - 1.0 - CROP_EDGE_FRAC * nf) as i64 + 1;
    let (mut rs, mut re) = (-1i64, 0i64);
    let mut all: Option<(i64, i64)> = None;
    let mut runs: Vec<(i64, i64)> = vec![];
    for i in e0..=fin {
        if i < fin && ok(i, CROP_MIN_DENSITY) {
            if rs < 0 {
                rs = i;
            }
            re = i;
            continue;
        }
        if rs >= 0 && (i - re > gap || i == fin) {
            all = Some((all.map_or(rs, |a| a.0), re));
            if re - rs + 1 >= minrun {
                runs.push((rs, re));
            }
            rs = -1;
        }
    }
    let band = (CROP_BAND_FRAC * nf) as i64;
    let (mut lo_lim, mut hi_lim) = (e0, n - 1 - e0);
    if band_rule && runs.len() >= 2 && runs[0].1 <= band {
        if runs[0].0 > e0 && text(runs[0].0, runs[0].1) {
            lo_lim = lo_lim.max(runs[0].0 - (CROP_PAD_FRAC * nf) as i64);
        } else {
            lo_lim = runs[0].1 + 1;
            runs.remove(0);
        }
    }
    if band_rule && runs.len() >= 2 && runs.last().unwrap().0 >= n - 1 - band {
        let r = *runs.last().unwrap();
        if r.1 < n - 1 - e0 && text(r.0, r.1) {
            hi_lim = hi_lim.min(r.1 + (CROP_PAD_FRAC * nf) as i64);
        } else {
            hi_lim = r.0 - 1;
            runs.pop();
        }
    }
    let block = if runs.is_empty() { all } else { Some((runs[0].0, runs.last().unwrap().1)) };
    let (mut first, mut last) = match block {
        None => (0, n - 1),
        Some((f0, l0)) => {
            let (mut f, mut l) = (f0, l0);
            let near = CROP_NEAR_FRAC * nf;
            let mut i = f0 - 1;
            while i as f64 >= f0 as f64 - near && i >= lo_lim {
                if ok(i, CROP_NEAR_DENSITY) {
                    f = i;
                }
                i -= 1;
            }
            let mut i = l0 + 1;
            while i as f64 <= l0 as f64 + near && i <= hi_lim {
                if ok(i, CROP_NEAR_DENSITY) {
                    l = i;
                }
                i += 1;
            }
            (f, l)
        }
    };
    first = ((first as f64 - CROP_PAD_FRAC * nf).trunc() as i64).max(0);
    last = ((last as f64 + CROP_PAD_FRAC * nf).trunc() as i64).min(n - 1);
    if lo_lim > e0 && first < lo_lim {
        first = lo_lim;
    }
    if hi_lim < n - 1 - e0 && last > hi_lim {
        last = hi_lim;
    }
    (first as usize, (last - first + 1) as usize)
}

/// Gutter of a double page from a flattened image: Some((x, width)) or None. Port of bash ink_profile + its awk:
/// column profile over GRID_ROWS bands (top/bottom shaved), widest ink-free run in the centre window, else the
/// deepest valley of the smoothed ink count. The centre window is 20-80 % of the text box's columns x0..x1, not of
/// the whole photo: flattening turns a dark table white, and the gap between its edge and the page was the widest
/// ink-free run in the photo's centre window (f6, #46). Widths stay fractions of the page (a box-relative width limit
/// turned a blank facing page on f3 p2 into a valley cut).
fn gutter(flat: &Img, (x0, x1): (usize, usize)) -> Option<(usize, usize)> {
    let (w, h) = (flat.w, flat.h);
    let shave = (h as f64 * GUTTER_EDGE_SHAVE) as usize;
    let hh = h - 2 * shave;
    // Area average per band. Bash uses -resize (Lanczos); a Lanczos port matched it better per pixel but picked a
    // gutter 243 px off on skewed p21 (two near-equal ink-free runs), the area average stays within 5 px (#29).
    let mut cnt = vec![0u32; w];
    for b in 0..GRID_ROWS {
        let (r0, r1) = (shave + b * hh / GRID_ROWS, shave + (b + 1) * hh / GRID_ROWS);
        for x in 0..w {
            let s: f32 = (r0..r1).map(|y| flat.px[y * w + x]).sum();
            if ((s / (r1 - r0) as f32) * 255.0).round() < GUTTER_INK_THRESH {
                cnt[x] += 1;
            }
        }
    }
    let ink: Vec<bool> = cnt.iter().map(|&c| c >= GUTTER_MIN_INK_ROWS).collect();
    let wf = w as f64;
    let bw = (x1 - x0) as f64;
    let (lo, hi) = ((x0 as f64 + bw * GUTTER_SEARCH_LO) / wf, (x0 as f64 + bw * GUTTER_SEARCH_HI) / wf);

    let valley = || {
        let sw = ((wf * 0.003) as i64).max(1);
        let sm: Vec<f64> = (0..w as i64)
            .map(|x| {
                let r = (x - sw).max(0)..=(x + sw).min(w as i64 - 1);
                let n = r.clone().count() as f64;
                r.map(|k| cnt[k as usize] as f64).sum::<f64>() / n
            })
            .collect();
        let (mut best, mut bx) = (1e9, -1i64);
        for x in (wf * lo) as i64..=(wf * hi) as i64 {
            if sm[x as usize] < best {
                (best, bx) = (sm[x as usize], x);
            }
        }
        let (a1, a2) = ((wf * 0.03) as i64, (wf * 0.15) as i64);
        let side = |r: std::ops::RangeInclusive<i64>| {
            let v: Vec<f64> = r.filter(|&k| k >= 0 && k < w as i64).map(|k| sm[k as usize]).collect();
            (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
        };
        let (Some(lb), Some(rb)) = (side(bx - a2..=bx - a1), side(bx + a1..=bx + a2)) else { return None };
        let r = GUTTER_VALLEY_RATIO;
        (bx >= 0 && best <= r * lb && best <= r * rb && lb > 0.0 && rb > 0.0).then_some((bx as usize, 0))
    };

    let (mut bestlen, mut best) = (-1i64, None);
    let mut start: Option<usize> = None;
    for x in 0..=w {
        if x < w && !ink[x] {
            start.get_or_insert(x);
        } else if let Some(rs) = start.take() {
            let re = x - 1;
            let mid = (rs + re) as f64 / 2.0 / wf;
            if mid >= lo && mid <= hi && (re - rs) as i64 > bestlen {
                (bestlen, best) = ((re - rs) as i64, Some((rs, re)));
            }
        }
    }
    let Some((bs, be)) = best else { return valley() };
    let (gx, gw) = ((bs + be) / 2, be - bs);
    let half = w / 2;
    let li = ink[..half].iter().filter(|&&b| b).count() as f64 / half as f64;
    let ri = ink[half..].iter().filter(|&&b| b).count() as f64 / (w - half) as f64;
    let (gf, gwf) = (gx as f64 / wf, gw as f64 / wf);
    let ok = gf >= lo && gf <= hi && gwf >= GUTTER_MIN_WIDTH_FRAC && gwf <= GUTTER_MAX_WIDTH_FRAC
        && li >= GUTTER_MIN_INK_FRAC && ri >= GUTTER_MIN_INK_FRAC;
    if ok { Some((gx, gw)) } else { valley() }
}

/// Tesseract OSD quarter turn (0/90/180/270), like bash rotate_page. Only with BITONAL_OSD=tesseract; `osd_own` is the default (#30).
fn osd(g: &Img) -> u32 {
    let path = std::env::temp_dir().join(format!("bitonal-osd-{}-{:p}.pgm", std::process::id(), g));
    let mut f = format!("P5 {} {} 255\n", g.w, g.h).into_bytes();
    f.extend(g.px.iter().map(|&v| (v * 255.0).round() as u8));
    if std::fs::write(&path, f).is_err() {
        return 0;
    }
    let out = std::process::Command::new("tesseract").arg(&path).args(["stdout", "--psm", "0"]).output();
    let _ = std::fs::remove_file(&path);
    let Ok(out) = out else { return 0 };
    let s = String::from_utf8_lossy(&out.stdout);
    let field = |k: &str| s.lines().find_map(|l| l.strip_prefix(k)).map(|v| v.trim().to_string());
    let deg: u32 = field("Rotate:").and_then(|v| v.parse().ok()).unwrap_or(0);
    let conf: f32 = field("Orientation confidence:").and_then(|v| v.parse().ok()).unwrap_or(0.0);
    if deg != 0 && conf >= OSD_MIN_CONFIDENCE { deg } else { 0 }
}

/// Orientation without Tesseract (#30): (quarter turn like `osd`, confidence). Two stages on flattened darkness maps:
/// the axis from the sharpness of row vs column profiles (coarse, `ka`), then the direction along that axis (`kd`).
fn osd_own(g: &Img, ka: usize, kd: usize, tile: usize) -> (u32, f32) {
    let (q, ratio, asym) = osd_parts(g, ka, kd, tile);
    (q, ratio * asym.abs())
}

/// `osd_own` before the confidence is formed: (quarter turn, axis ratio, direction score). `--osd-eval` prints both.
fn osd_parts(g: &Img, ka: usize, kd: usize, tile: usize) -> (u32, f32, f32) {
    let dark = |k: usize| {
        let f = flatten(&downsample(g, k), BLUR_SIGMA / k as f32);
        Img { w: f.w, h: f.h, px: f.px.iter().map(|&v| 1.0 - v).collect() }
    };
    let a = dark(ka);
    // Tiles with a row or column darker than text lines get (pictures, dark table or book edge in a photo, #45) are
    // left out: their edges would outweigh the text.
    let dense = |p: &[f64]| p.iter().any(|&v| v > OSD_DARK_MAX);
    let (pr, pc): (Vec<_>, Vec<_>) =
        profiles(&a, true, tile).into_iter().zip(profiles(&a, false, tile)).filter(|(r, c)| !dense(r) && !dense(c)).unzip();
    if pr.is_empty() {
        return (0, 0.0, 0.0); // nothing text-like (full-bleed picture): no evidence, and 0/0 would make the ratio NaN -> 1e3
    }
    let (sr, sc) = (sharp(&pr), sharp(&pc));
    let rows = sr >= sc;
    let d = dark(kd);
    let d = if rows { d } else { rot90(&d, 90) };
    let asym = direction(&profiles(&d, true, tile * ka / kd));
    let q = match (rows, asym < 0.0) { (true, false) => 0, (true, true) => 180, (false, false) => 90, (false, true) => 270 };
    (q, (if rows { sr / sc } else { sc / sr }).min(1e3) as f32, asym as f32)
}

/// Ink per row (rows = true) or per column of every `l` x `l` tile inside the page's central 90 % (scanner borders
/// out). Tiles, not whole-page profiles: skew smears a page-wide profile, and rows and columns get equal sample counts
/// so neither is favoured by noise.
fn profiles(g: &Img, rows: bool, l: usize) -> Vec<Vec<f64>> {
    let (x0, y0) = (g.w / 20, g.h / 20);
    let mut out = vec![];
    for ty in (y0..g.h - y0).step_by(l).filter(|t| t + l <= g.h - y0) {
        for tx in (x0..g.w - x0).step_by(l).filter(|t| t + l <= g.w - x0) {
            out.push((0..l).map(|i| (0..l).map(|j| {
                let (x, y) = if rows { (tx + j, ty + i) } else { (tx + i, ty + j) };
                g.px[y * g.w + x] as f64
            }).sum::<f64>() / l as f64).collect());
        }
    }
    out
}

/// Sum of squared steps of the profiles, relative to their energy: high for sharp text lines, low for smooth ones.
fn sharp(ps: &[Vec<f64>]) -> f64 {
    let d: f64 = ps.iter().flat_map(|p| p.windows(2)).map(|w| (w[1] - w[0]).powi(2)).sum();
    d / ps.iter().flatten().map(|v| v * v).sum::<f64>().max(1e-12)
}

/// > 0 for upright Latin text (rows top to bottom): per text line (between local minima of the smoothed profile),
/// the ink above the x-height band (ascenders, capitals) outweighs the ink below the baseline (descenders).
/// "Lines" peaking above OSD_DARK_MAX are dark edges, not text: a book edge at the bottom of a photo reads as one
/// huge descender and turned whole documents by 180° (#45).
/// Tried first and dropped: steps down in density outweigh steps up (docs/rust-port.md section 3).
fn direction(ps: &[Vec<f64>]) -> f64 {
    let (mut above, mut below) = (0.0, 0.0);
    for p in ps {
        let sm: Vec<f64> = (0..p.len()).map(|i| p[i.saturating_sub(1)..(i + 2).min(p.len())].iter().sum::<f64>() / 3.0).collect();
        let mut a = 0;
        while a + 1 < sm.len() {
            let mut b = a + 1;
            while b + 1 < sm.len() && !(sm[b] <= sm[b - 1] && sm[b] < sm[b + 1] && b - a >= 3) {
                b += 1;
            }
            let m = sm[a..=b].iter().cloned().fold(0.0, f64::max);
            if m > 0.02 && m <= OSD_DARK_MAX {
                let hi: Vec<usize> = (a..=b).filter(|&i| sm[i] >= 0.5 * m).collect();
                above += p[a..hi[0]].iter().sum::<f64>();
                below += p[hi[hi.len() - 1] + 1..=b].iter().sum::<f64>();
            }
            a = b;
        }
    }
    (above - below) / (above + below).max(1e-12)
}

/// #30 measurement: every page, upright (by Tesseract) and turned 0/90/180/270, own detector vs the truth.
fn osd_eval(data: &Arc<Vec<u8>>, n: usize) {
    let ev = |k: &str, d: usize| std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d);
    let (ka, kd, tile) = (ev("BITONAL_OSDKA", OSD_KA), ev("BITONAL_OSDKD", OSD_KD), ev("BITONAL_OSDTILE", OSD_TILE));
    let tess = std::env::var_os("EVAL_TESS").is_some();
    let upright = std::env::var_os("EVAL_UPRIGHT").is_some(); // pages known to be upright (#45): no Tesseract truth
    let rows = par_pages(data, n, |p, i| {
        let (g, _) = page_gray(p, 300.0, false);
        let g = Img { w: g.w as usize, h: g.h as usize, px: g.px.iter().map(|&v| v as f32 / 255.0).collect() };
        let up = if upright { g } else { rot90(&g, osd(&g)) };
        (0..4).map(|q| {
            let r = q * 90;
            let x = rot90(&up, r);
            let t0 = Instant::now();
            let (d, ratio, asym) = osd_parts(&x, ka, kd, tile);
            let c = ratio * asym.abs();
            let own_ms = t0.elapsed().as_millis();
            let (t, tms) = if tess { let t0 = Instant::now(); (osd(&x) as i64, t0.elapsed().as_millis()) } else { (-1, 0) };
            format!(
                "page {} turn {r} expect {} own {d} conf {c:.2} ratio {ratio:.2} asym {asym:.3} own_ms {own_ms} tess {t} tess_ms {tms}",
                i + 1,
                (360 - r) % 360
            )
        }).collect::<Vec<_>>()
    });
    rows.into_iter().flatten().for_each(|l| println!("{l}"));
}

/// #43 measurement, no rules: candidate picture-vs-text signals for every page and every DET_TILE square of it, one
/// TSV line each, for any number of PDFs. The page is always rendered (colour is needed); the ink map is the
/// flatten + hysteresis map of the output path. Join with the labels outside (docs/rust-port.md section 4).
const DET_TILE: usize = 320;
const DET_SIGNALS: [&str; 11] =
    ["hasler", "nongrey20", "nongrey60", "lum_std", "midtone", "entropy", "ink", "cc_per_mpx", "cc_med_area", "cc_big_frac", "edge"];

fn detect_eval(files: &[String]) {
    println!("#file\tpage\tscope\tx0\ty0\tx1\ty1\tW\tH\t{}", DET_SIGNALS.join("\t"));
    for f in files {
        let data = Arc::new(std::fs::read(f).unwrap_or_else(|e| die(&format!("{f}: {e}"))));
        let n = Pdf::new(data.clone()).unwrap_or_else(|e| die(&format!("{f}: {e:?}"))).pages().len();
        let name = std::path::Path::new(f).file_name().map_or(f.clone(), |s| s.to_string_lossy().into_owned());
        for l in par_pages(&data, n, detect_page).into_iter().flatten() {
            println!("{name}\t{l}");
        }
    }
}

fn detect_page(page: &Page, i: usize) -> Vec<String> {
    let (pw, ph) = page.render_dimensions();
    let s = 300.0f32 / 72.0;
    let (w, h) = ((pw * s).ceil() as usize, (ph * s).ceil() as usize);
    let rs = RenderSettings { x_scale: s, y_scale: s, width: Some(w as u16), height: Some(h as u16), bg_color: WHITE };
    let pix = render(page, &RenderCache::new(), &InterpreterSettings::default(), &rs);
    let rgb: Vec<[u8; 3]> = pix.data().iter().map(|p| [p.r, p.g, p.b]).collect();
    let lum: Vec<f32> = rgb.iter().map(|p| (p[0] as f32 * 0.299 + p[1] as f32 * 0.587 + p[2] as f32 * 0.114) / 255.0).collect();
    let img = Img { w, h, px: lum };
    let ink = hyst(&flatten(&img, BLUR_SIGMA), 60.0);
    let (lab, comps) = components(&ink, w, h);
    let lum = &img.px;

    let sig = |x0: usize, y0: usize, x1: usize, y1: usize| -> String {
        let (mut n, mut rg, mut rg2, mut yb, mut yb2, mut ng20, mut ng60) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
        let (mut l1, mut l2, mut mid, mut edge, mut inks, mut big) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
        let mut hist = [0f64; 64];
        for y in y0..y1 {
            for x in x0..x1 {
                let k = y * w + x;
                let [r, g, b] = rgb[k].map(|v| v as f64);
                let (d1, d2) = (r - g, 0.5 * (r + g) - b);
                let c = r.max(g).max(b) - r.min(g).min(b);
                n += 1.0;
                (rg, rg2, yb, yb2) = (rg + d1, rg2 + d1 * d1, yb + d2, yb2 + d2 * d2);
                ng20 += (c >= 20.0) as u8 as f64;
                ng60 += (c >= 60.0) as u8 as f64;
                let l = lum[k] as f64;
                (l1, l2) = (l1 + l, l2 + l * l);
                mid += (0.2..=0.8).contains(&l) as u8 as f64;
                hist[((l * 64.0) as usize).min(63)] += 1.0;
                if ink[k] {
                    inks += 1.0;
                    big += (comps[lab[k] as usize].0 >= 2000) as u8 as f64;
                }
                if x > 0 && y > 0 && x + 1 < w && y + 1 < h {
                    let (gx, gy) = ((lum[k + 1] - lum[k - 1]) as f64, (lum[k + w] - lum[k - w]) as f64);
                    edge += (gx * gx + gy * gy > 0.04) as u8 as f64;
                }
            }
        }
        let (mrg, myb) = (rg / n, yb / n);
        let hasler = ((rg2 / n - mrg * mrg).max(0.0) + (yb2 / n - myb * myb).max(0.0)).sqrt() + 0.3 * (mrg * mrg + myb * myb).sqrt();
        let entropy: f64 = hist.iter().filter(|&&c| c > 0.0).map(|&c| -(c / n) * (c / n).log2()).sum();
        let mut areas: Vec<u32> = comps
            .iter()
            .filter(|c| c.0 > 0 && (c.1 / c.0 as f64) >= x0 as f64 && (c.1 / c.0 as f64) < x1 as f64 && (c.2 / c.0 as f64) >= y0 as f64 && (c.2 / c.0 as f64) < y1 as f64)
            .map(|c| c.0)
            .collect();
        areas.sort_unstable();
        let med = areas.get(areas.len() / 2).copied().unwrap_or(0);
        let v = [
            hasler, ng20 / n, ng60 / n, (l2 / n - (l1 / n).powi(2)).max(0.0).sqrt(), mid / n, entropy, inks / n,
            areas.len() as f64 / (n / 1e6), med as f64, big / inks.max(1.0), edge / n,
        ];
        format!("{x0}\t{y0}\t{x1}\t{y1}\t{w}\t{h}\t{}", v.iter().map(|x| format!("{x:.4}")).collect::<Vec<_>>().join("\t"))
    };

    let mut out = vec![format!("{}\tpage\t{}", i + 1, sig(0, 0, w, h))];
    for y in (0..h).step_by(DET_TILE) {
        for x in (0..w).step_by(DET_TILE) {
            let (x1, y1) = ((x + DET_TILE).min(w), (y + DET_TILE).min(h));
            if x1 - x >= DET_TILE / 2 && y1 - y >= DET_TILE / 2 {
                out.push(format!("{}\ttile\t{}", i + 1, sig(x, y, x1, y1)));
            }
        }
    }
    out
}

/// 8-connected components of `ink`: the root label of every ink pixel, and per root (area, sum x, sum y); area 0 = not a root.
fn components(ink: &[bool], w: usize, h: usize) -> (Vec<u32>, Vec<(u32, f64, f64)>) {
    fn find(p: &mut [u32], mut a: u32) -> u32 {
        while p[a as usize] != a {
            p[a as usize] = p[p[a as usize] as usize];
            a = p[a as usize];
        }
        a
    }
    let mut lab = vec![0u32; w * h];
    let mut parent: Vec<u32> = vec![];
    for y in 0..h {
        for x in 0..w {
            if !ink[y * w + x] {
                continue;
            }
            let mut near = [u32::MAX; 4];
            // Indices are built for all four neighbours before the bounds flag says which are valid: wrap on purpose, the flag guards the use.
            let up = y.wrapping_sub(1).wrapping_mul(w);
            let nb = [(x > 0, (y * w).wrapping_add(x.wrapping_sub(1))), (x > 0 && y > 0, up.wrapping_add(x.wrapping_sub(1))), (y > 0, up.wrapping_add(x)), (y > 0 && x + 1 < w, up.wrapping_add(x + 1))];
            for (j, (ok, k)) in nb.iter().enumerate() {
                if *ok && ink[*k] {
                    near[j] = find(&mut parent, lab[*k]);
                }
            }
            let m = near.iter().copied().min().unwrap();
            let l = if m == u32::MAX {
                parent.push(parent.len() as u32);
                parent.len() as u32 - 1
            } else {
                for &o in near.iter().filter(|&&o| o != u32::MAX) {
                    parent[o as usize] = m;
                }
                m
            };
            lab[y * w + x] = l;
        }
    }
    let mut comps = vec![(0u32, 0f64, 0f64); parent.len()];
    for y in 0..h {
        for x in 0..w {
            if ink[y * w + x] {
                let r = find(&mut parent, lab[y * w + x]);
                lab[y * w + x] = r;
                let c = &mut comps[r as usize];
                *c = (c.0 + 1, c.1 + x as f64, c.2 + y as f64);
            }
        }
    }
    (lab, comps)
}

/// Gaussian blur, edges clamped. A sigma-30 blur is smooth, so it is computed on a copy downsampled by k (area
/// average), with an exact kernel there, and upsampled bilinearly; sigma is reduced by the variance the box and
/// the tent add. Close to the exact full-size Gaussian (test `blur_matches_exact`) at a fraction of its work.
/// The earlier 3x box blur was off by up to 9 % at a dark page edge, which flipped crop decisions (#29).
fn gauss(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let k = ((sigma / 8.0) as usize).max(1);
    if k == 1 {
        return gauss_exact(src, w, h, sigma);
    }
    // Low-res grid with a 4-sigma border of replicated edge pixels, so the clamp means the same as at full size
    // (a scan's edge band has a steep gradient in its outer rows; repeating a block average there was off by 1 %).
    let p = (4.0 * sigma / k as f32).ceil() as usize;
    let (lw, lh) = (w.div_ceil(k) + 2 * p, h.div_ceil(k) + 2 * p);
    let at = |x: usize, y: usize| src[(y.saturating_sub(p * k)).min(h - 1) * w + (x.saturating_sub(p * k)).min(w - 1)];
    let mut low = vec![0.0f32; lw * lh];
    for y in 0..lh * k {
        for x in 0..lw * k {
            low[(y / k) * lw + x / k] += at(x, y);
        }
    }
    let kf = k as f32;
    low.iter_mut().for_each(|v| *v /= kf * kf);
    let s = (sigma * sigma - kf * kf / 12.0 - kf * kf / 6.0).sqrt() / kf;
    let low = gauss_exact(&low, lw, lh, s);
    // bilinear; low-res pixel j has its centre at (j - p + 0.5) * k - 0.5
    let coord = |x: usize| {
        let u = (x as f32 + 0.5) / kf - 0.5 + p as f32;
        let j = u as usize;
        (j, j + 1, u - j as f32)
    };
    let xs: Vec<_> = (0..w).map(coord).collect();
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        let (j0, j1, fy) = coord(y);
        let (r0, r1) = (&low[j0 * lw..(j0 + 1) * lw], &low[j1 * lw..(j1 + 1) * lw]);
        for (x, &(i0, i1, fx)) in xs.iter().enumerate() {
            let a = r0[i0] + (r0[i1] - r0[i0]) * fx;
            let b = r1[i0] + (r1[i1] - r1[i0]) * fx;
            out[y * w + x] = a + (b - a) * fy;
        }
    }
    out
}

/// Exact separable Gaussian, radius 4 sigma, edges clamped.
fn gauss_exact(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let r = (4.0 * sigma).ceil() as isize;
    let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let norm: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|v| v / norm).collect();
    let pass = |src: &[f32], n: usize, m: usize, step: usize, lstep: usize| {
        let mut dst = vec![0.0; src.len()];
        for l in 0..m {
            for i in 0..n as isize {
                let mut acc = 0.0;
                for (j, kv) in k.iter().enumerate() {
                    let t = (i + j as isize - r).clamp(0, n as isize - 1) as usize;
                    acc += kv * src[l * lstep + t * step];
                }
                dst[l * lstep + i as usize * step] = acc;
            }
        }
        dst
    };
    let a = pass(src, w, h, 1, w);
    pass(&a, h, w, w, 1)
}

fn encode_g4(bits: &[bool], w: u32) -> Vec<u8> {
    let mut enc = fax::encoder::Encoder::new(fax::VecWriter::new());
    for row in bits.chunks_exact(w as usize) {
        enc.encode_line(row.iter().map(|&b| if b { fax::Color::Black } else { fax::Color::White }), w).unwrap();
    }
    enc.finish().unwrap().finish()
}

/// A finished output page. G4: 1-bit, page size in pt = px*72/dpi. Jpeg: the MediaBox is given (`pt`), so a 150-dpi
/// colour page can sit on the 300-dpi text canvas' page size.
#[allow(dead_code)] // Jpeg is unused until B4e (#76)
enum Slot {
    G4(u32, u32, Vec<u8>),
    Jpeg { w: u32, h: u32, data: Vec<u8>, pt: (f32, f32) },
}

/// RGB -> baseline JPEG, 4:2:0, quality `q` (the encoder's default tables are the IJG ones).
#[allow(dead_code)]
fn encode_jpeg(g: &Rgb, q: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = jpeg_encoder::Encoder::new(&mut out, q);
    enc.set_sampling_factor(jpeg_encoder::SamplingFactor::F_2_2);
    enc.encode(&g.px, g.w as u16, g.h as u16, jpeg_encoder::ColorType::Rgb).unwrap();
    out
}

/// A colour page (q65) for a finished RGB slot, on a page of `pt` points.
// #76: crop, split and deskew are done on bits in Pass B today; the RGB slot must have them applied before it gets here.
#[allow(dead_code)]
fn jpeg_slot(g: &Rgb, pt: (f32, f32)) -> Slot {
    Slot::Jpeg { w: g.w, h: g.h, data: encode_jpeg(g, 65), pt }
}

fn write_pdf(pages: &[(u32, u32, Vec<u8>)], dpi: f32) -> Vec<u8> {
    let slots: Vec<Slot> = pages.iter().map(|(w, h, g4)| Slot::G4(*w, *h, g4.clone())).collect();
    write_slots(&slots, dpi)
}

fn write_slots(pages: &[Slot], dpi: f32) -> Vec<u8> {
    use pdf_writer::{Content, Filter, Name, Pdf, Rect, Ref};
    let mut pdf = Pdf::new();
    let (catalog, tree) = (Ref::new(1), Ref::new(2));
    let ids = |k: usize| (Ref::new(3 + 3 * k as i32), Ref::new(4 + 3 * k as i32), Ref::new(5 + 3 * k as i32));
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree).kids((0..pages.len()).map(|k| ids(k).0)).count(pages.len() as i32);
    for (k, slot) in pages.iter().enumerate() {
        let (page_id, img_id, content_id) = ids(k);
        let (pw, ph) = match slot {
            Slot::G4(w, h, _) => (*w as f32 * 72.0 / dpi, *h as f32 * 72.0 / dpi),
            Slot::Jpeg { pt, .. } => *pt,
        };
        let mut page = pdf.page(page_id);
        page.parent(tree).media_box(Rect::new(0.0, 0.0, pw, ph)).contents(content_id);
        page.resources().x_objects().pair(Name(b"Im0"), img_id);
        drop(page);
        match slot {
            Slot::G4(w, h, g4) => {
                let mut img = pdf.image_xobject(img_id, g4);
                img.filter(Filter::CcittFaxDecode);
                img.width(*w as i32).height(*h as i32).bits_per_component(1);
                img.color_space().device_gray();
                img.decode_parms().pair(Name(b"K"), -1).pair(Name(b"Columns"), *w as i32).pair(Name(b"Rows"), *h as i32);
            }
            Slot::Jpeg { w, h, data, .. } => {
                let mut img = pdf.image_xobject(img_id, data);
                img.filter(Filter::DctDecode);
                img.width(*w as i32).height(*h as i32).bits_per_component(8);
                img.color_space().device_rgb();
            }
        }
        let mut c = Content::new();
        c.save_state().transform([pw, 0.0, 0.0, ph, 0.0, 0.0]).x_object(Name(b"Im0")).restore_state();
        pdf.stream(content_id, &c.finish());
    }
    pdf.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Round trip through the whole I/O layer: bitmap -> G4 -> PDF -> hayro -> extraction path -> same bitmap,
    // at the right page size. Covers encoder polarity, DecodeParms, MediaBox/dpi and the Probe fit check.
    #[test]
    fn g4_pdf_roundtrip() {
        let (w, h, dpi) = (301u32, 207u32, 150.0);
        let bits: Vec<bool> = (0..w * h).map(|i| (i % w) * (i / w) % 7 == 0 || (i % w + 2 * (i / w)) % 13 < 3).collect();
        let pdf = write_pdf(&[(w, h, encode_g4(&bits, w))], dpi);
        let doc = Pdf::new(Arc::new(pdf)).unwrap();
        let page = &doc.pages()[0];
        let (pw, ph) = page.render_dimensions();
        assert!((pw - w as f32 * 72.0 / dpi).abs() < 0.01 && (ph - h as f32 * 72.0 / dpi).abs() < 0.01);
        let (g, src) = page_gray(page, dpi, true);
        assert_eq!((src, g.w, g.h), ("image", w, h));
        assert!(g.px.iter().zip(&bits).all(|(&p, &b)| (p < 128) == b));
    }

    // One PDF, G4 + JPEG + G4 pages, one page size: hayro decodes all three, G4 stays exact, the JPEG page is the source
    // within a loose PSNR (proves decoding, colour order and placement, not codec quality).
    #[test]
    fn mixed_pdf_roundtrip() {
        let (w, h, dpi) = (400u32, 300u32, 300.0f32);
        let bits: Vec<bool> = (0..w * h).map(|i| (i % w) * (i / w) % 7 == 0 || (i % w + 2 * (i / w)) % 13 < 3).collect();
        let (jw, jh) = (w / 2, h / 2); // 150 dpi on the 300-dpi canvas' page size
        let px: Vec<u8> = (0..jw * jh)
            .flat_map(|i| {
                let (x, y) = (i % jw, i / jw);
                [(x * 255 / jw) as u8, (y * 255 / jh) as u8, if x < jw / 2 { 40 } else { 220 }]
            })
            .collect();
        let rgb = Rgb { w: jw, h: jh, px };
        let pt = (w as f32 * 72.0 / dpi, h as f32 * 72.0 / dpi);
        let g4 = || Slot::G4(w, h, encode_g4(&bits, w));
        let pdf = write_slots(&[g4(), jpeg_slot(&rgb, pt), g4()], dpi);
        if let Some(f) = std::env::var_os("MIXED_OUT") {
            std::fs::write(f, &pdf).unwrap(); // for pdfimages -list / a visual check
        }
        let doc = Pdf::new(Arc::new(pdf)).unwrap();
        let pages = doc.pages();
        assert_eq!(pages.len(), 3);
        for p in pages.iter() {
            let (pw, ph) = p.render_dimensions();
            assert!((pw - pt.0).abs() < 0.01 && (ph - pt.1).abs() < 0.01);
        }
        let (g, src) = page_gray(&pages[0], dpi, true);
        assert_eq!(src, "image");
        assert!(g.px.iter().zip(&bits).all(|(&p, &b)| (p < 128) == b));
        let out = page_rgb(&pages[1], 150.0);
        assert_eq!((out.w, out.h), (jw, jh));
        let mse = out.px.iter().zip(&rgb.px).map(|(&a, &b)| (a as f64 - b as f64).powi(2)).sum::<f64>() / rgb.px.len() as f64;
        let psnr = 10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10();
        assert!(psnr >= 30.0, "psnr {psnr}");
    }

    // #75: encode time of one 150-dpi A4 page (1240x1754), 3 runs. cargo test --release -- --ignored --nocapture jpeg_encode_time
    #[test]
    #[ignore]
    fn jpeg_encode_time() {
        let (w, h) = (1240u32, 1754u32);
        let px: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                let t = if (x / 9 + y / 14) % 5 == 0 { 30 } else { 235 }; // text-like edges on a tint
                [t, (x * 255 / w) as u8 / 2 + t / 2, (y * 255 / h) as u8 / 2 + t / 2]
            })
            .collect();
        let rgb = Rgb { w, h, px };
        for run in 1..=3 {
            let t = Instant::now();
            let n = encode_jpeg(&rgb, 65).len();
            eprintln!("run {run}: {} ms, {n} B", t.elapsed().as_millis());
        }
    }

    // Same on a real page: REAL_PDF=<file> REAL_PAGE=<0-based> cargo test --release -- --ignored --nocapture jpeg_encode_time_real
    // Times the render to RGB (page_rgb) and the encode separately.
    #[test]
    #[ignore]
    fn jpeg_encode_time_real() {
        let f = std::env::var("REAL_PDF").expect("REAL_PDF");
        let k: usize = std::env::var("REAL_PAGE").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        let doc = Pdf::new(Arc::new(std::fs::read(f).unwrap())).unwrap();
        for run in 1..=3 {
            let t = Instant::now();
            let rgb = page_rgb(&doc.pages()[k], 150.0);
            let render_ms = t.elapsed().as_millis();
            let t = Instant::now();
            let n = encode_jpeg(&rgb, 65).len();
            eprintln!("run {run}: {}x{} render {render_ms} ms, encode {} ms, {n} B", rgb.w, rgb.h, t.elapsed().as_millis());
        }
    }

    // A slanted, dithered dark band at the slot edge is whitened (#53), no single column of it reaches half the height;
    // a text-like left margin (short stems) and text are kept.
    #[test]
    fn edge_band_cleared_text_kept() {
        let (w, h) = (400usize, 480usize);
        let mut bits = vec![false; w * h];
        for y in 0..h {
            let x0 = 20 + y / 24; // slants 20 px over the height
            for x in x0..x0 + 12 {
                bits[y * w + x] = (x + y) % 5 != 0; // dithered
            }
            if y % 20 < 8 {
                for x in 100..108 {
                    bits[y * w + x] = true; // margin stems
                }
                for x in 150..300 {
                    bits[y * w + x] = y % 3 == 0;
                }
            }
        }
        let before = bits.clone();
        clear_edge_bands(&mut bits, w, h);
        assert!((0..h).all(|y| (0..60).all(|x| !bits[y * w + x])));
        assert!((0..h).all(|y| (60..w).all(|x| bits[y * w + x] == before[y * w + x])));
    }

    // Text-like lines skewed by +2° (falling to the right): skew_angle finds it, and rotate by that angle
    // makes the lines horizontal again. Fails with the rotation sign flipped.
    #[test]
    fn skew_found_and_undone() {
        let (w, h) = (600usize, 400usize);
        let t = 2f32.to_radians().tan();
        let px: Vec<f32> = (0..w * h)
            .map(|i| {
                // 4x4 supersampled, so the lines are anti-aliased like a real scan
                let ink = (0..16).filter(|k| {
                    let (x, y) = ((i % w) as f32 + (k % 4) as f32 / 4.0, (i / w) as f32 + (k / 4) as f32 / 4.0);
                    let yy = y - (x - 300.0) * t;
                    x > 50.0 && x < 550.0 && yy > 50.0 && yy < 350.0 && yy % 20.0 < 3.0
                });
                1.0 - ink.count() as f32 / 16.0
            })
            .collect();
        let g = Img { w, h, px };
        let ink = |g: &Img| g.px.iter().map(|&v| v < 0.5).collect::<Vec<_>>();
        let a = skew_angle(&ink(&g), w, h);
        assert!((a - 2.0).abs() <= 0.05, "{a}");
        let r = rotate(&g, a, w, h, 1.0);
        let b = skew_angle(&ink(&r), w, h);
        assert!(b.abs() <= 0.05, "{b}");
        assert_eq!(rot90(&rot90(&g, 90), 270).px, g.px);
        assert_eq!(rot90(&rot90(&g, 90), 90).px, rot90(&g, 180).px);
    }

    // Lines of "letters" (x-height bar, 30 % with an ascender above, 8 % with a descender below), turned by each quarter:
    // osd_own must undo the turn. Fails with the direction sign flipped.
    #[test]
    fn orientation_found_for_all_turns() {
        let (w, h) = (1200usize, 1600usize);
        let mut px = vec![1.0f32; w * h];
        let mut r = 12345u32;
        let mut rnd = || { r = r.wrapping_mul(1103515245).wrapping_add(12345); (r >> 16) % 100 };
        for line in 0..28 {
            let base = 120 + line * 50;
            let mut x0 = 100;
            while x0 < 1080 {
                let bw = 6 + rnd() as usize % 9;
                let (asc, desc) = (rnd() < 30, rnd() < 8);
                let (top, bot) = (base - 20 - if asc { 14 } else { 0 }, base + if desc { 14 } else { 0 });
                for y in top..bot {
                    for x in (x0..x0 + 2).chain(x0 + bw - 2..x0 + bw) {
                        px[y * w + x] = 0.0; // two stems per letter: solid blocks are darker than text (OSD_DARK_MAX)
                    }
                }
                x0 += bw + 3 + rnd() as usize % 7;
            }
        }
        let g = Img { w, h, px };
        for q in [0, 90, 180, 270] {
            let (d, c) = osd_own(&rot90(&g, q), OSD_KA, OSD_KD, OSD_TILE);
            assert_eq!(d, (360 - q) % 360, "turn {q}");
            assert!(c >= OSD_OWN_MIN_CONFIDENCE, "confidence {c}");
        }
    }

    // One tile profile from a photographed page (#45, f7): an upright text line with a small ascender excess, then a
    // dark book edge (fast rise to 0.7, long fade). The edge is not a text line and must not outvote it.
    #[test]
    fn direction_ignores_dark_edge() {
        let mut p = vec![0.0; 96];
        p[10..14].copy_from_slice(&[0.06, 0.09, 0.12, 0.15]); // ascenders
        p[14..22].fill(0.25); // x-height band
        p[22..25].fill(0.05); // descenders
        for i in 60..96 {
            p[i] = if i < 66 { 0.12 * (i - 59) as f64 } else { 0.7 - 0.02 * (i - 66) as f64 };
        }
        assert!(direction(&[p]) > 0.0);
    }

    // A photographed spread (#46): table flattened white, book edge, a margin gap at 22 % of the photo, text, a
    // spine with a little shadow (ink in a few bands, so no ink-free run), text. The whole-photo search took the gap.
    #[test]
    fn gutter_ignores_table_gap() {
        let (w, h) = (1000usize, 600usize);
        let px: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let text = (240..560).contains(&x) || (600..950).contains(&x);
                let dark = (190..200).contains(&x) || (text && (y / 6) % 2 == 0) || ((560..600).contains(&x) && (100..130).contains(&y));
                if dark { 0.0 } else { 1.0 }
            })
            .collect();
        let flat = Img { w, h, px };
        assert!(gutter(&flat, (0, w)).is_some_and(|(x, _)| x < 250), "the setup must reproduce the old failure");
        let g = gutter(&flat, (190, 950));
        assert!(g.is_some_and(|(x, _)| (560..600).contains(&x)), "{g:?}");
    }

    // The fast blur equals the exact Gaussian within 0.1 % of the range, also at a dark edge band and the corners.
    #[test]
    fn blur_matches_exact() {
        let (w, h) = (400usize, 300usize);
        let px: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                if y < 12 { 0.1 } else if (x * 7 + y * 13) % 11 < 3 { 0.2 } else { 0.95 }
            })
            .collect();
        let (a, b) = (gauss(&px, w, h, 30.0), gauss_exact(&px, w, h, 30.0));
        let (i, d) = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).enumerate().fold((0, 0.0), |m, v| if v.1 > m.1 { v } else { m });
        assert!(d < 0.001, "{d} at {},{}: {} vs {}", i % w, i / w, a[i], b[i]);
    }

    // content_box on a page with a wide band in the top 10 % cut off by a gap, a text block and a sparse
    // page number below it: the band is dropped, the page number kept (lessons.md 4, 6b).
    #[test]
    fn crop_drops_band_keeps_page_number() {
        let (w, h) = (1000usize, 1400usize);
        let ink: Vec<bool> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                (y > 30 && y < 100 && x < 600) // band
                    || (y >= 200 && y < 1100 && x >= 150 && x < 850 && (x + y) % 5 == 0) // text block
                    || (y >= 1180 && y < 1200 && x >= 490 && x < 510) // page number
            })
            .collect();
        let [x0, y0, x1, y1] = content_box(&ink, w, h, true, 300.0 / 25.4);
        assert!(y0 > 100 && y0 < 200, "{y0}");
        assert!(y1 > 1200, "{y1}");
        assert!(x0 < 150 && x1 > 850);
    }

    // #91: a text line near the top is kept by the band rule, a dark bar there is still dropped
    #[test]
    fn band_rule_keeps_text_line_drops_bar() {
        let (w, h) = (1000usize, 1400usize);
        let page = |top: &dyn Fn(usize, usize) -> bool| -> usize {
            let ink: Vec<bool> = (0..w * h)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    top(x, y) || (y >= 300 && y < 1100 && x >= 150 && x < 850 && (x + y) % 5 == 0)
                })
                .collect();
            content_box(&ink, w, h, true, 300.0 / 25.4)[1]
        };
        // 20 px line of 10 px letters with 6 px gaps, at 3 % from the top
        let line = page(&|x, y| y >= 42 && y < 62 && x >= 150 && x < 850 && x % 16 < 10);
        let bar = page(&|x, y| y >= 42 && y < 62 && x >= 150 && x < 850);
        assert!(line < 42, "{line}");
        assert!(bar > 200, "{bar}");
    }

    // keep_box (#55, #60) on a synthetic page: 10x14 letters in 13 lines make the text block. Returns the bits and its box.
    fn text_page() -> (Vec<bool>, usize, usize, [usize; 4]) {
        let (w, h) = (600usize, 800usize);
        let mut bits = vec![false; w * h];
        for line in 0..13 {
            for k in 0..18 {
                for y in 0..14 {
                    for x in 0..10 {
                        bits[(100 + line * 30 + y) * w + 200 + k * 16 + x] = true;
                    }
                }
            }
        }
        let a = content_box(&bits, w, h, false, 0.0);
        (bits, w, h, a)
    }

    fn letter(bits: &mut [bool], w: usize, x: usize, y: usize) {
        for yy in y..y + 14 {
            for xx in x..x + 10 {
                bits[yy * w + xx] = true;
            }
        }
    }

    // A page number far below a pre-cropped block is kept, and so is one on the cut edge.
    #[test]
    fn keep_box_keeps_page_numbers() {
        let (mut bits, w, h, a) = text_page();
        letter(&mut bits, w, 340, 640); // 140 px below the block, in its columns
        letter(&mut bits, w, 340, 786); // on the cut bottom edge
        let (k, _, _) = keep_box(&mut bits, w, h, a, &|_, y| y >= h - 3, 4.0, None);
        assert!(bits[645 * w + 345] && bits[790 * w + 345]);
        assert!(k[3] >= 800);
    }

    // A thin full-height rule at the edge is dropped, and the box does not stretch to it.
    #[test]
    fn keep_box_drops_edge_rule() {
        let (mut bits, w, h, a) = text_page();
        for y in 0..h {
            bits[y * w + 2] = true;
            bits[y * w + 3] = true;
        }
        let (k, _, _) = keep_box(&mut bits, w, h, a, &|x, _| x < 3, 4.0, None);
        assert!(!bits[400 * w + 2] && k[0] > 100);
    }

    // #71: white text on a grey fill wider than BLUR_SIGMA (f5 p1's circles). The normal flatten whitens fill and text alike;
    // the cover flatten keeps the fill as ink and the text white.
    #[test]
    fn cover_flatten_keeps_fill_around_white_text() {
        let (w, h) = (1000usize, 1000usize);
        let px = (0..w * h).map(|i| {
            let (x, y) = (i % w, i / w);
            let fill = (300..700).contains(&x) && (300..700).contains(&y);
            let text = (480..520).contains(&x) && (480..520).contains(&y);
            if fill && !text { 0.43 } else { 1.0 }
        });
        let g = Img { w, h, px: px.collect() };
        let normal = hyst(&flatten(&g, BLUR_SIGMA), 60.0);
        assert!(!normal[400 * w + 400]);
        let cover = hyst(&flatten(&g, COVER_BLUR_SIGMA), 60.0);
        assert!(cover[400 * w + 400] && !cover[500 * w + 500]);
    }

    // A page number beside the running head is kept (#60) unless a dotted rule stands right there.
    #[test]
    fn keep_box_head_number_not_dotted_rule() {
        let (mut bits, w, h, a) = text_page();
        letter(&mut bits, w, 20, 100); // on the first line, 180 px out
        let (k, _, _) = keep_box(&mut bits, w, h, a, &|_, _| false, 4.0, None);
        assert!(bits[105 * w + 25] && k[0] <= 20);
        let (mut bits, w, h, a) = text_page();
        for y in (100..700).step_by(30) {
            letter(&mut bits, w, 20, y); // the same letter, further out than the footer zone reaches, but one of a dotted column
        }
        keep_box(&mut bits, w, h, a, &|_, _| false, 4.0, None);
        assert!(!bits[105 * w + 25]);
    }

    // #84/#87: a page number the slot's top cut clipped is completed from the ring; so is a clipped line beside an anchored
    // letter (a running head: >= 3 letter pieces); an anchored letter among specks is not.
    #[test]
    fn reach_completes_page_number_and_line_not_specks() {
        // main canvas 600x800 holds a 560x760 slot at (20, 20), no skew; ring r = 10
        let (sw, sh, r) = (560usize, 760usize, 10usize);
        let run = |ring: &dyn Fn(&mut [bool], usize)| {
            let (mut bits, w, h, a) = text_page();
            let mut rb = vec![false; (w + 2 * r) * (h + 2 * r)];
            for y in 0..h {
                for x in 0..w {
                    rb[(y + r) * (w + 2 * r) + x + r] = bits[y * w + x];
                }
            }
            ring(&mut rb, w + 2 * r);
            for y in 0..h {
                for x in 0..w {
                    // the main canvas only sees the slot
                    bits[y * w + x] = rb[(y + r) * (w + 2 * r) + x + r] && (20..580).contains(&x) && (20..780).contains(&y);
                }
            }
            let near = |_: usize, y: usize| y < 22;
            let (_, _, kept) = keep_box(&mut bits, w, h, a, &near, 4.0, None);
            let e = reach(&rb, w + 2 * r, h + 2 * r, r, sw, sh, (300.0, 400.0), (280.0, 400.0 - 20.0), (0.0, 1.0), [false, true, false, false],
                [false, true, false, false], a, kept, &mut bits, w, h, None);
            (bits, w, e)
        };
        // two digits 20 px tall (1.4 letters) at y 14..34 in main coords: the top 6 rows lie above the slot
        let digits = |rb: &mut [bool], rw: usize| {
            for x0 in [470usize, 482] {
                for y in 14..34 {
                    for x in x0..x0 + 10 {
                        rb[(y + r) * rw + x + r] = true;
                    }
                }
            }
        };
        let (bits, w, e) = run(&digits);
        assert!(e.is_some() && bits[15 * w + 475] && bits[15 * w + 487]);
        // a clipped head: eight 14 px letters, 6 px apart; the part in the slot is kept, the 6 rows above it come back
        let head = |rb: &mut [bool], rw: usize| {
            for k in 0..8 {
                for y in 14..28 {
                    for x in 300 + k * 16..310 + k * 16 {
                        rb[(y + r) * rw + x + r] = true;
                    }
                }
            }
        };
        let (bits, w, e) = run(&head);
        assert!(e.is_some() && bits[15 * w + 305] && bits[15 * w + 305 + 7 * 16]);
        // the same first letter among six 3 px specks: one letter piece, no line
        let specks = |rb: &mut [bool], rw: usize| {
            for y in 14..28 {
                for x in 300..310 {
                    rb[(y + r) * rw + x + r] = true;
                }
            }
            for k in 1..7 {
                for y in 14..17 {
                    for x in 300 + k * 16..303 + k * 16 {
                        rb[(y + r) * rw + x + r] = true;
                    }
                }
            }
        };
        let (bits, w, e) = run(&specks);
        assert!(e.is_none() && !bits[15 * w + 305] && !bits[15 * w + 305 + 16]);
    }

    // After compose, ink that filled two differently wide slots edge to edge has the same margin on both sides.
    #[test]
    fn compose_equal_margins() {
        let slot = |w: usize, h: usize| (w as u32, h as u32, vec![0xFFu8; w.div_ceil(8) * h], [w / 2, 0]);
        let pages = compose(vec![slot(96, 40), slot(160, 56)]);
        let pdf = write_pdf(&pages, 150.0);
        let doc = Pdf::new(Arc::new(pdf)).unwrap();
        for page in doc.pages().iter() {
            let (g, _) = page_gray(page, 150.0, true);
            let (gw, gh) = (g.w as usize, g.h as usize);
            let cols: Vec<usize> = (0..gw).filter(|&x| (0..gh).any(|y| g.px[y * gw + x] < 128)).collect();
            assert_eq!(cols[0], gw - 1 - cols[cols.len() - 1]);
        }
    }
}
