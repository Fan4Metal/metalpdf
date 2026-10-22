//! Procedurally drawn application icon: a white sheet with a folded corner
//! and grey lines of text, a red label with "PDF" across its lower part
//! (as MetalMD's sheet carries the Markdown sign). Dependency-free so that
//! `build.rs` can include this file (`#[path = "src/icon.rs"]`) to produce
//! the `.ico` embedded in the executable and the raw RGBA the app shows
//! (window icon, start page, About), so that nothing is rasterised at
//! start-up.
//!
//! Small layers (24 px and below) drop the letters and the text lines: a
//! white sheet with a red band reads better there than four-pixel glyphs.

// build.rs and the app use different parts.
#![allow(dead_code)]

/// Sizes in the `.ico` of the executable.
pub const ICO_SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

/// Straight RGBA, channels 0..1.
type Rgba = [f32; 4];

fn hex(c: u32) -> Rgba {
    let ch = |s: u32| ((c >> s) & 0xff) as f32 / 255.0;
    [ch(16), ch(8), ch(0), 1.0]
}

fn with_alpha(c: Rgba, a: f32) -> Rgba {
    [c[0], c[1], c[2], c[3] * a.clamp(0.0, 1.0)]
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}

/// Paint `top` over `under`.
fn over(under: Rgba, top: Rgba) -> Rgba {
    let a = top[3] + under[3] * (1.0 - top[3]);
    if a <= 0.0 {
        return [0.0; 4];
    }
    let mut out = [0.0, 0.0, 0.0, a];
    for i in 0..3 {
        out[i] = (top[i] * top[3] + under[i] * under[3] * (1.0 - top[3])) / a;
    }
    out
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Signed distance from `(x, y)` to a box from `min` to `max` with corners
/// rounded by `r` (negative inside).
fn rounded_box(x: f32, y: f32, min: (f32, f32), max: (f32, f32), r: f32) -> f32 {
    let (cx, cy) = ((min.0 + max.0) / 2.0, (min.1 + max.1) / 2.0);
    let (hx, hy) = ((max.0 - min.0) / 2.0 - r, (max.1 - min.1) / 2.0 - r);
    let qx = (x - cx).abs() - hx;
    let qy = (y - cy).abs() - hy;
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// Distance from `(x, y)` to the segment `a`–`b`.
fn segment(x: f32, y: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (px, py) = (x - a.0, y - a.1);
    let (bx, by) = (b.0 - a.0, b.1 - a.1);
    let len2 = bx * bx + by * by;
    let t = if len2 > 0.0 { ((px * bx + py * by) / len2).clamp(0.0, 1.0) } else { 0.0 };
    ((px - bx * t).powi(2) + (py - by * t).powi(2)).sqrt()
}

/// Distance to the right half of the circle round `c` with radius `r`.
fn right_arc(x: f32, y: f32, c: (f32, f32), r: f32) -> f32 {
    if x >= c.0 {
        (((x - c.0).powi(2) + (y - c.1).powi(2)).sqrt() - r).abs()
    } else {
        segment(x, y, (c.0, c.1 - r), (c.0, c.1 + r)).min(f32::INFINITY)
            .min(((x - c.0).powi(2) + (y - c.1 - r).powi(2)).sqrt())
            .min(((x - c.0).powi(2) + (y - c.1 + r).powi(2)).sqrt())
    }
}

/// Distance to the strokes of "PDF", in letter units (height 1, the
/// baseline at y = 0, x from 0), the letters' left edges at `offsets`.
fn pdf_strokes(x: f32, y: f32) -> f32 {
    // Letter heights run upwards here; the caller flips y.
    let mut d = f32::INFINITY;
    // P
    let p = 0.0;
    d = d.min(segment(x, y, (p, 0.0), (p, 1.0)));
    d = d.min(segment(x, y, (p, 1.0), (p + 0.24, 1.0)));
    d = d.min(segment(x, y, (p, 0.46), (p + 0.24, 0.46)));
    d = d.min(right_arc(x, y, (p + 0.24, 0.73), 0.27));
    // D
    let dd = 0.78;
    d = d.min(segment(x, y, (dd, 0.0), (dd, 1.0)));
    d = d.min(segment(x, y, (dd, 1.0), (dd + 0.2, 1.0)));
    d = d.min(segment(x, y, (dd, 0.0), (dd + 0.2, 0.0)));
    d = d.min(right_arc(x, y, (dd + 0.2, 0.5), 0.5));
    // F
    let f = 1.72;
    d = d.min(segment(x, y, (f, 0.0), (f, 1.0)));
    d = d.min(segment(x, y, (f, 1.0), (f + 0.5, 1.0)));
    d = d.min(segment(x, y, (f, 0.52), (f + 0.42, 0.52)));
    d
}

/// The colour of the icon at `(x, y)`, both in 0..1 of the icon's side;
/// `px` is a pixel of the layer being drawn, in the same units, so that
/// edges are smoothed over about one pixel whatever the layer.
fn sample(x: f32, y: f32, px: f32) -> Rgba {
    let aa = px * 0.7;
    let edge = |d: f32| smoothstep(aa, -aa, d);
    let small = px > 1.0 / 28.0;
    let mut c = [0.0; 4];

    // The sheet: from (0.17, 0.05) to (0.83, 0.95), the top right corner
    // folded over a FOLD-sized square.
    const L: f32 = 0.17;
    const R: f32 = 0.83;
    const T: f32 = 0.05;
    const B: f32 = 0.95;
    const FOLD: f32 = 0.2;
    let sheet = rounded_box(x, y, (L, T), (R, B), 0.035);
    // The corner cut off beyond the diagonal from (R − FOLD, T) to
    // (R, T + FOLD): the signed distance to that line, positive in the cut
    // corner, so the sheet is what is inside the box and not in the corner.
    let cut = ((x - (R - FOLD)) - (y - T) - FOLD) / std::f32::consts::SQRT_2;
    let body = sheet.max(cut);

    // Soft shadow under the sheet, offset down.
    let shadow_d = rounded_box(x, y - 0.025, (L, T), (R, B), 0.05).max(cut - 0.02);
    let shadow = smoothstep(0.05, -0.02, shadow_d) * 0.28;
    c = over(c, with_alpha(hex(0x000000), shadow));

    // Paper: light from the top.
    let paper = mix(hex(0xffffff), hex(0xeceef1), (y - T) / (B - T));
    c = over(c, with_alpha(paper, edge(body)));

    // The fold: the triangle turned over, a little darker, lit at its edge.
    let flap_a = (R - FOLD, T);
    let flap_b = (R, T + FOLD);
    let flap_c = (R - FOLD, T + FOLD);
    let flap = triangle(x, y, flap_a, flap_b, flap_c);
    let flap_color = mix(hex(0xf4f5f7), hex(0xcfd3d9), (x - (R - FOLD)) / FOLD);
    c = over(c, with_alpha(flap_color, edge(flap.max(-0.0))));
    // A faint line along the fold.
    let crease = segment(x, y, flap_a, flap_b) - px * 0.4;
    c = over(c, with_alpha(hex(0xb8bdc5), edge(crease) * 0.8));

    // Lines of text, hidden on the small layers.
    if !small {
        let line = |y0: f32, x1: f32| rounded_box(x, y, (0.27, y0), (x1, y0 + 0.036), 0.018);
        for (y0, x1) in [(0.30, 0.62), (0.385, 0.73), (0.47, 0.70)] {
            c = over(c, with_alpha(hex(0xb4bac3), edge(line(y0, x1))));
        }
    }

    // The red label across the lower part, a little past the sheet's left
    // edge, as a tag.
    let (label_l, label_r, label_t, label_b) = (0.09, 0.78, 0.58, 0.82);
    let label = rounded_box(x, y, (label_l, label_t), (label_r, label_b), 0.045);
    let label_shadow = rounded_box(x, y - 0.015, (label_l, label_t), (label_r, label_b), 0.05);
    c = over(c, with_alpha(hex(0x000000), smoothstep(0.03, -0.01, label_shadow) * 0.25 * edge(body.min(label_shadow))));
    let red = mix(hex(0xe8453a), hex(0xc0281f), (y - label_t) / (label_b - label_t));
    c = over(c, with_alpha(red, edge(label)));

    if !small {
        // "PDF" in white strokes, centred on the label.
        let height = 0.13;
        let total = 2.72 * height;
        let x0 = (label_l + label_r) / 2.0 - total / 2.0 + 0.012;
        let y0 = (label_t + label_b) / 2.0 + height / 2.0;
        let lx = (x - x0) / height;
        let ly = (y0 - y) / height;
        let stroke = pdf_strokes(lx, ly) * height - 0.021;
        c = over(c, with_alpha(hex(0xffffff), edge(stroke)));
    }
    c
}

/// Signed distance to the triangle `a b c` (negative inside).
fn triangle(x: f32, y: f32, a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> f32 {
    let side = |p: (f32, f32), q: (f32, f32)| (q.0 - p.0) * (y - p.1) - (q.1 - p.1) * (x - p.0);
    let (s1, s2, s3) = (side(a, b), side(b, c), side(c, a));
    let inside = (s1 >= 0.0 && s2 >= 0.0 && s3 >= 0.0) || (s1 <= 0.0 && s2 <= 0.0 && s3 <= 0.0);
    let d = segment(x, y, a, b).min(segment(x, y, b, c)).min(segment(x, y, c, a));
    if inside { -d } else { d }
}

/// Straight (non-premultiplied) RGBA pixels, `size * size * 4` bytes, row
/// major from the top. 4x4 supersampling gives smooth edges.
pub fn rgba(size: u32) -> Vec<u8> {
    const SS: u32 = 4;
    let n = size as usize;
    let px = 1.0 / size as f32;
    let mut out = vec![0u8; n * n * 4];
    for py in 0..size {
        for pxi in 0..size {
            // Averaged premultiplied, so transparent samples add no colour.
            let mut acc = [0.0f32; 4];
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (pxi as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                    let c = sample(x, y, px);
                    for i in 0..3 {
                        acc[i] += c[i] * c[3];
                    }
                    acc[3] += c[3];
                }
            }
            let count = (SS * SS) as f32;
            let alpha = acc[3] / count;
            let i = (py as usize * n + pxi as usize) * 4;
            if alpha > 0.0 {
                for ch in 0..3 {
                    out[i + ch] = (acc[ch] / acc[3] * 255.0).round().clamp(0.0, 255.0) as u8;
                }
                out[i + 3] = (alpha * 255.0).round() as u8;
            }
        }
    }
    out
}

/// Encode the icon as a multi-resolution `.ico` (32-bit BMP entries).
pub fn ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| bmp_entry(s)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]); // reserved, type = icon
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&s, img) in sizes.iter().zip(&images) {
        let dim = if s >= 256 { 0 } else { s as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bpp
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend_from_slice(&img);
    }
    out
}

/// BITMAPINFOHEADER + bottom-up BGRA pixels + 1-bpp AND mask.
fn bmp_entry(size: u32) -> Vec<u8> {
    let px = rgba(size);
    let n = size as usize;
    let mask_stride = n.div_ceil(32) * 4;
    let mut out = Vec::with_capacity(40 + n * n * 4 + mask_stride * n);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    out.extend_from_slice(&(2 * size as i32).to_le_bytes()); // XOR + AND
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]); // compression .. colours important
    for y in (0..n).rev() {
        for x in 0..n {
            let i = (y * n + x) * 4;
            out.extend_from_slice(&[px[i + 2], px[i + 1], px[i], px[i + 3]]);
        }
    }
    // Alpha channel carries transparency; an all-zero AND mask is correct.
    out.resize(out.len() + mask_stride * n, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_shape() {
        let px = rgba(64);
        assert_eq!(px.len(), 64 * 64 * 4);
        let at = |x: usize, y: usize| &px[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
        // The corners are transparent, the folded corner too.
        assert_eq!(at(0, 0)[3], 0);
        assert_eq!(at(63, 0)[3], 0);
        assert_eq!(at(61, 3)[3], 0, "the folded corner is cut off");
        // The sheet is white, the label red, the letters white on it.
        let paper = at(32, 12);
        assert!(paper[3] == 255 && paper[0] > 240 && paper[2] > 240, "{paper:?}");
        let red = at(10, 45);
        assert!(red[3] == 255 && red[0] > 180 && red[1] < 90, "{red:?}");
        let ico = ico(&[16, 32]);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 2, 0]);
    }
}
