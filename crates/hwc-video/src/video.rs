//! Bink 1 video decoding (revisions `f` through `k`; the older `b` coding is not supported).
//!
//! A frame is coded plane by plane (alpha if present, luma, then the two half-resolution chroma
//! planes). Each plane is split into 8×8 blocks. Per-block parameters travel in nine "bundles"
//! (block types, sub-block types, colours, patterns, motion x/y, intra/inter DC values and run
//! lengths) which are refilled one block row at a time from the bitstream; block bodies (DCT
//! coefficients and residues) are read inline as the blocks are reconstructed. Each plane ends
//! on a 32-bit boundary.

use std::sync::OnceLock;

use crate::bits::BitReader;
use crate::container::{BinkFile, Header};
use crate::tables::{DCT_SCAN, HUFF_TREES, INTER_QUANT, INTRA_QUANT, RUN_SCANS};
use crate::{Error, Result};

// Block types.
const SKIP: u8 = 0;
const SCALED: u8 = 1;
const MOTION: u8 = 2;
const RUN: u8 = 3;
const RESIDUE: u8 = 4;
const INTRA: u8 = 5;
const FILL: u8 = 6;
const INTER: u8 = 7;
const PATTERN: u8 = 8;
const RAW: u8 = 9;

// Bundle slots, in the order their headers appear in the bitstream.
const B_BLOCK_TYPES: usize = 0;
const B_SUB_BLOCK_TYPES: usize = 1;
const B_COLORS: usize = 2;
const B_PATTERN: usize = 3;
const B_X_OFF: usize = 4;
const B_Y_OFF: usize = 5;
const B_INTRA_DC: usize = 6;
const B_INTER_DC: usize = 7;
const B_RUN: usize = 8;
const BUNDLE_COUNT: usize = 9;

/// Bit width of the first DC value in a DC bundle row.
const DC_START_BITS: u32 = 11;
/// Run lengths for block-type symbols 12..=15.
const BLOCK_TYPE_RUNS: [usize; 4] = [4, 8, 12, 32];

const HUFF_PEEK: u32 = 7;

/// Per-codebook lookup: indexed by the next 7 bits, gives (symbol slot, code length).
fn huff_lookup() -> &'static [[(u8, u8); 1 << HUFF_PEEK]; 16] {
    static TABLES: OnceLock<[[(u8, u8); 1 << HUFF_PEEK]; 16]> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut out = [[(0u8, 0u8); 1 << HUFF_PEEK]; 16];
        for (table, codes) in out.iter_mut().zip(HUFF_TREES.iter()) {
            for (slot, &(code, len)) in codes.iter().enumerate() {
                let step = 1usize << len;
                let mut i = code as usize;
                while i < table.len() {
                    table[i] = (slot as u8, len);
                    i += step;
                }
            }
        }
        out
    })
}

/// A codebook choice plus the symbol permutation sent with it.
#[derive(Clone, Copy)]
struct Tree {
    codebook: u8,
    symbols: [u8; 16],
}

impl Default for Tree {
    fn default() -> Self {
        Self {
            codebook: 0,
            symbols: std::array::from_fn(|i| i as u8),
        }
    }
}

impl Tree {
    fn read(br: &mut BitReader) -> Tree {
        let mut tree = Tree {
            codebook: br.read(4) as u8,
            ..Tree::default()
        };
        if tree.codebook == 0 {
            return tree;
        }
        if br.bit() {
            // An explicit list of leading symbols, the unused ones following in order.
            let mut used = [false; 16];
            let count = br.read(3) as usize + 1;
            for i in 0..count {
                let s = br.read(4) as u8;
                tree.symbols[i] = s;
                used[s as usize] = true;
            }
            let mut n = count;
            for s in 0..16u8 {
                if n >= 16 {
                    break;
                }
                if !used[s as usize] {
                    tree.symbols[n] = s;
                    n += 1;
                }
            }
        } else {
            // A bitwise merge sort of the identity order, `passes` levels deep.
            let passes = br.read(2) as usize + 1;
            let mut cur: [u8; 16] = std::array::from_fn(|i| i as u8);
            let mut next = [0u8; 16];
            for level in 0..passes {
                let size = 1usize << level;
                let mut start = 0;
                while start < 16 {
                    merge_runs(
                        br,
                        &cur[start..start + 2 * size],
                        &mut next[start..start + 2 * size],
                        size,
                    );
                    start += 2 * size;
                }
                std::mem::swap(&mut cur, &mut next);
            }
            tree.symbols = cur;
        }
        tree
    }

    #[inline(always)]
    fn decode(&self, br: &mut BitReader) -> u8 {
        let (slot, len) = huff_lookup()[self.codebook as usize][br.peek(HUFF_PEEK) as usize];
        br.skip(len as usize);
        self.symbols[slot as usize]
    }
}

/// Merges the two `size`-long halves of `src` into `dst`, one bit choosing each element.
fn merge_runs(br: &mut BitReader, src: &[u8], dst: &mut [u8], size: usize) {
    let (mut a, mut b) = (0usize, size);
    let mut out = 0;
    while a < size && b < 2 * size {
        if br.bit() {
            dst[out] = src[b];
            b += 1;
        } else {
            dst[out] = src[a];
            a += 1;
        }
        out += 1;
    }
    while a < size {
        dst[out] = src[a];
        a += 1;
        out += 1;
    }
    while b < 2 * size {
        dst[out] = src[b];
        b += 1;
        out += 1;
    }
}

/// One parameter stream. Values are appended a row at a time and consumed by the blocks.
#[derive(Default)]
struct Bundle {
    count_bits: u32,
    tree: Tree,
    data: Vec<i16>,
    read: usize,
    write: usize,
    ended: bool,
}

impl Bundle {
    fn reset(&mut self, capacity: usize) {
        if self.data.len() < capacity {
            self.data.resize(capacity, 0);
        }
        self.read = 0;
        self.write = 0;
        self.ended = false;
    }

    /// Reads the value count for the next chunk, if this bundle wants one now: only when all
    /// previously decoded values have been used up, and never after a zero count.
    fn begin(&mut self, br: &mut BitReader) -> Result<Option<usize>> {
        if self.ended || self.write > self.read {
            return Ok(None);
        }
        let count = br.read(self.count_bits) as usize;
        if count == 0 {
            self.ended = true;
            return Ok(None);
        }
        if self.write + count > self.data.len() {
            return Err(Error::Invalid("bundle overflow"));
        }
        Ok(Some(count))
    }

    #[inline(always)]
    fn push(&mut self, v: i16) {
        self.data[self.write] = v;
        self.write += 1;
    }

    #[inline(always)]
    fn next(&mut self) -> Result<i32> {
        if self.read >= self.write {
            return Err(Error::Invalid("bundle underrun"));
        }
        let v = self.data[self.read];
        self.read += 1;
        Ok(v as i32)
    }
}

/// One image plane, sized to whole 8×8 blocks plus a margin for 16×16 blocks at the edges.
#[derive(Clone)]
struct Plane {
    width: usize,
    stride: usize,
    blocks_w: usize,
    blocks_h: usize,
    data: Vec<u8>,
}

impl Plane {
    fn new(width: usize, blocks_w: usize, blocks_h: usize, fill: u8) -> Self {
        let stride = blocks_w * 8;
        Self {
            width,
            stride,
            blocks_w,
            blocks_h,
            data: vec![fill; stride * (blocks_h * 8 + 16) + 16],
        }
    }
}

/// A decoded picture in its native planar YUV 4:2:0 form.
#[derive(Clone, Copy)]
pub struct YuvFrame<'a> {
    pub width: usize,
    pub height: usize,
    pub y: &'a [u8],
    pub y_stride: usize,
    pub u: &'a [u8],
    pub v: &'a [u8],
    pub uv_stride: usize,
    /// Alpha plane (same layout as `y`) when the movie has one.
    pub alpha: Option<&'a [u8]>,
}

/// Stateful video decoder; feed it every frame's video packet in order.
pub struct VideoDecoder {
    revision: u8,
    width: usize,
    height: usize,
    has_alpha: bool,
    grayscale: bool,
    /// Planes being built (Y, U, V, A).
    cur: Vec<Plane>,
    /// The previous picture, source of skipped and motion-compensated blocks.
    prev: Vec<Plane>,
    bundles: [Bundle; BUNDLE_COUNT],
    color_high: [Tree; 16],
    color_last: usize,
    frames_decoded: u64,
}

impl VideoDecoder {
    pub fn new(header: &Header) -> Result<Self> {
        let revision = header.revision;
        if revision == b'b' {
            return Err(Error::Unsupported("Bink revision b"));
        }
        if !(b'c'..=b'k').contains(&revision) {
            return Err(Error::Unsupported("unknown Bink revision"));
        }
        let width = header.width as usize;
        let height = header.height as usize;
        let luma = |fill| Plane::new(width, width.div_ceil(8), height.div_ceil(8), fill);
        let chroma = |fill| Plane::new(width / 2, (width + 15) >> 4, (height + 15) >> 4, fill);
        let planes = vec![luma(0), chroma(0), chroma(0), luma(255)];
        Ok(Self {
            revision,
            width,
            height,
            has_alpha: header.has_alpha(),
            grayscale: header.is_grayscale(),
            cur: planes.clone(),
            prev: planes,
            bundles: Default::default(),
            color_high: [Tree::default(); 16],
            color_last: 0,
            frames_decoded: 0,
        })
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Number of frames decoded so far.
    pub fn frames_decoded(&self) -> u64 {
        self.frames_decoded
    }

    /// Clears the reference picture (to black), e.g. before restarting at a keyframe.
    pub fn reset(&mut self) {
        for (i, p) in self.prev.iter_mut().enumerate() {
            p.data.fill(if i == 3 { 255 } else { 0 });
        }
        self.frames_decoded = 0;
    }

    /// Decodes one frame's video packet; the result is then available from [`Self::yuv`].
    pub fn decode(&mut self, packet: &[u8]) -> Result<()> {
        let mut br = BitReader::new(packet);
        let skip_header = self.revision >= b'i';
        // From revision `h` on, the second coded chroma plane is U rather than V.
        let swap_chroma = self.revision >= b'h';

        let mut done = [false; 4];
        if self.has_alpha {
            if skip_header {
                br.skip(32);
            }
            self.decode_plane(&mut br, 3)?;
            done[3] = true;
        }
        if skip_header {
            br.skip(32);
        }
        let plane_count = if self.grayscale { 1 } else { 3 };
        for coded in 0..plane_count {
            let index = if coded > 0 && swap_chroma {
                3 - coded
            } else {
                coded
            };
            self.decode_plane(&mut br, index)?;
            done[index] = true;
            if br.position() >= br.len_bits() {
                break;
            }
        }
        if br.overrun() {
            return Err(Error::Truncated);
        }
        for (i, finished) in done.iter().enumerate() {
            if !finished {
                // Planes not coded in this packet keep the previous picture.
                let (cur, prev) = (&mut self.cur[i], &self.prev[i]);
                if self.grayscale && (i == 1 || i == 2) {
                    cur.data.fill(128);
                } else {
                    cur.data.copy_from_slice(&prev.data);
                }
            }
        }
        std::mem::swap(&mut self.cur, &mut self.prev);
        self.frames_decoded += 1;
        Ok(())
    }

    /// The most recently decoded picture.
    pub fn yuv(&self) -> YuvFrame<'_> {
        YuvFrame {
            width: self.width,
            height: self.height,
            y: &self.prev[0].data,
            y_stride: self.prev[0].stride,
            u: &self.prev[1].data,
            v: &self.prev[2].data,
            uv_stride: self.prev[1].stride,
            alpha: self.has_alpha.then(|| &self.prev[3].data[..]),
        }
    }

    /// Converts the most recent picture to RGBA8 (`width * height * 4` bytes).
    pub fn to_rgba(&self, out: &mut [u8]) {
        yuv_to_rgba(&self.yuv(), out);
    }

    fn read_bundle_headers(&mut self, br: &mut BitReader) {
        for (i, bundle) in self.bundles.iter_mut().enumerate() {
            if i == B_COLORS {
                for t in self.color_high.iter_mut() {
                    *t = Tree::read(br);
                }
                self.color_last = 0;
            }
            if i != B_INTRA_DC && i != B_INTER_DC {
                bundle.tree = Tree::read(br);
            }
            bundle.read = 0;
            bundle.write = 0;
            bundle.ended = false;
        }
    }

    fn decode_plane(&mut self, br: &mut BitReader, index: usize) -> Result<()> {
        let (blocks_w, blocks_h, plane_width) = {
            let p = &self.cur[index];
            (p.blocks_w, p.blocks_h, p.width)
        };
        let blocks = blocks_w * blocks_h;

        // Bit widths of the per-row value counts depend on the plane width.
        let width = (plane_width.max(8) + 7) & !7;
        let log2 = |x: usize| (usize::BITS - 1 - x.leading_zeros()) + 1;
        let count_bits = [
            log2((width >> 3) + 511),
            log2((width >> 4) + 511),
            log2(blocks_w * 64 + 511),
            log2((blocks_w << 3) + 511),
            log2((width >> 3) + 511),
            log2((width >> 3) + 511),
            log2((width >> 3) + 511),
            log2((width >> 3) + 511),
            log2(blocks_w * 48 + 511),
        ];
        let capacity = [
            blocks + 64,
            blocks + 64,
            blocks * 64 + 64,
            blocks * 8 + 64,
            blocks + 64,
            blocks + 64,
            blocks + 64,
            blocks + 64,
            blocks * 64 + 64,
        ];
        for (i, b) in self.bundles.iter_mut().enumerate() {
            b.count_bits = count_bits[i];
            b.reset(capacity[i]);
        }
        self.read_bundle_headers(br);

        let mut cur = std::mem::replace(&mut self.cur[index], Plane::new(0, 0, 0, 0));
        let prev = std::mem::take(&mut self.prev[index].data);
        let result = self.decode_blocks(br, &mut cur, &prev);
        self.cur[index] = cur;
        self.prev[index].data = prev;
        result?;
        br.align32();
        Ok(())
    }

    fn decode_blocks(&mut self, br: &mut BitReader, cur: &mut Plane, prev: &[u8]) -> Result<()> {
        let stride = cur.stride;
        let (blocks_w, blocks_h) = (cur.blocks_w, cur.blocks_h);
        let ref_limit = ((blocks_h - 1) * 8 * stride + (blocks_w - 1) * 8) as isize;
        let new_colors = self.revision >= b'i';

        let mut coeffs = [0i32; 64];
        let mut coef_order = [0usize; 64];
        let mut small = [0u8; 64];

        for by in 0..blocks_h {
            self.fill_block_types(br, B_BLOCK_TYPES)?;
            self.fill_block_types(br, B_SUB_BLOCK_TYPES)?;
            self.fill_colors(br, new_colors)?;
            self.fill_patterns(br)?;
            self.fill_motion(br, B_X_OFF)?;
            self.fill_motion(br, B_Y_OFF)?;
            self.fill_dcs(br, B_INTRA_DC, false)?;
            self.fill_dcs(br, B_INTER_DC, true)?;
            self.fill_runs(br)?;

            let row = by * 8 * stride;
            let mut bx = 0;
            while bx < blocks_w {
                let at = row + bx * 8;
                let kind = self.bundles[B_BLOCK_TYPES].next()? as u8;
                // On odd rows a scaled block marks the lower half of the one decoded above.
                if by & 1 == 1 && kind == SCALED {
                    bx += 2;
                    continue;
                }
                match kind {
                    SKIP => copy_block(&mut cur.data, at, prev, at as isize, stride),
                    SCALED => {
                        let sub = self.bundles[B_SUB_BLOCK_TYPES].next()? as u8;
                        match sub {
                            RUN => {
                                self.run_block(br, &mut small, 0, 8)?;
                                scale_block(&small, &mut cur.data, at, stride);
                            }
                            INTRA => {
                                coeffs.fill(0);
                                coeffs[0] = self.bundles[B_INTRA_DC].next()?;
                                let (n, q) = read_dct_coeffs(br, &mut coeffs, &mut coef_order);
                                dequantize(&mut coeffs, &INTRA_QUANT[q], &coef_order[..n]);
                                let out = idct(&coeffs);
                                for (d, s) in small.iter_mut().zip(out.iter()) {
                                    *d = *s as u8;
                                }
                                scale_block(&small, &mut cur.data, at, stride);
                            }
                            FILL => {
                                let v = self.bundles[B_COLORS].next()? as u8;
                                for y in 0..16 {
                                    cur.data[at + y * stride..at + y * stride + 16].fill(v);
                                }
                            }
                            PATTERN => {
                                self.pattern_block(&mut small, 0, 8)?;
                                scale_block(&small, &mut cur.data, at, stride);
                            }
                            RAW => {
                                self.raw_block(&mut small, 0, 8)?;
                                scale_block(&small, &mut cur.data, at, stride);
                            }
                            _ => return Err(Error::Invalid("scaled sub-block type")),
                        }
                        bx += 2;
                        continue;
                    }
                    MOTION => {
                        let src = self.motion_source(by, bx, stride, ref_limit)?;
                        copy_block(&mut cur.data, at, prev, src, stride);
                    }
                    RUN => self.run_block(br, &mut cur.data, at, stride)?,
                    RESIDUE => {
                        let src = self.motion_source(by, bx, stride, ref_limit)?;
                        copy_block(&mut cur.data, at, prev, src, stride);
                        coeffs.fill(0);
                        let masks = br.read(7) as i32;
                        read_residue(br, &mut coeffs, masks);
                        add_block(&mut cur.data, at, stride, &coeffs);
                    }
                    INTRA => {
                        coeffs.fill(0);
                        coeffs[0] = self.bundles[B_INTRA_DC].next()?;
                        let (n, q) = read_dct_coeffs(br, &mut coeffs, &mut coef_order);
                        dequantize(&mut coeffs, &INTRA_QUANT[q], &coef_order[..n]);
                        let out = idct(&coeffs);
                        for y in 0..8 {
                            for x in 0..8 {
                                cur.data[at + y * stride + x] = out[y * 8 + x] as u8;
                            }
                        }
                    }
                    FILL => {
                        let v = self.bundles[B_COLORS].next()? as u8;
                        for y in 0..8 {
                            cur.data[at + y * stride..at + y * stride + 8].fill(v);
                        }
                    }
                    INTER => {
                        let src = self.motion_source(by, bx, stride, ref_limit)?;
                        copy_block(&mut cur.data, at, prev, src, stride);
                        coeffs.fill(0);
                        coeffs[0] = self.bundles[B_INTER_DC].next()?;
                        let (n, q) = read_dct_coeffs(br, &mut coeffs, &mut coef_order);
                        dequantize(&mut coeffs, &INTER_QUANT[q], &coef_order[..n]);
                        let out = idct(&coeffs);
                        add_block(&mut cur.data, at, stride, &out);
                    }
                    PATTERN => self.pattern_block(&mut cur.data, at, stride)?,
                    RAW => self.raw_block(&mut cur.data, at, stride)?,
                    _ => return Err(Error::Invalid("block type")),
                }
                bx += 1;
            }
        }
        Ok(())
    }

    /// Offset of the reference block for a motion-compensated block, checked against the
    /// reference plane the same way the original decoder bounds it.
    fn motion_source(
        &mut self,
        by: usize,
        bx: usize,
        stride: usize,
        limit: isize,
    ) -> Result<isize> {
        let dx = self.bundles[B_X_OFF].next()? as isize;
        let dy = self.bundles[B_Y_OFF].next()? as isize;
        let src = ((by * 8) as isize + dy) * stride as isize + (bx * 8) as isize + dx;
        if src < 0 || src > limit {
            return Err(Error::Invalid("motion vector out of frame"));
        }
        Ok(src)
    }

    fn run_block(
        &mut self,
        br: &mut BitReader,
        dst: &mut [u8],
        at: usize,
        stride: usize,
    ) -> Result<()> {
        let scan = &RUN_SCANS[br.read(4) as usize];
        let mut pos = 0usize;
        let put = |dst: &mut [u8], pos: usize, v: u8| {
            let p = scan[pos] as usize;
            dst[at + (p >> 3) * stride + (p & 7)] = v;
        };
        while pos < 63 {
            let run = self.bundles[B_RUN].next()? as usize + 1;
            if pos + run > 64 {
                return Err(Error::Invalid("run past end of block"));
            }
            if br.bit() {
                let v = self.bundles[B_COLORS].next()? as u8;
                for k in pos..pos + run {
                    put(dst, k, v);
                }
            } else {
                for k in pos..pos + run {
                    let v = self.bundles[B_COLORS].next()? as u8;
                    put(dst, k, v);
                }
            }
            pos += run;
        }
        if pos == 63 {
            let v = self.bundles[B_COLORS].next()? as u8;
            put(dst, 63, v);
        }
        Ok(())
    }

    fn pattern_block(&mut self, dst: &mut [u8], at: usize, stride: usize) -> Result<()> {
        let colors = [
            self.bundles[B_COLORS].next()? as u8,
            self.bundles[B_COLORS].next()? as u8,
        ];
        for y in 0..8 {
            let mut bits = self.bundles[B_PATTERN].next()? as u32;
            for x in 0..8 {
                dst[at + y * stride + x] = colors[(bits & 1) as usize];
                bits >>= 1;
            }
        }
        Ok(())
    }

    fn raw_block(&mut self, dst: &mut [u8], at: usize, stride: usize) -> Result<()> {
        for y in 0..8 {
            for x in 0..8 {
                dst[at + y * stride + x] = self.bundles[B_COLORS].next()? as u8;
            }
        }
        Ok(())
    }

    fn fill_block_types(&mut self, br: &mut BitReader, slot: usize) -> Result<()> {
        let b = &mut self.bundles[slot];
        let Some(count) = b.begin(br)? else {
            return Ok(());
        };
        let end = b.write + count;
        if br.bit() {
            let v = br.read(4) as i16;
            for _ in 0..count {
                b.push(v);
            }
        } else {
            let mut last = 0i16;
            while b.write < end {
                let v = b.tree.decode(br);
                if v < 12 {
                    last = v as i16;
                    b.push(last);
                } else {
                    let run = BLOCK_TYPE_RUNS[v as usize - 12];
                    if end - b.write < run {
                        return Err(Error::Invalid("block type run overflow"));
                    }
                    for _ in 0..run {
                        b.push(last);
                    }
                }
            }
        }
        Ok(())
    }

    fn fill_colors(&mut self, br: &mut BitReader, new_colors: bool) -> Result<()> {
        let b = &mut self.bundles[B_COLORS];
        let Some(count) = b.begin(br)? else {
            return Ok(());
        };
        let next_color = |br: &mut BitReader, last: &mut usize, tree: &Tree| -> i16 {
            *last = self.color_high[*last].decode(br) as usize;
            let v = ((*last as u8) << 4) | tree.decode(br);
            if new_colors {
                v as i16
            } else {
                // Older revisions store colours as sign-magnitude around 128.
                let sign = ((v as i8) >> 7) as i32;
                let mag = (v & 0x7f) as i32;
                (((mag ^ sign) - sign) + 0x80) as u8 as i16
            }
        };
        let tree = b.tree;
        let mut last = self.color_last;
        if br.bit() {
            let v = next_color(br, &mut last, &tree);
            for _ in 0..count {
                b.push(v);
            }
        } else {
            for _ in 0..count {
                let v = next_color(br, &mut last, &tree);
                b.push(v);
            }
        }
        self.color_last = last;
        Ok(())
    }

    fn fill_patterns(&mut self, br: &mut BitReader) -> Result<()> {
        let b = &mut self.bundles[B_PATTERN];
        let Some(count) = b.begin(br)? else {
            return Ok(());
        };
        for _ in 0..count {
            let lo = b.tree.decode(br) as i16;
            let hi = b.tree.decode(br) as i16;
            b.push(lo | (hi << 4));
        }
        Ok(())
    }

    fn fill_motion(&mut self, br: &mut BitReader, slot: usize) -> Result<()> {
        let b = &mut self.bundles[slot];
        let Some(count) = b.begin(br)? else {
            return Ok(());
        };
        let signed = |br: &mut BitReader, v: i16| if v != 0 && br.bit() { -v } else { v };
        if br.bit() {
            let v = br.read(4) as i16;
            let v = signed(br, v);
            for _ in 0..count {
                b.push(v);
            }
        } else {
            for _ in 0..count {
                let v = b.tree.decode(br) as i16;
                let v = signed(br, v);
                b.push(v);
            }
        }
        Ok(())
    }

    fn fill_dcs(&mut self, br: &mut BitReader, slot: usize, has_sign: bool) -> Result<()> {
        let b = &mut self.bundles[slot];
        let Some(count) = b.begin(br)? else {
            return Ok(());
        };
        let mut v = br.read(DC_START_BITS - has_sign as u32) as i32;
        if v != 0 && has_sign && br.bit() {
            v = -v;
        }
        b.push(v as i16);
        let rest = count - 1;
        let mut done = 0;
        while done < rest {
            let group = (rest - done).min(8);
            let bits = br.read(4);
            if bits != 0 {
                for _ in 0..group {
                    let mut d = br.read(bits) as i32;
                    if d != 0 && br.bit() {
                        d = -d;
                    }
                    v += d;
                    if !(-32768..=32767).contains(&v) {
                        return Err(Error::Invalid("DC out of range"));
                    }
                    b.push(v as i16);
                }
            } else {
                for _ in 0..group {
                    b.push(v as i16);
                }
            }
            done += group;
        }
        Ok(())
    }

    fn fill_runs(&mut self, br: &mut BitReader) -> Result<()> {
        let b = &mut self.bundles[B_RUN];
        let Some(count) = b.begin(br)? else {
            return Ok(());
        };
        if br.bit() {
            let v = br.read(4) as i16;
            for _ in 0..count {
                b.push(v);
            }
        } else {
            for _ in 0..count {
                let v = b.tree.decode(br) as i16;
                b.push(v);
            }
        }
        Ok(())
    }
}

/// Reads one coefficient magnitude of `bits` significant bits (the top bit implied) and sign.
#[inline(always)]
fn read_coeff(br: &mut BitReader, bits: u32) -> i32 {
    if bits == 0 {
        if br.bit() {
            -1
        } else {
            1
        }
    } else {
        let v = (br.read(bits) | (1 << bits)) as i32;
        if br.bit() {
            -v
        } else {
            v
        }
    }
}

// Work-list entry kinds shared by the DCT and residue coders.
const LIST_QUAD_START: u8 = 0;
const LIST_QUAD_SPLIT: u8 = 1;
const LIST_QUAD: u8 = 2;
const LIST_SINGLE: u8 = 3;

/// Reads the AC coefficients of a DCT block, coded by bit planes over a hierarchical list of
/// coefficient groups. Returns the number of coefficients set (their scan indices are stored in
/// `order`) and the quantiser index.
fn read_dct_coeffs(
    br: &mut BitReader,
    block: &mut [i32; 64],
    order: &mut [usize; 64],
) -> (usize, usize) {
    let mut coef = [0u8; 128];
    let mut mode = [0u8; 128];
    let mut start = 64usize;
    let mut end = 64usize;
    for (c, m) in [
        (4, LIST_QUAD_START),
        (24, LIST_QUAD_START),
        (44, LIST_QUAD_START),
        (1, LIST_SINGLE),
        (2, LIST_SINGLE),
        (3, LIST_SINGLE),
    ] {
        coef[end] = c;
        mode[end] = m;
        end += 1;
    }
    let mut count = 0usize;

    let planes = br.read(4) as i32;
    for bits in (0..planes).rev() {
        let bits = bits as u32;
        let mut pos = start;
        while pos < end {
            if (coef[pos] == 0 && mode[pos] == 0) || !br.bit() {
                pos += 1;
                continue;
            }
            let mut c = coef[pos];
            match mode[pos] {
                LIST_QUAD_START | LIST_QUAD => {
                    if mode[pos] == LIST_QUAD_START {
                        coef[pos] = c + 4;
                        mode[pos] = LIST_QUAD_SPLIT;
                    } else {
                        coef[pos] = 0;
                        mode[pos] = 0;
                        pos += 1;
                    }
                    for _ in 0..4 {
                        if br.bit() {
                            start -= 1;
                            coef[start] = c;
                            mode[start] = LIST_SINGLE;
                        } else {
                            let idx = (c & 63) as usize;
                            block[DCT_SCAN[idx] as usize] = read_coeff(br, bits);
                            order[count & 63] = idx;
                            count += 1;
                        }
                        c += 1;
                    }
                }
                LIST_QUAD_SPLIT => {
                    mode[pos] = LIST_QUAD;
                    for _ in 0..3 {
                        c += 4;
                        coef[end] = c;
                        mode[end] = LIST_QUAD;
                        end += 1;
                    }
                }
                _ => {
                    let idx = (c & 63) as usize;
                    block[DCT_SCAN[idx] as usize] = read_coeff(br, bits);
                    order[count & 63] = idx;
                    count += 1;
                    coef[pos] = 0;
                    mode[pos] = 0;
                    pos += 1;
                }
            }
        }
    }
    let quant = br.read(4) as usize;
    (count.min(64), quant)
}

fn dequantize(block: &mut [i32; 64], quant: &[i32; 64], order: &[usize]) {
    block[0] = block[0].wrapping_mul(quant[0]) >> 11;
    for &idx in order {
        let p = DCT_SCAN[idx] as usize;
        block[p] = block[p].wrapping_mul(quant[p]) >> 11;
    }
}

/// Reads a residue block: like the DCT coder's group list, but each bit plane first refines the
/// coefficients already found. At most `masks + 1` refinements/coefficients are read.
fn read_residue(br: &mut BitReader, block: &mut [i32; 64], mut masks: i32) {
    let mut coef = [0u8; 128];
    let mut mode = [0u8; 128];
    let mut start = 64usize;
    let mut end = 64usize;
    for (c, m) in [
        (4, LIST_QUAD_START),
        (24, LIST_QUAD_START),
        (44, LIST_QUAD_START),
        (0, LIST_QUAD),
    ] {
        coef[end] = c;
        mode[end] = m;
        end += 1;
    }
    let mut nonzero = [0usize; 64];
    let mut nz = 0usize;

    let mut mask = 1i32 << br.read(3);
    while mask != 0 {
        for &p in &nonzero[..nz] {
            if !br.bit() {
                continue;
            }
            if block[p] < 0 {
                block[p] -= mask;
            } else {
                block[p] += mask;
            }
            masks -= 1;
            if masks < 0 {
                return;
            }
        }
        let mut pos = start;
        while pos < end {
            if (coef[pos] == 0 && mode[pos] == 0) || !br.bit() {
                pos += 1;
                continue;
            }
            let mut c = coef[pos];
            match mode[pos] {
                LIST_QUAD_START | LIST_QUAD => {
                    if mode[pos] == LIST_QUAD_START {
                        coef[pos] = c + 4;
                        mode[pos] = LIST_QUAD_SPLIT;
                    } else {
                        coef[pos] = 0;
                        mode[pos] = 0;
                        pos += 1;
                    }
                    for _ in 0..4 {
                        if br.bit() {
                            start -= 1;
                            coef[start] = c;
                            mode[start] = LIST_SINGLE;
                        } else {
                            let p = DCT_SCAN[(c & 63) as usize] as usize;
                            nonzero[nz & 63] = p;
                            nz = (nz + 1).min(64);
                            block[p] = if br.bit() { -mask } else { mask };
                            masks -= 1;
                            if masks < 0 {
                                return;
                            }
                        }
                        c += 1;
                    }
                }
                LIST_QUAD_SPLIT => {
                    mode[pos] = LIST_QUAD;
                    for _ in 0..3 {
                        c += 4;
                        coef[end] = c;
                        mode[end] = LIST_QUAD;
                        end += 1;
                    }
                }
                _ => {
                    let p = DCT_SCAN[(c & 63) as usize] as usize;
                    nonzero[nz & 63] = p;
                    nz = (nz + 1).min(64);
                    block[p] = if br.bit() { -mask } else { mask };
                    coef[pos] = 0;
                    mode[pos] = 0;
                    pos += 1;
                    masks -= 1;
                    if masks < 0 {
                        return;
                    }
                }
            }
        }
        mask >>= 1;
    }
}

// Fixed-point constants of the 8-point inverse DCT (AAN factorisation), 11 fractional bits.
const C_SQRT2: i32 = 2896; // sqrt(2)
const C_A: i32 = 2217; // 2 * sqrt(2) * cos(3*pi/8)
const C_B: i32 = 3784; // 2 * cos(pi/8)
const C_C: i32 = -5352; // -2 * sqrt(2) * cos(pi/8)

#[inline(always)]
fn idct8(s: [i32; 8]) -> [i32; 8] {
    let e0 = s[0] + s[4];
    let e1 = s[0] - s[4];
    let e2 = s[2] + s[6];
    let e3 = (C_SQRT2 * (s[2] - s[6])) >> 11;
    let o4 = s[5] + s[3];
    let o5 = s[5] - s[3];
    let o6 = s[1] + s[7];
    let o7 = s[1] - s[7];
    let t0 = o4 + o6;
    let t1 = (C_B * (o5 + o7)) >> 11;
    let t2 = ((C_C * o5) >> 11) - t0 + t1;
    let t3 = ((C_SQRT2 * (o6 - o4)) >> 11) - t2;
    let t4 = ((C_A * o7) >> 11) + t3 - t1;
    [
        e0 + e2 + t0,
        e1 + e3 - e2 + t2,
        e1 - e3 + e2 + t3,
        e0 - e2 - t4,
        e0 - e2 + t4,
        e1 - e3 + e2 - t3,
        e1 + e3 - e2 - t2,
        e0 + e2 - t0,
    ]
}

/// 2-D inverse DCT: columns first (unscaled), then rows with rounding and an 8-bit shift.
fn idct(block: &[i32; 64]) -> [i32; 64] {
    let mut tmp = [0i32; 64];
    for x in 0..8 {
        let col: [i32; 8] = std::array::from_fn(|y| block[y * 8 + x]);
        let out = if col[1..].iter().all(|&v| v == 0) {
            [col[0]; 8]
        } else {
            idct8(col)
        };
        for y in 0..8 {
            tmp[y * 8 + x] = out[y];
        }
    }
    let mut res = [0i32; 64];
    for y in 0..8 {
        let row: [i32; 8] = std::array::from_fn(|x| tmp[y * 8 + x]);
        let out = idct8(row);
        for x in 0..8 {
            res[y * 8 + x] = (out[x] + 0x7f) >> 8;
        }
    }
    res
}

#[inline(always)]
fn copy_block(dst: &mut [u8], at: usize, src: &[u8], from: isize, stride: usize) {
    let from = from as usize;
    for y in 0..8 {
        let d = at + y * stride;
        let s = from + y * stride;
        dst[d..d + 8].copy_from_slice(&src[s..s + 8]);
    }
}

#[inline(always)]
fn add_block(dst: &mut [u8], at: usize, stride: usize, block: &[i32; 64]) {
    for y in 0..8 {
        for x in 0..8 {
            let p = &mut dst[at + y * stride + x];
            *p = p.wrapping_add(block[y * 8 + x] as u8);
        }
    }
}

/// Writes an 8×8 block doubled in both directions.
fn scale_block(src: &[u8; 64], dst: &mut [u8], at: usize, stride: usize) {
    for y in 0..8 {
        let row = &src[y * 8..y * 8 + 8];
        for dy in 0..2 {
            let d = at + (2 * y + dy) * stride;
            for (x, &v) in row.iter().enumerate() {
                dst[d + 2 * x] = v;
                dst[d + 2 * x + 1] = v;
            }
        }
    }
}

/// BT.601 limited-range YUV 4:2:0 to RGBA8 (alpha from the alpha plane or opaque).
pub fn yuv_to_rgba(f: &YuvFrame, out: &mut [u8]) {
    assert!(out.len() >= f.width * f.height * 4);
    for y in 0..f.height {
        let yrow = &f.y[y * f.y_stride..];
        let crow = (y >> 1) * f.uv_stride;
        let urow = &f.u[crow..];
        let vrow = &f.v[crow..];
        let orow = &mut out[y * f.width * 4..(y + 1) * f.width * 4];
        for x in 0..f.width {
            let c = (yrow[x] as i32 - 16) * 76309;
            let u = urow[x >> 1] as i32 - 128;
            let v = vrow[x >> 1] as i32 - 128;
            let r = (c + 104597 * v + 32768) >> 16;
            let g = (c - 25675 * u - 53279 * v + 32768) >> 16;
            let b = (c + 132201 * u + 32768) >> 16;
            let px = &mut orow[x * 4..x * 4 + 4];
            px[0] = r.clamp(0, 255) as u8;
            px[1] = g.clamp(0, 255) as u8;
            px[2] = b.clamp(0, 255) as u8;
            px[3] = f.alpha.map_or(255, |a| a[y * f.y_stride + x]);
        }
    }
}

/// A decoded frame converted to RGBA.
#[derive(Clone)]
pub struct Frame {
    pub index: u32,
    /// Presentation time in seconds.
    pub time: f64,
    pub keyframe: bool,
    pub width: usize,
    pub height: usize,
    /// `width * height * 4` bytes, rows top to bottom.
    pub rgba: Vec<u8>,
}

/// Iterator over every frame of a movie, decoded to RGBA. See [`BinkFile::frames`].
pub struct Frames<'a> {
    file: &'a BinkFile,
    decoder: VideoDecoder,
    next: u32,
    failed: bool,
}

impl<'a> Frames<'a> {
    pub(crate) fn new(file: &'a BinkFile) -> Result<Self> {
        Ok(Self {
            file,
            decoder: VideoDecoder::new(file.header())?,
            next: 0,
            failed: false,
        })
    }

    /// The underlying decoder (for YUV access to the last frame).
    pub fn decoder(&self) -> &VideoDecoder {
        &self.decoder
    }
}

impl Iterator for Frames<'_> {
    type Item = Result<Frame>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.next >= self.file.frame_count() {
            return None;
        }
        let index = self.next;
        self.next += 1;
        let result = self.file.packet(index).and_then(|packet| {
            self.decoder.decode(packet.video)?;
            let (w, h) = (self.decoder.width(), self.decoder.height());
            let mut rgba = vec![0u8; w * h * 4];
            self.decoder.to_rgba(&mut rgba);
            Ok(Frame {
                index,
                time: self.file.header().frame_time(index),
                keyframe: packet.keyframe,
                width: w,
                height: h,
                rgba,
            })
        });
        if result.is_err() {
            self.failed = true;
        }
        Some(result)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = (self.file.frame_count() - self.next) as usize;
        (n, Some(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huffman_tables_are_prefix_free_and_complete() {
        for (t, codes) in HUFF_TREES.iter().enumerate() {
            let kraft: f64 = codes.iter().map(|&(_, l)| 0.5f64.powi(l as i32)).sum();
            assert!((kraft - 1.0).abs() < 1e-12, "tree {t}");
            let table = &huff_lookup()[t];
            for (slot, &(code, len)) in codes.iter().enumerate() {
                assert_eq!(table[code as usize], (slot as u8, len));
            }
        }
    }

    #[test]
    fn scans_are_permutations() {
        for scan in RUN_SCANS.iter().chain(std::iter::once(&DCT_SCAN)) {
            let mut seen = [false; 64];
            for &p in scan {
                assert!(!seen[p as usize]);
                seen[p as usize] = true;
            }
        }
    }

    #[test]
    fn dc_only_idct_is_flat() {
        let mut block = [0i32; 64];
        block[0] = 1000 << 3;
        let out = idct(&block);
        assert!(out.iter().all(|&v| v == out[0]));
        assert_eq!(out[0], (8000 + 0x7f) >> 8);
    }

    #[test]
    fn tree_merge_matches_hand_example() {
        // codebook 1, merge mode, one pass, bits choose: pairs swapped when bit = 1.
        // Bits (LSB first): 4 bits codebook=1, 1 bit mode=0, 2 bits passes-1=0, then 8 merge bits.
        let mut bits: u64 = 1;
        bits |= 0 << 4;
        bits |= 0 << 5;
        let merge: u64 = 0b1010_1010; // swap pairs 1,3,5,7 (bit = 1 picks the second element)
        bits |= merge << 7;
        let data = bits.to_le_bytes();
        let mut br = BitReader::new(&data);
        let tree = Tree::read(&mut br);
        assert_eq!(tree.codebook, 1);
        // Pair 0 takes bit 0 (=0): keeps [0,1]; pair 1 takes bit 1 (=1) first -> [3,2]...
        assert_eq!(&tree.symbols[..4], &[0, 1, 3, 2]);
    }
}
