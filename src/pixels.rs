//! Pixels on their way from the engine to the GPU: BGRx rows as PDFium
//! paints them, with the mip levels the engine builds (a 2×2 box filter,
//! about 1.5 ms for a 3.7-megapixel page), so that the UI thread only hands
//! them to the driver (`textures.rs`) and a page drawn smaller than its
//! render (while zooming out, in the sidebar) does not shimmer.

/// An image with its mip levels: level 0 first, each next one half the
/// size (rounded up), down to 1 × 1. Every level is BGRx, 4 bytes a pixel,
/// rows packed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub levels: Vec<Vec<u8>>,
}

impl Pixels {
    /// Level 0 from `bgrx` (`width * height * 4` bytes), with the levels
    /// below it built here.
    pub fn with_levels(width: u32, height: u32, bgrx: Vec<u8>) -> Pixels {
        debug_assert_eq!(bgrx.len(), width as usize * height as usize * 4);
        let mut levels = vec![bgrx];
        let (mut w, mut h) = (width, height);
        while w > 1 || h > 1 {
            let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
            let next = half(levels.last().expect("level 0"), w, h, nw, nh);
            levels.push(next);
            (w, h) = (nw, nh);
        }
        Pixels { width, height, levels }
    }

    /// Only level 0, no mips (a preview that is never drawn smaller).
    pub fn flat(width: u32, height: u32, bgrx: Vec<u8>) -> Pixels {
        Pixels { width, height, levels: vec![bgrx] }
    }

    /// The size of level `level` (each level halves the one above, rounded
    /// up).
    pub fn level_size(&self, level: usize) -> (u32, u32) {
        let (mut w, mut h) = (self.width, self.height);
        for _ in 0..level {
            (w, h) = (w.div_ceil(2), h.div_ceil(2));
        }
        (w.max(1), h.max(1))
    }

    /// The pixels of the first level no wider than `max_width` (the last
    /// level if none is), as an image of their own: a preview taken from a
    /// full render instead of a render of its own.
    pub fn level_at_most(&self, max_width: u32) -> Pixels {
        let level = (0..self.levels.len()).find(|&l| self.level_size(l).0 <= max_width).unwrap_or(self.levels.len() - 1);
        let (w, h) = self.level_size(level);
        Pixels { width: w, height: h, levels: self.levels[level..].to_vec() }
    }

    /// Bytes of all the levels.
    pub fn bytes(&self) -> usize {
        self.levels.iter().map(Vec::len).sum()
    }
}

/// `src` (`w` × `h`) halved into `nw` × `nh` by averaging 2×2 blocks (an
/// odd last row or column is averaged with itself).
fn half(src: &[u8], w: u32, h: u32, nw: u32, nh: u32) -> Vec<u8> {
    let (w, h, nw, nh) = (w as usize, h as usize, nw as usize, nh as usize);
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        let (y0, y1) = (2 * y, (2 * y + 1).min(h - 1));
        let (row0, row1) = (&src[y0 * w * 4..(y0 + 1) * w * 4], &src[y1 * w * 4..(y1 + 1) * w * 4]);
        let dst = &mut out[y * nw * 4..(y + 1) * nw * 4];
        for x in 0..nw {
            let (x0, x1) = (2 * x * 4, (2 * x + 1).min(w - 1) * 4);
            for c in 0..4 {
                let sum = u32::from(row0[x0 + c]) + u32::from(row0[x1 + c]) + u32::from(row1[x0 + c]) + u32::from(row1[x1 + c]);
                dst[x * 4 + c] = ((sum + 2) / 4) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_halve_down_to_one_pixel() {
        // 3 × 2: an odd width.
        let row0 = [0u8, 0, 0, 255, 100, 100, 100, 255, 200, 200, 200, 255];
        let row1 = [50u8, 50, 50, 255, 150, 150, 150, 255, 250, 250, 250, 255];
        let mut bgrx = row0.to_vec();
        bgrx.extend_from_slice(&row1);
        let p = Pixels::with_levels(3, 2, bgrx);
        assert_eq!(p.levels.len(), 3);
        assert_eq!(p.level_size(1), (2, 1));
        assert_eq!(p.level_size(2), (1, 1));
        // The first block averages 0, 100, 50, 150 = 75; the odd column
        // averages 200 and 250 with themselves = 225.
        assert_eq!(&p.levels[1][..4], &[75, 75, 75, 255]);
        assert_eq!(&p.levels[1][4..8], &[225, 225, 225, 255]);
        assert_eq!(p.level_at_most(2).width, 2);
        assert_eq!(p.level_at_most(1).levels.len(), 1);
        assert_eq!(p.bytes(), 24 + 8 + 4);
    }
}
