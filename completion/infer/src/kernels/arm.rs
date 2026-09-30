//! NEON kernels for arm64, where Advanced SIMD is always present.
//!
//! Decode is a plain dot product per row, converting the f16 or 8-bit weights as they load.
//! Prefill is compute-bound, so it works on tiles: a block of weight rows is converted to f32
//! once into a per-thread scratch buffer, packed eight rows wide so one column of the block
//! is two vectors, and a register-blocked kernel accumulates eight rows against eight tokens
//! as outer products, broadcasting each input value from a lane. Each accumulator then holds
//! four adjacent outputs of one token, which store directly with no horizontal sums.

use super::{Matrix, Weights};
use half::f16;
use rayon::prelude::*;
use std::arch::aarch64::*;
use std::cell::RefCell;

/// Dot product of an f32 vector with an f16 row.
pub fn dot_f16(x: &[f32], w: &[f16]) -> f32 {
    debug_assert_eq!(x.len(), w.len());
    let n = x.len();
    let (xp, wp) = (x.as_ptr(), w.as_ptr() as *const u16);
    let mut i = 0;
    // Four independent accumulators hide the FMA latency.
    let mut acc = [unsafe { vdupq_n_f32(0.0) }; 4];
    unsafe {
        while i + 16 <= n {
            let w0 = vld1q_u16(wp.add(i));
            let w1 = vld1q_u16(wp.add(i + 8));
            let f0 = vcvt_f32_f16(vreinterpret_f16_u16(vget_low_u16(w0)));
            let f1 = vcvt_f32_f16(vreinterpret_f16_u16(vget_high_u16(w0)));
            let f2 = vcvt_f32_f16(vreinterpret_f16_u16(vget_low_u16(w1)));
            let f3 = vcvt_f32_f16(vreinterpret_f16_u16(vget_high_u16(w1)));
            let p = xp.add(i);
            acc[0] = vfmaq_f32(acc[0], vld1q_f32(p), f0);
            acc[1] = vfmaq_f32(acc[1], vld1q_f32(p.add(4)), f1);
            acc[2] = vfmaq_f32(acc[2], vld1q_f32(p.add(8)), f2);
            acc[3] = vfmaq_f32(acc[3], vld1q_f32(p.add(12)), f3);
            i += 16;
        }
    }
    let mut sum = unsafe {
        vaddvq_f32(vaddq_f32(
            vaddq_f32(acc[0], acc[1]),
            vaddq_f32(acc[2], acc[3]),
        ))
    };
    for j in i..n {
        sum += x[j] * w[j].to_f32();
    }
    sum
}

/// Dot product of an f32 vector with a row of 8-bit weights, as [`super::dot_q8`]. Each
/// group is summed unscaled and then scaled once, so the conversion is only a widening.
pub fn dot_q8(x: &[f32], w: &[i8], scales: &[f16], group: usize) -> f32 {
    debug_assert!(group.is_multiple_of(16) && w.len() == scales.len() * group);
    let (xp, wp) = (x.as_ptr(), w.as_ptr());
    unsafe {
        let mut total = vdupq_n_f32(0.0);
        for (g, &s) in scales.iter().enumerate() {
            let mut acc = [vdupq_n_f32(0.0); 4];
            let mut i = g * group;
            let end = i + group;
            while i < end {
                let q = vld1q_s8(wp.add(i));
                let (lo, hi) = (vmovl_s8(vget_low_s8(q)), vmovl_high_s8(q));
                let f = [
                    vcvtq_f32_s32(vmovl_s16(vget_low_s16(lo))),
                    vcvtq_f32_s32(vmovl_high_s16(lo)),
                    vcvtq_f32_s32(vmovl_s16(vget_low_s16(hi))),
                    vcvtq_f32_s32(vmovl_high_s16(hi)),
                ];
                for (k, (a, f)) in acc.iter_mut().zip(f).enumerate() {
                    *a = vfmaq_f32(*a, vld1q_f32(xp.add(i + 4 * k)), f);
                }
                i += 16;
            }
            let sum = vaddq_f32(vaddq_f32(acc[0], acc[1]), vaddq_f32(acc[2], acc[3]));
            total = vfmaq_n_f32(total, sum, s.to_f32());
        }
        vaddvq_f32(total)
    }
}

/// `acc + a · b`, four lanes each summing four byte products, as SDOT computes it but with
/// widening multiplies, for cores without the dot product extension. Integer sums are exact,
/// so both give the same result.
#[inline(always)]
fn sdot_widening(acc: int32x4_t, a: int8x16_t, b: int8x16_t) -> int32x4_t {
    unsafe {
        let lo = vmull_s8(vget_low_s8(a), vget_low_s8(b));
        let hi = vmull_high_s8(a, b);
        vaddq_s32(acc, vpadalq_s16(vpaddlq_s16(lo), hi))
    }
}

/// Dot product of a quantized input with a row of 8-bit weights, as [`super::dot_q8q8`].
pub fn dot_q8q8(xq: &[i8], xs: &[f32], w: &[i8], ws: &[f16], group: usize) -> f32 {
    debug_assert!(group.is_multiple_of(16) && w.len() == ws.len() * group);
    if std::arch::is_aarch64_feature_detected!("dotprod") {
        // Safety: the extension was detected.
        unsafe { dot_q8q8_sdot(xq, xs, w, ws, group) }
    } else {
        dot_q8q8_with(xq, xs, w, ws, group, sdot_widening)
    }
}

/// [`dot_q8q8`] with SDOT. The loop is instantiated inside this function so the intrinsic
/// inlines with the extension enabled.
///
/// # Safety
/// The CPU must have the dot product extension.
#[target_feature(enable = "dotprod")]
unsafe fn dot_q8q8_sdot(xq: &[i8], xs: &[f32], w: &[i8], ws: &[f16], group: usize) -> f32 {
    dot_q8q8_with(xq, xs, w, ws, group, |acc, a, b| vdotq_s32(acc, a, b))
}

#[inline(always)]
fn dot_q8q8_with(
    xq: &[i8],
    xs: &[f32],
    w: &[i8],
    ws: &[f16],
    group: usize,
    dot: impl Fn(int32x4_t, int8x16_t, int8x16_t) -> int32x4_t,
) -> f32 {
    let (xp, wp) = (xq.as_ptr(), w.as_ptr());
    unsafe {
        // Groups rotate through four sums so the scaling FMAs don't form one long chain.
        let mut total = [vdupq_n_f32(0.0); 4];
        for (g, (&s, &x_scale)) in ws.iter().zip(xs).enumerate() {
            let mut acc = vdupq_n_s32(0);
            let mut i = g * group;
            let end = i + group;
            while i < end {
                acc = dot(acc, vld1q_s8(wp.add(i)), vld1q_s8(xp.add(i)));
                i += 16;
            }
            let t = &mut total[g % 4];
            *t = vfmaq_n_f32(*t, vcvtq_f32_s32(acc), s.to_f32() * x_scale);
        }
        vaddvq_f32(vaddq_f32(
            vaddq_f32(total[0], total[1]),
            vaddq_f32(total[2], total[3]),
        ))
    }
}

/// `y += a * x`, converting `x` from f16.
pub fn axpy_f16(a: f32, x: &[f16], y: &mut [f32]) {
    debug_assert_eq!(x.len(), y.len());
    let n = x.len();
    let xp = x.as_ptr() as *const u16;
    let mut i = 0;
    unsafe {
        while i + 4 <= n {
            let yp = y.as_mut_ptr().add(i);
            let xv = vcvt_f32_f16(vreinterpret_f16_u16(vld1_u16(xp.add(i))));
            vst1q_f32(yp, vfmaq_n_f32(vld1q_f32(yp), xv, a));
            i += 4;
        }
    }
    for j in i..n {
        y[j] += a * x[j].to_f32();
    }
}

/// `e^x` for each lane, to within a couple of ulp, using the Cephes polynomial. Inputs are
/// clamped to the range where the result is a normal f32.
fn exp(x: float32x4_t) -> float32x4_t {
    unsafe {
        let x = vminq_f32(vmaxq_f32(x, vdupq_n_f32(-87.0)), vdupq_n_f32(88.0));
        // x = n ln2 + r, with ln2 split in two so n ln2 is exact enough.
        let n = vrndnq_f32(vmulq_n_f32(x, std::f32::consts::LOG2_E));
        let r = vfmsq_n_f32(x, n, 0.693_359_4);
        let r = vfmsq_n_f32(r, n, -2.121_944_4e-4);
        let mut p = vdupq_n_f32(1.987_569_1e-4);
        for c in [
            1.398_199_9e-3,
            8.333_452e-3,
            4.166_579_6e-2,
            1.666_666_5e-1,
            0.5,
        ] {
            p = vfmaq_f32(vdupq_n_f32(c), p, r);
        }
        let y = vaddq_f32(vfmaq_f32(r, p, vmulq_f32(r, r)), vdupq_n_f32(1.0));
        // Scale by 2^n by building the exponent bits directly.
        let bits = vshlq_n_s32::<23>(vaddq_s32(vcvtq_s32_f32(n), vdupq_n_s32(127)));
        vmulq_f32(y, vreinterpretq_f32_s32(bits))
    }
}

/// Softmax in place, as [`super::softmax`].
pub fn softmax(x: &mut [f32]) {
    let n = x.len();
    let xp = x.as_mut_ptr();
    let mut i = 0;
    let mut mv = unsafe { vdupq_n_f32(f32::NEG_INFINITY) };
    unsafe {
        while i + 4 <= n {
            mv = vmaxq_f32(mv, vld1q_f32(xp.add(i)));
            i += 4;
        }
    }
    let max = x[i..]
        .iter()
        .copied()
        .fold(unsafe { vmaxvq_f32(mv) }, f32::max);
    let mut sumv = unsafe { vdupq_n_f32(0.0) };
    i = 0;
    unsafe {
        while i + 4 <= n {
            let e = exp(vsubq_f32(vld1q_f32(xp.add(i)), vdupq_n_f32(max)));
            vst1q_f32(xp.add(i), e);
            sumv = vaddq_f32(sumv, e);
            i += 4;
        }
    }
    let mut sum = unsafe { vaddvq_f32(sumv) };
    for v in &mut x[i..] {
        *v = (*v - max).exp();
        sum += *v;
    }
    let inv = 1.0 / sum;
    i = 0;
    unsafe {
        while i + 4 <= n {
            vst1q_f32(xp.add(i), vmulq_n_f32(vld1q_f32(xp.add(i)), inv));
            i += 4;
        }
    }
    for v in &mut x[i..] {
        *v *= inv;
    }
}

/// Attention for one query over one head's cache, as [`super::attend`].
pub fn attend(
    q: &[f32],
    keys: &[f16],
    values: &[f16],
    scale: f32,
    scores: &mut [f32],
    out: &mut [f32],
) {
    let hd = q.len();
    let len = scores.len();
    assert!(keys.len() == len * hd && values.len() == len * hd && out.len() == hd);
    let (qp, kp, vp) = (
        q.as_ptr(),
        keys.as_ptr() as *const u16,
        values.as_ptr() as *const u16,
    );
    let half = |v: uint16x8_t| unsafe {
        (
            vcvt_f32_f16(vreinterpret_f16_u16(vget_low_u16(v))),
            vcvt_f32_f16(vreinterpret_f16_u16(vget_high_u16(v))),
        )
    };
    // Scores four keys at a time, sharing each load of the query, with two accumulators per
    // key so the eight FMA chains hide the latency.
    const KEYS: usize = 4;
    let mut p = 0;
    while p + KEYS <= len {
        let mut acc = [[unsafe { vdupq_n_f32(0.0) }; 2]; KEYS];
        let mut i = 0;
        unsafe {
            while i + 8 <= hd {
                let (q0, q1) = (vld1q_f32(qp.add(i)), vld1q_f32(qp.add(i + 4)));
                for (j, a) in acc.iter_mut().enumerate() {
                    let (k0, k1) = half(vld1q_u16(kp.add((p + j) * hd + i)));
                    a[0] = vfmaq_f32(a[0], k0, q0);
                    a[1] = vfmaq_f32(a[1], k1, q1);
                }
                i += 8;
            }
        }
        for (j, a) in acc.iter().enumerate() {
            let mut sum = unsafe { vaddvq_f32(vaddq_f32(a[0], a[1])) };
            for k in i..hd {
                sum += q[k] * keys[(p + j) * hd + k].to_f32();
            }
            scores[p + j] = sum * scale;
        }
        p += KEYS;
    }
    for p in p..len {
        scores[p] = dot_f16(q, &keys[p * hd..(p + 1) * hd]) * scale;
    }
    softmax(scores);
    // The weighted sum of values, a register-sized slice of the output at a time so it stays
    // in registers across every position.
    const REGS: usize = 16;
    const WIDTH: usize = REGS * 4;
    let mut c = 0;
    while c + WIDTH <= hd {
        let mut acc = [unsafe { vdupq_n_f32(0.0) }; REGS];
        for (p, &w) in scores.iter().enumerate() {
            for (r, a) in acc.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let (v0, v1) = half(unsafe { vld1q_u16(vp.add(p * hd + c + 8 * r)) });
                unsafe {
                    a[0] = vfmaq_n_f32(a[0], v0, w);
                    a[1] = vfmaq_n_f32(a[1], v1, w);
                }
            }
        }
        for (r, &a) in acc.iter().enumerate() {
            unsafe { vst1q_f32(out.as_mut_ptr().add(c + 4 * r), a) };
        }
        c += WIDTH;
    }
    if c < hd {
        out[c..].fill(0.0);
        for (p, &w) in scores.iter().enumerate() {
            axpy_f16(w, &values[p * hd + c..(p + 1) * hd], &mut out[c..]);
        }
    }
}

/// Weight rows per microkernel tile, packed together: two vectors per column.
const TILE_ROWS: usize = 8;
/// Tokens per microkernel tile. Eight rows by eight tokens is sixteen accumulators, leaving
/// the rest of the 32 NEON registers for the eight input vectors and the weights.
const TILE_TOKENS: usize = 8;

/// Converts `rows <= TILE_ROWS` rows of `cols` values to f32, packed column by column into
/// `dst` (`cols * TILE_ROWS` values). Missing rows are zero. `load(r, k)` gives row `r`'s
/// values at columns `k..k + 4`, and `scalar(r, k)` its value at column `k`.
///
/// # Safety
/// `load` must be safe to call for every `r < rows` and `k + 4 <= cols`.
unsafe fn pack(
    rows: usize,
    cols: usize,
    dst: &mut [f32],
    load: impl Fn(usize, usize) -> float32x4_t,
    scalar: impl Fn(usize, usize) -> f32,
) {
    debug_assert_eq!(dst.len(), cols * TILE_ROWS);
    let dp = dst.as_mut_ptr();
    let mut k = 0;
    if rows == TILE_ROWS {
        // Four columns at a time: convert a 4x4 block per half of the rows and transpose it
        // so each column's four rows are one vector.
        while k + 4 <= cols {
            for half in 0..2 {
                let (r0, r1, r2, r3) = (
                    load(4 * half, k),
                    load(4 * half + 1, k),
                    load(4 * half + 2, k),
                    load(4 * half + 3, k),
                );
                unsafe {
                    let t0 = vreinterpretq_f64_f32(vtrn1q_f32(r0, r1));
                    let t1 = vreinterpretq_f64_f32(vtrn2q_f32(r0, r1));
                    let t2 = vreinterpretq_f64_f32(vtrn1q_f32(r2, r3));
                    let t3 = vreinterpretq_f64_f32(vtrn2q_f32(r2, r3));
                    let cols4 = [
                        vtrn1q_f64(t0, t2),
                        vtrn1q_f64(t1, t3),
                        vtrn2q_f64(t0, t2),
                        vtrn2q_f64(t1, t3),
                    ];
                    for (j, &c) in cols4.iter().enumerate() {
                        vst1q_f32(
                            dp.add((k + j) * TILE_ROWS + 4 * half),
                            vreinterpretq_f32_f64(c),
                        );
                    }
                }
            }
            k += 4;
        }
    }
    for k in k..cols {
        for r in 0..TILE_ROWS {
            dst[k * TILE_ROWS + r] = if r < rows { scalar(r, k) } else { 0.0 };
        }
    }
}

/// Packs rows `r..r + rows` of `w` into `dst`, as [`pack`].
///
/// # Safety
/// The rows must be in the matrix.
unsafe fn pack_rows(w: &Matrix, r: usize, rows: usize, dst: &mut [f32]) {
    let cols = w.cols;
    match &w.data {
        Weights::F16(data) => {
            let src = unsafe { data.as_ptr().add(r * cols) } as *const u16;
            let load = |r: usize, k: usize| unsafe {
                vcvt_f32_f16(vreinterpret_f16_u16(vld1_u16(src.add(r * cols + k))))
            };
            // Safety: r < rows and k < cols.
            let scalar = |r: usize, k: usize| f16::from_bits(unsafe { *src.add(r * cols + k) });
            unsafe { pack(rows, cols, dst, load, |r, k| scalar(r, k).to_f32()) }
        }
        Weights::Q8(q) => {
            let groups = cols / q.group;
            let src = unsafe { q.values.as_ptr().add(r * cols) };
            let scales = &q.scales[r * groups..(r + rows) * groups];
            let scale = |r: usize, k: usize| scales[r * groups + k / q.group].to_f32();
            // Four values never straddle a group, whose size is a multiple of 16.
            let load = |r: usize, k: usize| unsafe {
                let bytes = vld1_lane_s32::<0>(src.add(r * cols + k) as *const i32, vdup_n_s32(0));
                let wide = vmovl_s16(vget_low_s16(vmovl_s8(vreinterpret_s8_s32(bytes))));
                vmulq_n_f32(vcvtq_f32_s32(wide), scale(r, k))
            };
            // Safety: r < rows and k < cols.
            let scalar =
                |r: usize, k: usize| unsafe { *src.add(r * cols + k) } as f32 * scale(r, k);
            unsafe { pack(rows, cols, dst, load, scalar) }
        }
    }
}

/// `ys[tok + t][row + r] = w[r] · x[t]` for the `rows_here <= TILE_ROWS` rows packed at `w`
/// and `T` inputs starting at `x` with stride `cols`; `ys` is `[tokens, rows]`.
///
/// # Safety
/// `w` must hold `cols * TILE_ROWS` packed values, `x` `T * cols`, and `ys` must have room for
/// rows `row..row + rows_here` of tokens `tok..tok + T`.
#[allow(clippy::too_many_arguments)]
unsafe fn tile<const T: usize>(
    w: *const f32,
    x: *const f32,
    cols: usize,
    ys: *mut f32,
    rows: usize,
    row: usize,
    rows_here: usize,
    tok: usize,
) {
    unsafe {
        let mut acc = [[vdupq_n_f32(0.0); 2]; T];
        let mut k = 0;
        while k + 4 <= cols {
            let mut xv = [vdupq_n_f32(0.0); T];
            for (t, v) in xv.iter_mut().enumerate() {
                *v = vld1q_f32(x.add(t * cols + k));
            }
            macro_rules! step {
                ($j:literal) => {
                    let w0 = vld1q_f32(w.add((k + $j) * TILE_ROWS));
                    let w1 = vld1q_f32(w.add((k + $j) * TILE_ROWS + 4));
                    for (a, &v) in acc.iter_mut().zip(&xv) {
                        a[0] = vfmaq_laneq_f32::<$j>(a[0], w0, v);
                        a[1] = vfmaq_laneq_f32::<$j>(a[1], w1, v);
                    }
                };
            }
            step!(0);
            step!(1);
            step!(2);
            step!(3);
            k += 4;
        }
        for k in k..cols {
            let w0 = vld1q_f32(w.add(k * TILE_ROWS));
            let w1 = vld1q_f32(w.add(k * TILE_ROWS + 4));
            for (t, a) in acc.iter_mut().enumerate() {
                let v = *x.add(t * cols + k);
                a[0] = vfmaq_n_f32(a[0], w0, v);
                a[1] = vfmaq_n_f32(a[1], w1, v);
            }
        }
        for (t, a) in acc.iter().enumerate() {
            let out = ys.add((tok + t) * rows + row);
            if rows_here == TILE_ROWS {
                vst1q_f32(out, a[0]);
                vst1q_f32(out.add(4), a[1]);
            } else {
                let mut vals = [0f32; TILE_ROWS];
                vst1q_f32(vals.as_mut_ptr(), a[0]);
                vst1q_f32(vals.as_mut_ptr().add(4), a[1]);
                std::ptr::copy_nonoverlapping(vals.as_ptr(), out, rows_here);
            }
        }
    }
}

/// A tile with `t <= TILE_TOKENS` tokens, for the ragged edge.
///
/// # Safety
/// As [`tile`].
#[allow(clippy::too_many_arguments)]
unsafe fn tile_dispatch(
    t: usize,
    w: *const f32,
    x: *const f32,
    cols: usize,
    ys: *mut f32,
    rows: usize,
    row: usize,
    rows_here: usize,
    tok: usize,
) {
    let args = (w, x, cols, ys, rows, row, rows_here, tok);
    macro_rules! go {
        ($t:literal) => {
            unsafe {
                tile::<$t>(
                    args.0, args.1, args.2, args.3, args.4, args.5, args.6, args.7,
                )
            }
        };
    }
    match t {
        8 => go!(8),
        7 => go!(7),
        6 => go!(6),
        5 => go!(5),
        4 => go!(4),
        3 => go!(3),
        2 => go!(2),
        1 => go!(1),
        _ => unreachable!("tile of {t} tokens"),
    }
}

thread_local! {
    /// Weight rows converted to f32 and packed for the block a thread is working on.
    static SCRATCH: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
}

/// Bytes of packed f32 weights per task. The kernel sweeps the whole block for each tile of
/// tokens, so it is sized to stay in L1, which is 64 KB on the performance cores.
const ROW_BLOCK_BYTES: usize = 48 * 1024;
/// Bytes of inputs per task. Each task packs its rows once, so larger token blocks spread
/// that cost further; beyond this there are too few tasks to balance across the cores.
const TOKEN_BLOCK_BYTES: usize = 192 * 1024;

/// `Y = X Wᵀ` for `n` vectors, as [`super::matmul`].
///
/// The output is split into blocks of weight rows by blocks of tokens, one task each. A task
/// converts and packs its rows once and reuses them for every token in its block.
pub fn matmul(w: &Matrix, xs: &[f32], ys: &mut [f32], n: usize) {
    assert_eq!(xs.len(), n * w.cols);
    assert_eq!(ys.len(), n * w.rows);
    let (rows, cols) = (w.rows, w.cols);
    let row_block = (ROW_BLOCK_BYTES / (cols * 4 * TILE_ROWS)).max(1) * TILE_ROWS;
    let tok_block = (TOKEN_BLOCK_BYTES / (cols * 4 * TILE_TOKENS)).max(1) * TILE_TOKENS;
    let row_blocks = rows.div_ceil(row_block);
    let tok_blocks = n.div_ceil(tok_block);
    let ys_ptr = ys.as_mut_ptr() as usize;
    (0..row_blocks * tok_blocks)
        .into_par_iter()
        .for_each(|task| {
            let (rb, tb) = (task / tok_blocks, task % tok_blocks);
            let r0 = rb * row_block;
            let r1 = (r0 + row_block).min(rows);
            let t0 = tb * tok_block;
            let t1 = (t0 + tok_block).min(n);
            SCRATCH.with_borrow_mut(|scratch| {
                scratch.resize(row_block * cols, 0.0);
                let groups = (r1 - r0).div_ceil(TILE_ROWS);
                for (g, packed) in scratch
                    .chunks_exact_mut(TILE_ROWS * cols)
                    .take(groups)
                    .enumerate()
                {
                    let r = r0 + g * TILE_ROWS;
                    // Safety: the rows packed, at most r1 - r, are in the matrix.
                    unsafe { pack_rows(w, r, TILE_ROWS.min(r1 - r), packed) };
                }
                for t in (t0..t1).step_by(TILE_TOKENS) {
                    let tn = TILE_TOKENS.min(t1 - t);
                    for g in 0..groups {
                        let r = r0 + g * TILE_ROWS;
                        // Safety: the rows and tokens are in bounds, and every (t, r) cell of
                        // `ys` is written by exactly one task.
                        unsafe {
                            tile_dispatch(
                                tn,
                                scratch.as_ptr().add(g * TILE_ROWS * cols),
                                xs.as_ptr().add(t * cols),
                                cols,
                                ys_ptr as *mut f32,
                                rows,
                                r,
                                TILE_ROWS.min(r1 - r),
                                t,
                            )
                        };
                    }
                }
            });
        });
}
