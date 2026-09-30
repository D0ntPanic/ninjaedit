//! AVX2 kernels for x86_64, selected at run time so one binary still runs on CPUs without
//! them. F16C converts eight f16 weights per instruction, a sign extension eight 8-bit ones,
//! and FMA does the arithmetic.
//!
//! Decode is a plain dot product per row, like the NEON kernel. Prefill is compute-bound, so
//! it works on tiles: a block of weight rows is converted to f32 once into a per-thread
//! scratch buffer, then a register-blocked kernel computes four rows against three tokens at
//! a time, loading each weight vector once for three tokens and each input vector once for
//! four rows.

use super::{Matrix, Weights};
use half::f16;
use rayon::prelude::*;
use std::arch::x86_64::*;
use std::cell::RefCell;

/// Whether this CPU has the features every kernel here needs. The detection macro caches its
/// result, so this is a few loads.
pub fn available() -> bool {
    is_x86_feature_detected!("avx2")
        && is_x86_feature_detected!("fma")
        && is_x86_feature_detected!("f16c")
}

#[target_feature(enable = "avx2,fma")]
fn hsum(v: __m256) -> f32 {
    let s = _mm_add_ps(_mm256_castps256_ps128(v), _mm256_extractf128_ps::<1>(v));
    let s = _mm_add_ps(s, _mm_movehl_ps(s, s));
    let s = _mm_add_ss(s, _mm_movehdup_ps(s));
    _mm_cvtss_f32(s)
}

/// Dot product of an f32 vector with an f16 row.
///
/// # Safety
/// The CPU must support AVX2, FMA and F16C ([`available`]).
#[target_feature(enable = "avx2,fma,f16c")]
pub unsafe fn dot_f16(x: &[f32], w: &[f16]) -> f32 {
    debug_assert_eq!(x.len(), w.len());
    let n = x.len();
    let (xp, wp) = (x.as_ptr(), w.as_ptr() as *const __m128i);
    let mut i = 0;
    // Four independent accumulators hide the FMA latency.
    let mut acc = [_mm256_setzero_ps(); 4];
    unsafe {
        while i + 32 <= n {
            for (k, a) in acc.iter_mut().enumerate() {
                let w = _mm256_cvtph_ps(_mm_loadu_si128(wp.byte_add(2 * (i + 8 * k))));
                *a = _mm256_fmadd_ps(_mm256_loadu_ps(xp.add(i + 8 * k)), w, *a);
            }
            i += 32;
        }
        while i + 8 <= n {
            let w = _mm256_cvtph_ps(_mm_loadu_si128(wp.byte_add(2 * i)));
            acc[0] = _mm256_fmadd_ps(_mm256_loadu_ps(xp.add(i)), w, acc[0]);
            i += 8;
        }
    }
    let mut sum = hsum(_mm256_add_ps(
        _mm256_add_ps(acc[0], acc[1]),
        _mm256_add_ps(acc[2], acc[3]),
    ));
    for j in i..n {
        sum += x[j] * w[j].to_f32();
    }
    sum
}

/// Eight 8-bit values at `p`, converted.
///
/// # Safety
/// The CPU must support AVX2, and the values must be readable.
#[target_feature(enable = "avx2")]
#[inline]
unsafe fn load8_i8(p: *const i8) -> __m256 {
    unsafe { _mm256_cvtepi32_ps(_mm256_cvtepi8_epi32(_mm_loadl_epi64(p as *const __m128i))) }
}

/// Dot product of an f32 vector with a row of 8-bit weights, as [`super::dot_q8`]. Each
/// group is summed unscaled and then scaled once.
///
/// # Safety
/// The CPU must support AVX2, FMA and F16C ([`available`]).
#[target_feature(enable = "avx2,fma,f16c")]
pub unsafe fn dot_q8(x: &[f32], w: &[i8], scales: &[f16], group: usize) -> f32 {
    debug_assert!(group.is_multiple_of(16) && w.len() == scales.len() * group);
    let (xp, wp) = (x.as_ptr(), w.as_ptr());
    let mut total = _mm256_setzero_ps();
    for (g, &s) in scales.iter().enumerate() {
        let mut acc = [_mm256_setzero_ps(); 2];
        let mut i = g * group;
        let end = i + group;
        unsafe {
            while i < end {
                for (k, a) in acc.iter_mut().enumerate() {
                    let w = load8_i8(wp.add(i + 8 * k));
                    *a = _mm256_fmadd_ps(_mm256_loadu_ps(xp.add(i + 8 * k)), w, *a);
                }
                i += 16;
            }
        }
        total = _mm256_fmadd_ps(
            _mm256_add_ps(acc[0], acc[1]),
            _mm256_set1_ps(s.to_f32()),
            total,
        );
    }
    hsum(total)
}

/// Dot product of a quantized input with a row of 8-bit weights, as [`super::dot_q8q8`]:
/// sixteen values at a time are sign-extended to 16 bits and multiplied and pairwise summed
/// into 32-bit lanes, exactly.
///
/// # Safety
/// The CPU must support AVX2, FMA and F16C ([`available`]).
#[target_feature(enable = "avx2,fma,f16c")]
pub unsafe fn dot_q8q8(xq: &[i8], xs: &[f32], w: &[i8], ws: &[f16], group: usize) -> f32 {
    debug_assert!(group.is_multiple_of(16) && w.len() == ws.len() * group);
    let (xp, wp) = (xq.as_ptr(), w.as_ptr());
    let mut total = [_mm256_setzero_ps(); 2];
    for (g, (&s, &x_scale)) in ws.iter().zip(xs).enumerate() {
        let mut acc = _mm256_setzero_si256();
        let mut i = g * group;
        let end = i + group;
        unsafe {
            while i < end {
                let a = _mm256_cvtepi8_epi16(_mm_loadu_si128(wp.add(i) as *const __m128i));
                let b = _mm256_cvtepi8_epi16(_mm_loadu_si128(xp.add(i) as *const __m128i));
                acc = _mm256_add_epi32(acc, _mm256_madd_epi16(a, b));
                i += 16;
            }
        }
        let t = &mut total[g % 2];
        *t = _mm256_fmadd_ps(
            _mm256_cvtepi32_ps(acc),
            _mm256_set1_ps(s.to_f32() * x_scale),
            *t,
        );
    }
    hsum(_mm256_add_ps(total[0], total[1]))
}

/// `y += a * x`, converting `x` from f16.
///
/// # Safety
/// The CPU must support AVX2, FMA and F16C ([`available`]).
#[target_feature(enable = "avx2,fma,f16c")]
pub unsafe fn axpy_f16(a: f32, x: &[f16], y: &mut [f32]) {
    debug_assert_eq!(x.len(), y.len());
    let n = x.len();
    let av = _mm256_set1_ps(a);
    let mut i = 0;
    unsafe {
        while i + 8 <= n {
            let xv = _mm256_cvtph_ps(_mm_loadu_si128(x.as_ptr().add(i) as *const __m128i));
            let yp = y.as_mut_ptr().add(i);
            _mm256_storeu_ps(yp, _mm256_fmadd_ps(av, xv, _mm256_loadu_ps(yp)));
            i += 8;
        }
    }
    for j in i..n {
        y[j] += a * x[j].to_f32();
    }
}

/// `e^x` for each lane, to within a couple of ulp, using the Cephes polynomial. Inputs are
/// clamped to the range where the result is a normal f32.
#[target_feature(enable = "avx2,fma")]
fn exp(x: __m256) -> __m256 {
    let x = _mm256_min_ps(
        _mm256_max_ps(x, _mm256_set1_ps(-87.0)),
        _mm256_set1_ps(88.0),
    );
    // x = n ln2 + r, with ln2 split in two so n ln2 is exact enough.
    let n = _mm256_round_ps::<{ _MM_FROUND_TO_NEAREST_INT | _MM_FROUND_NO_EXC }>(_mm256_mul_ps(
        x,
        _mm256_set1_ps(std::f32::consts::LOG2_E),
    ));
    let r = _mm256_fnmadd_ps(n, _mm256_set1_ps(0.693_359_4), x);
    let r = _mm256_fnmadd_ps(n, _mm256_set1_ps(-2.121_944_4e-4), r);
    let mut p = _mm256_set1_ps(1.987_569_1e-4);
    for c in [
        1.398_199_9e-3,
        8.333_452e-3,
        4.166_579_6e-2,
        1.666_666_5e-1,
        0.5,
    ] {
        p = _mm256_fmadd_ps(p, r, _mm256_set1_ps(c));
    }
    let y = _mm256_add_ps(
        _mm256_fmadd_ps(p, _mm256_mul_ps(r, r), r),
        _mm256_set1_ps(1.0),
    );
    // Scale by 2^n by building the exponent bits directly.
    let bits = _mm256_slli_epi32::<23>(_mm256_add_epi32(
        _mm256_cvtps_epi32(n),
        _mm256_set1_epi32(127),
    ));
    _mm256_mul_ps(y, _mm256_castsi256_ps(bits))
}

/// Softmax in place, as [`super::softmax`].
///
/// # Safety
/// The CPU must support AVX2 and FMA ([`available`]).
#[target_feature(enable = "avx2,fma")]
pub unsafe fn softmax(x: &mut [f32]) {
    let n = x.len();
    let xp = x.as_mut_ptr();
    let mut i = 0;
    let mut mv = _mm256_set1_ps(f32::NEG_INFINITY);
    unsafe {
        while i + 8 <= n {
            mv = _mm256_max_ps(mv, _mm256_loadu_ps(xp.add(i)));
            i += 8;
        }
    }
    let mut lanes = [0f32; 8];
    unsafe { _mm256_storeu_ps(lanes.as_mut_ptr(), mv) };
    let max = lanes
        .iter()
        .chain(&x[i..])
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    let maxv = _mm256_set1_ps(max);
    let mut sumv = _mm256_setzero_ps();
    i = 0;
    unsafe {
        while i + 8 <= n {
            let e = exp(_mm256_sub_ps(_mm256_loadu_ps(xp.add(i)), maxv));
            _mm256_storeu_ps(xp.add(i), e);
            sumv = _mm256_add_ps(sumv, e);
            i += 8;
        }
    }
    let mut sum = hsum(sumv);
    for v in &mut x[i..] {
        *v = (*v - max).exp();
        sum += *v;
    }
    let inv = 1.0 / sum;
    let invv = _mm256_set1_ps(inv);
    i = 0;
    unsafe {
        while i + 8 <= n {
            _mm256_storeu_ps(xp.add(i), _mm256_mul_ps(_mm256_loadu_ps(xp.add(i)), invv));
            i += 8;
        }
    }
    for v in &mut x[i..] {
        *v *= inv;
    }
}

/// Attention for one query over one head's cache, as [`super::attend`].
///
/// # Safety
/// The CPU must support AVX2, FMA and F16C ([`available`]).
#[target_feature(enable = "avx2,fma,f16c")]
pub unsafe fn attend(
    q: &[f32],
    keys: &[f16],
    values: &[f16],
    scale: f32,
    scores: &mut [f32],
    out: &mut [f32],
) {
    let hd = q.len();
    let len = scores.len();
    debug_assert!(keys.len() == len * hd && values.len() == len * hd && out.len() == hd);
    let (qp, kp, vp) = (
        q.as_ptr(),
        keys.as_ptr() as *const __m128i,
        values.as_ptr() as *const __m128i,
    );
    // Scores four keys at a time, sharing each load of the query.
    const KEYS: usize = 4;
    let mut p = 0;
    while p + KEYS <= len {
        let mut acc = [_mm256_setzero_ps(); KEYS];
        let mut i = 0;
        unsafe {
            while i + 8 <= hd {
                let qv = _mm256_loadu_ps(qp.add(i));
                for (j, a) in acc.iter_mut().enumerate() {
                    *a = _mm256_fmadd_ps(load8(kp, (p + j) * hd + i), qv, *a);
                }
                i += 8;
            }
        }
        for (j, &a) in acc.iter().enumerate() {
            let mut sum = hsum(a);
            for k in i..hd {
                sum += q[k] * keys[(p + j) * hd + k].to_f32();
            }
            scores[p + j] = sum * scale;
        }
        p += KEYS;
    }
    for p in p..len {
        scores[p] = unsafe { dot_f16(q, &keys[p * hd..(p + 1) * hd]) } * scale;
    }
    unsafe { softmax(scores) };
    // The weighted sum of values, a register-sized slice of the output at a time so it stays
    // in registers across every position.
    const REGS: usize = 8;
    const WIDTH: usize = REGS * 8;
    let mut c = 0;
    while c + WIDTH <= hd {
        let mut acc = [_mm256_setzero_ps(); REGS];
        for (p, &w) in scores.iter().enumerate() {
            let wv = _mm256_set1_ps(w);
            for (r, a) in acc.iter_mut().enumerate() {
                *a = _mm256_fmadd_ps(wv, unsafe { load8(vp, p * hd + c + 8 * r) }, *a);
            }
        }
        for (r, &a) in acc.iter().enumerate() {
            unsafe { _mm256_storeu_ps(out.as_mut_ptr().add(c + 8 * r), a) };
        }
        c += WIDTH;
    }
    if c < hd {
        out[c..].fill(0.0);
        for (p, &w) in scores.iter().enumerate() {
            unsafe { axpy_f16(w, &values[p * hd + c..(p + 1) * hd], &mut out[c..]) };
        }
    }
}

/// The eight f16 values at element offset `i` from `p`, converted.
///
/// # Safety
/// The CPU must support F16C, and the values must be readable.
#[target_feature(enable = "avx2,f16c")]
#[inline]
unsafe fn load8(p: *const __m128i, i: usize) -> __m256 {
    unsafe { _mm256_cvtph_ps(_mm_loadu_si128(p.byte_add(2 * i))) }
}

/// Converts `src` to f32 into `dst`.
#[target_feature(enable = "avx2,fma,f16c")]
fn convert(src: &[f16], dst: &mut [f32]) {
    debug_assert_eq!(src.len(), dst.len());
    let n = src.len();
    let mut i = 0;
    unsafe {
        while i + 8 <= n {
            let v = _mm256_cvtph_ps(_mm_loadu_si128(src.as_ptr().add(i) as *const __m128i));
            _mm256_storeu_ps(dst.as_mut_ptr().add(i), v);
            i += 8;
        }
    }
    for j in i..n {
        dst[j] = src[j].to_f32();
    }
}

/// Converts 8-bit rows of `cols` values to f32 into `dst`, with their groups' `scales`.
#[target_feature(enable = "avx2,fma,f16c")]
fn convert_q8(src: &[i8], scales: &[f16], group: usize, dst: &mut [f32]) {
    debug_assert_eq!(src.len(), dst.len());
    for ((src, dst), &s) in src.chunks(group).zip(dst.chunks_mut(group)).zip(scales) {
        let sv = _mm256_set1_ps(s.to_f32());
        // Groups are a multiple of 16 values, so of eight.
        for (i, d) in dst.as_chunks_mut::<8>().0.iter_mut().enumerate() {
            unsafe {
                _mm256_storeu_ps(
                    d.as_mut_ptr(),
                    _mm256_mul_ps(load8_i8(src.as_ptr().add(8 * i)), sv),
                )
            };
        }
    }
}

/// Converts rows `r0..r1` of `w` to f32 into `dst`.
#[target_feature(enable = "avx2,fma,f16c")]
fn convert_rows(w: &Matrix, r0: usize, r1: usize, dst: &mut [f32]) {
    let cols = w.cols;
    match &w.data {
        Weights::F16(data) => convert(&data[r0 * cols..r1 * cols], dst),
        Weights::Q8(q) => {
            let groups = cols / q.group;
            convert_q8(
                &q.values[r0 * cols..r1 * cols],
                &q.scales[r0 * groups..r1 * groups],
                q.group,
                dst,
            )
        }
    }
}

/// Weight rows per microkernel tile.
const TILE_ROWS: usize = 4;
/// Tokens per microkernel tile. Four rows by three tokens is twelve accumulators, which
/// leaves room in the sixteen AVX registers for the three inputs and one weight vector.
const TILE_TOKENS: usize = 3;

/// `out[r][t] = w[r] · x[t]` for `R` f32 rows starting at `w` and `T` inputs starting at `x`,
/// both with stride `cols`.
///
/// # Safety
/// `w` must hold `R * cols` values and `x` `T * cols`.
#[target_feature(enable = "avx2,fma,f16c")]
unsafe fn tile<const R: usize, const T: usize>(
    w: *const f32,
    x: *const f32,
    cols: usize,
) -> [[f32; T]; R] {
    let mut acc = [[_mm256_setzero_ps(); T]; R];
    let mut i = 0;
    unsafe {
        while i + 8 <= cols {
            let mut xv = [_mm256_setzero_ps(); T];
            for (t, v) in xv.iter_mut().enumerate() {
                *v = _mm256_loadu_ps(x.add(t * cols + i));
            }
            for (r, row) in acc.iter_mut().enumerate() {
                let wv = _mm256_loadu_ps(w.add(r * cols + i));
                for (a, &v) in row.iter_mut().zip(&xv) {
                    *a = _mm256_fmadd_ps(wv, v, *a);
                }
            }
            i += 8;
        }
    }
    let mut out = [[0f32; T]; R];
    for r in 0..R {
        for t in 0..T {
            let mut sum = hsum(acc[r][t]);
            for j in i..cols {
                // Safety: j < cols, within row r and input t.
                unsafe { sum += *w.add(r * cols + j) * *x.add(t * cols + j) };
            }
            out[r][t] = sum;
        }
    }
    out
}

/// Runs the `R`-row by `T`-token tile and stores it into `ys` (`[tokens, rows]`).
///
/// # Safety
/// As [`tile`], and `ys` must have room for rows `row..row + R` of tokens `tok..tok + T`.
#[target_feature(enable = "avx2,fma,f16c")]
#[allow(clippy::too_many_arguments)]
unsafe fn tile_store<const R: usize, const T: usize>(
    w: *const f32,
    x: *const f32,
    cols: usize,
    ys: *mut f32,
    rows: usize,
    row: usize,
    tok: usize,
) {
    let out = unsafe { tile::<R, T>(w, x, cols) };
    for (r, vals) in out.iter().enumerate() {
        for (t, &v) in vals.iter().enumerate() {
            unsafe { *ys.add((tok + t) * rows + row + r) = v };
        }
    }
}

/// A tile with `r <= TILE_ROWS` rows and `t <= TILE_TOKENS` tokens, for the ragged edges.
///
/// # Safety
/// As [`tile_store`].
#[target_feature(enable = "avx2,fma,f16c")]
#[allow(clippy::too_many_arguments)]
unsafe fn tile_dispatch(
    r: usize,
    t: usize,
    w: *const f32,
    x: *const f32,
    cols: usize,
    ys: *mut f32,
    rows: usize,
    row: usize,
    tok: usize,
) {
    let args = (w, x, cols, ys, rows, row, tok);
    macro_rules! go {
        ($r:literal, $t:literal) => {
            unsafe { tile_store::<$r, $t>(args.0, args.1, args.2, args.3, args.4, args.5, args.6) }
        };
    }
    match (r, t) {
        (4, 3) => go!(4, 3),
        (4, 2) => go!(4, 2),
        (4, 1) => go!(4, 1),
        (3, 3) => go!(3, 3),
        (3, 2) => go!(3, 2),
        (3, 1) => go!(3, 1),
        (2, 3) => go!(2, 3),
        (2, 2) => go!(2, 2),
        (2, 1) => go!(2, 1),
        (1, 3) => go!(1, 3),
        (1, 2) => go!(1, 2),
        (1, 1) => go!(1, 1),
        _ => unreachable!("tile {r}x{t}"),
    }
}

thread_local! {
    /// Weight rows converted to f32 for the block a thread is working on.
    static SCRATCH: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
}

/// Bytes of f32 weights and of inputs each task works on, sized so both stay in a 512 KB L2
/// with room to spare.
const BLOCK_BYTES: usize = 96 * 1024;

/// `Y = X Wᵀ` for `n` vectors, as [`super::matmul`].
///
/// The output is split into blocks of weight rows by blocks of tokens, one task each. A task
/// converts its rows to f32 once and reuses them for every token in its block.
///
/// # Safety
/// The CPU must support AVX2, FMA and F16C ([`available`]).
pub unsafe fn matmul(w: &Matrix, xs: &[f32], ys: &mut [f32], n: usize) {
    debug_assert_eq!(xs.len(), n * w.cols);
    debug_assert_eq!(ys.len(), n * w.rows);
    let (rows, cols) = (w.rows, w.cols);
    let per_block = (BLOCK_BYTES / (cols * 4)).max(1);
    let row_block = (per_block / TILE_ROWS).max(1) * TILE_ROWS;
    let tok_block = (per_block / TILE_TOKENS).max(1) * TILE_TOKENS;
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
                let block = &mut scratch[..(r1 - r0) * cols];
                // Safety: the caller guarantees the features.
                unsafe { convert_rows(w, r0, r1, block) };
                for t in (t0..t1).step_by(TILE_TOKENS) {
                    let tn = TILE_TOKENS.min(t1 - t);
                    for r in (r0..r1).step_by(TILE_ROWS) {
                        let rn = TILE_ROWS.min(r1 - r);
                        // Safety: the rows and tokens are in bounds, and every (t, r) cell of
                        // `ys` is written by exactly one task.
                        unsafe {
                            tile_dispatch(
                                rn,
                                tn,
                                block.as_ptr().add((r - r0) * cols),
                                xs.as_ptr().add(t * cols),
                                cols,
                                ys_ptr as *mut f32,
                                rows,
                                r,
                                t,
                            )
                        };
                    }
                }
            });
        });
}
