// bitonalPDF Rust spike (#28): PDF in (embedded page image or render) -> 1-bit -> CCITT G4 PDF out.
// Text mode only, no --rotate/--crop/--split/--deskew yet (#29). Same CLI as bitonalpdf.sh:
//   bitonalpdf [--split off] input.pdf [output.pdf] [threshold%=60] [dpi=300]
// BITONAL_TIMING=1 prints per-page source and stage times (ms) to stderr; BITONAL_RENDER=1 always renders.
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

struct Gray {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut pos = vec![];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--split" if it.next().map(String::as_str) == Some("off") => {}
            s if s.starts_with("--") => die(&format!("{s}: not in the Rust spike yet (#29)")),
            _ => pos.push(a.clone()),
        }
    }
    let input = pos.first().unwrap_or_else(|| die("usage: bitonalpdf input.pdf [output.pdf] [threshold%] [dpi]"));
    let output = pos.get(1).cloned().unwrap_or_else(|| format!("{}.1bit.pdf", input.trim_end_matches(".pdf")));
    let thresh: f32 = pos.get(2).map_or(60.0, |s| s.parse().unwrap_or_else(|_| die("bad threshold")));
    let dpi: f32 = pos.get(3).map_or(300.0, |s| s.parse().unwrap_or_else(|_| die("bad dpi")));
    if &output == input {
        die("output must differ from input");
    }
    let timing = std::env::var_os("BITONAL_TIMING").is_some();

    let data = Arc::new(std::fs::read(input).unwrap_or_else(|e| die(&format!("{input}: {e}"))));
    let n = Pdf::new(data.clone()).unwrap_or_else(|e| die(&format!("{input}: {e:?}"))).pages().len();

    // One Pdf per thread: hayro's page/cache types are not Send. Pages are handed out by an atomic counter.
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(n.max(1));
    let mut pages: Vec<(usize, u32, u32, Vec<u8>)> = std::thread::scope(|s| {
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
                        let t0 = Instant::now();
                        let (g, src) = page_gray(&pdf.pages()[i], dpi);
                        let t1 = Instant::now();
                        let bits = binarize(&g, thresh);
                        let t2 = Instant::now();
                        let g4 = encode_g4(&bits, g.w);
                        if timing {
                            let ms = |a: Instant, b: Instant| (b - a).as_millis();
                            eprintln!("page {} {src} {}x{}: get {} binarize {} g4 {} ms, {} B",
                                i + 1, g.w, g.h, ms(t0, t1), ms(t1, t2), ms(t2, Instant::now()), g4.len());
                        }
                        done.push((i, g.w, g.h, g4));
                    }
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    pages.sort_by_key(|p| p.0);

    let pdf = write_pdf(&pages, dpi);
    let (in_len, out_len) = (data.len(), pdf.len());
    if out_len >= in_len {
        println!("Not smaller ({in_len} B is already small) — no file written");
        return;
    }
    std::fs::write(&output, pdf).unwrap_or_else(|e| die(&format!("{output}: {e}")));
    println!("{in_len} B -> {out_len} B: {output}");
}

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}

/// The page as 8-bit gray at `dpi`, pixel size rounded up like pdftoppm. A page that is nothing
/// but one raster image filling the page at (nearly) this resolution is decoded directly;
/// anything else is rendered.
fn page_gray(page: &Page, dpi: f32) -> (Gray, &'static str) {
    let (pw, ph) = page.render_dimensions();
    let s = dpi / 72.0;
    let (w, h) = ((pw * s).ceil() as u32, (ph * s).ceil() as u32);
    let init = Affine::scale_non_uniform(w as f64 / pw as f64, h as f64 / ph as f64) * page.initial_transform(true).to_kurbo();

    let settings = InterpreterSettings::default();
    let mut probe = Probe { w, h, other: false, images: 0, got: None };
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

/// Device that only checks whether the page is exactly one unrotated raster image covering the
/// page within 1 % of the target size, and decodes it if so.
struct Probe {
    w: u32,
    h: u32,
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
                self.got = Some(to_page(&Gray { w, h, px }, t, self.w, self.h));
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

/// 1 = ink. Flatten (pixel / blurred copy, sigma 30) then the #20 hysteresis rule:
/// ink if < thresh, or < 75 % with a pixel < 45 % within radius 2.
// ponytail: plain port of bitonalpdf.sh finish_slot for timing and size; parity tuning belongs to #29.
fn binarize(g: &Gray, thresh: f32) -> Vec<bool> {
    let (w, h) = (g.w as usize, g.h as usize);
    let src: Vec<f32> = g.px.iter().map(|&v| v as f32).collect();
    let blur = gauss(&src, w, h, 30.0);
    let flat: Vec<f32> = src.iter().zip(&blur).map(|(s, b)| if *b <= 0.0 { 1.0 } else { (s / b).min(1.0) }).collect();
    let (t, weak, seed) = (thresh / 100.0, 0.75f32.max(thresh / 100.0), 0.45);
    let seeds: Vec<bool> = flat.iter().map(|&v| v < seed).collect();
    let r = 2isize;
    let mut out = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let v = flat[y * w + x];
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

/// Gaussian blur approximated by three box blurs per axis (error < 3 %), edges clamped.
fn gauss(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let r = (((12.0 * sigma * sigma / 3.0) + 1.0).sqrt() / 2.0).round() as usize;
    let mut a = src.to_vec();
    let mut b = vec![0.0; a.len()];
    for _ in 0..3 {
        box_pass(&a, &mut b, w, h, r, 1, w); // rows
        box_pass(&b, &mut a, h, w, r, w, 1); // columns
    }
    a
}

/// One box blur of radius r along lines of length n (stride `step`), `m` lines `lstep` apart.
fn box_pass(src: &[f32], dst: &mut [f32], n: usize, m: usize, r: usize, step: usize, lstep: usize) {
    let norm = 1.0 / (2 * r + 1) as f32;
    for l in 0..m {
        let at = |i: isize| src[l * lstep + (i.clamp(0, n as isize - 1) as usize) * step];
        let mut acc: f32 = (-(r as isize)..=r as isize).map(at).sum();
        for i in 0..n as isize {
            dst[l * lstep + i as usize * step] = acc * norm;
            acc += at(i + r as isize + 1) - at(i - r as isize);
        }
    }
}

fn encode_g4(bits: &[bool], w: u32) -> Vec<u8> {
    let mut enc = fax::encoder::Encoder::new(fax::VecWriter::new());
    for row in bits.chunks_exact(w as usize) {
        enc.encode_line(row.iter().map(|&b| if b { fax::Color::Black } else { fax::Color::White }), w).unwrap();
    }
    enc.finish().unwrap().finish()
}

fn write_pdf(pages: &[(usize, u32, u32, Vec<u8>)], dpi: f32) -> Vec<u8> {
    use pdf_writer::{Content, Filter, Name, Pdf, Rect, Ref};
    let mut pdf = Pdf::new();
    let (catalog, tree) = (Ref::new(1), Ref::new(2));
    let ids = |k: usize| (Ref::new(3 + 3 * k as i32), Ref::new(4 + 3 * k as i32), Ref::new(5 + 3 * k as i32));
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree).kids((0..pages.len()).map(|k| ids(k).0)).count(pages.len() as i32);
    for (k, (_, w, h, g4)) in pages.iter().enumerate() {
        let (page_id, img_id, content_id) = ids(k);
        let (pw, ph) = (*w as f32 * 72.0 / dpi, *h as f32 * 72.0 / dpi);
        let mut page = pdf.page(page_id);
        page.parent(tree).media_box(Rect::new(0.0, 0.0, pw, ph)).contents(content_id);
        page.resources().x_objects().pair(Name(b"Im0"), img_id);
        drop(page);
        let mut img = pdf.image_xobject(img_id, g4);
        img.filter(Filter::CcittFaxDecode);
        img.width(*w as i32).height(*h as i32).bits_per_component(1);
        img.color_space().device_gray();
        img.decode_parms().pair(Name(b"K"), -1).pair(Name(b"Columns"), *w as i32).pair(Name(b"Rows"), *h as i32);
        drop(img);
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
        let pdf = write_pdf(&[(0, w, h, encode_g4(&bits, w))], dpi);
        let doc = Pdf::new(Arc::new(pdf)).unwrap();
        let page = &doc.pages()[0];
        let (pw, ph) = page.render_dimensions();
        assert!((pw - w as f32 * 72.0 / dpi).abs() < 0.01 && (ph - h as f32 * 72.0 / dpi).abs() < 0.01);
        let (g, src) = page_gray(page, dpi);
        assert_eq!((src, g.w, g.h), ("image", w, h));
        assert!(g.px.iter().zip(&bits).all(|(&p, &b)| (p < 128) == b));
    }
}
