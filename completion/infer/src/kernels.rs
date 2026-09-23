//! Numeric kernels. The f16 weight matrices are row-major `[rows, cols]`, and a matvec
//! computes one dot product per row.

use half::f16;
use rayon::prelude::*;

#[cfg(target_arch = "aarch64")]
mod arm;
#[cfg(target_arch = "x86_64")]
mod x86;

/// An f16 matrix, row-major, each row contiguous.
pub struct Matrix {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<f16>,
}

impl Matrix {
    pub fn row(&self, r: usize) -> &[f16] {
        &self.data[r * self.cols..(r + 1) * self.cols]
    }

    pub fn bytes(&self) -> usize {
        self.data.len() * 2
    }
}

/// Dot product of an f32 vector with an f16 row, converting on the fly.
pub fn dot_f16(x: &[f32], w: &[f16]) -> f32 {
    #[cfg(target_arch = "aarch64")]
    return arm::dot_f16(x, w);
    #[cfg(target_arch = "x86_64")]
    if x86::available() {
        // Safety: the features were detected.
        return unsafe { x86::dot_f16(x, w) };
    }
    #[cfg(not(target_arch = "aarch64"))]
    dot_f16_portable(x, w)
}

#[cfg(not(target_arch = "aarch64"))]
fn dot_f16_portable(x: &[f32], w: &[f16]) -> f32 {
    let mut acc = [0f32; 8];
    let mut chunks_x = x.chunks_exact(8);
    let mut chunks_w = w.chunks_exact(8);
    for (cx, cw) in (&mut chunks_x).zip(&mut chunks_w) {
        for k in 0..8 {
            acc[k] += cx[k] * cw[k].to_f32();
        }
    }
    let mut sum: f32 = acc.iter().sum();
    for (a, b) in chunks_x.remainder().iter().zip(chunks_w.remainder()) {
        sum += a * b.to_f32();
    }
    sum
}

/// Dot product of two f32 vectors.
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    #[cfg(target_arch = "aarch64")]
    return arm::dot(a, b);
    #[cfg(target_arch = "x86_64")]
    if x86::available() {
        // Safety: the features were detected.
        return unsafe { x86::dot(a, b) };
    }
    #[cfg(not(target_arch = "aarch64"))]
    dot_portable(a, b)
}

#[cfg(not(target_arch = "aarch64"))]
fn dot_portable(a: &[f32], b: &[f32]) -> f32 {
    // Eight independent sums, which the compiler turns into vector lanes.
    let mut acc = [0f32; 8];
    let (ca, ra) = a.as_chunks::<8>();
    let (cb, rb) = b.as_chunks::<8>();
    for (x, y) in ca.iter().zip(cb) {
        for k in 0..8 {
            acc[k] += x[k] * y[k];
        }
    }
    let mut sum: f32 = acc.iter().sum();
    for (x, y) in ra.iter().zip(rb) {
        sum += x * y;
    }
    sum
}

/// `y += a * x`.
pub fn axpy(a: f32, x: &[f32], y: &mut [f32]) {
    #[cfg(target_arch = "aarch64")]
    return arm::axpy(a, x, y);
    #[cfg(target_arch = "x86_64")]
    if x86::available() {
        // Safety: the features were detected.
        return unsafe { x86::axpy(a, x, y) };
    }
    #[cfg(not(target_arch = "aarch64"))]
    for (y, &x) in y.iter_mut().zip(x) {
        *y += a * x;
    }
}

/// Attention for one query over one head's cache: `keys` and `values` hold `scores.len()`
/// positions of `q.len()` values each. `scores` is scratch for the attention weights, and
/// `out` receives the weighted sum of the values.
pub fn attend(
    q: &[f32],
    keys: &[f32],
    values: &[f32],
    scale: f32,
    scores: &mut [f32],
    out: &mut [f32],
) {
    #[cfg(target_arch = "aarch64")]
    return arm::attend(q, keys, values, scale, scores, out);
    #[cfg(target_arch = "x86_64")]
    if x86::available() {
        // Safety: the features were detected.
        return unsafe { x86::attend(q, keys, values, scale, scores, out) };
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let hd = q.len();
        for (s, k) in scores.iter_mut().zip(keys.chunks(hd)) {
            *s = dot(q, k) * scale;
        }
        softmax(scores);
        out.fill(0.0);
        for (&w, v) in scores.iter().zip(values.chunks(hd)) {
            axpy(w, v, out);
        }
    }
}

/// Matrices smaller than this are handled on the calling thread; the work is too small to
/// pay for a parallel dispatch.
const PARALLEL_MIN_ELEMENTS: usize = 128 * 1024;

/// `y = W x` for one vector.
pub fn matvec(w: &Matrix, x: &[f32], y: &mut [f32]) {
    debug_assert_eq!(x.len(), w.cols);
    debug_assert_eq!(y.len(), w.rows);
    if w.rows * w.cols < PARALLEL_MIN_ELEMENTS {
        for (r, out) in y.iter_mut().enumerate() {
            *out = dot_f16(x, w.row(r));
        }
        return;
    }
    let chunk = w.rows.div_ceil(rayon::current_num_threads() * 4).max(8);
    y.par_chunks_mut(chunk).enumerate().for_each(|(c, ys)| {
        let base = c * chunk;
        for (j, out) in ys.iter_mut().enumerate() {
            *out = dot_f16(x, w.row(base + j));
        }
    });
}

/// `Y = X Wᵀ` for `n` vectors: `xs` is `n * cols`, `ys` is `n * rows`. Each weight row is
/// read from memory once and applied to every vector while it is in cache.
pub fn matmul(w: &Matrix, xs: &[f32], ys: &mut [f32], n: usize) {
    debug_assert_eq!(xs.len(), n * w.cols);
    debug_assert_eq!(ys.len(), n * w.rows);
    #[cfg(target_arch = "aarch64")]
    return arm::matmul(w, xs, ys, n);
    #[cfg(target_arch = "x86_64")]
    if x86::available() {
        // Safety: the features were detected.
        return unsafe { x86::matmul(w, xs, ys, n) };
    }
    #[cfg(not(target_arch = "aarch64"))]
    matmul_portable(w, xs, ys, n)
}

#[cfg(not(target_arch = "aarch64"))]
fn matmul_portable(w: &Matrix, xs: &[f32], ys: &mut [f32], n: usize) {
    let rows = w.rows;
    let cols = w.cols;
    let chunk = rows.div_ceil(rayon::current_num_threads() * 4).max(8);
    // Split the output columns (weight rows) across threads; each thread writes its own
    // column range for every vector.
    let ys_ptr = ys.as_mut_ptr() as usize;
    (0..rows.div_ceil(chunk)).into_par_iter().for_each(|c| {
        let start = c * chunk;
        let end = (start + chunk).min(rows);
        for r in start..end {
            let row = w.row(r);
            for t in 0..n {
                let x = &xs[t * cols..(t + 1) * cols];
                // Safety: every (t, r) cell is written by exactly one task.
                unsafe {
                    *(ys_ptr as *mut f32).add(t * rows + r) = dot_f16(x, row);
                }
            }
        }
    });
}

/// RMS normalization: `out = x / sqrt(mean(x²) + eps) * gamma`.
pub fn rms_norm(x: &[f32], gamma: &[f32], eps: f32, out: &mut [f32]) {
    let mean_sq = x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32;
    let scale = 1.0 / (mean_sq + eps).sqrt();
    for ((o, &v), &g) in out.iter_mut().zip(x).zip(gamma) {
        *o = v * scale * g;
    }
}

pub fn softmax(x: &mut [f32]) {
    #[cfg(target_arch = "aarch64")]
    return arm::softmax(x);
    #[cfg(target_arch = "x86_64")]
    if x86::available() {
        // Safety: the features were detected.
        return unsafe { x86::softmax(x) };
    }
    #[cfg(not(target_arch = "aarch64"))]
    softmax_portable(x)
}

#[cfg(not(target_arch = "aarch64"))]
fn softmax_portable(x: &mut [f32]) {
    let max = x.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0;
    for v in x.iter_mut() {
        *v = (*v - max).exp();
        sum += *v;
    }
    let inv = 1.0 / sum;
    for v in x.iter_mut() {
        *v *= inv;
    }
}

pub fn silu(x: f32) -> f32 {
    x / (1.0 + (-x).exp())
}

/// Rotates adjacent pairs of `x` in place, the interleaved rotary variant used by the trained
/// model. `cos` and `sin` hold one value per pair for this position.
pub fn rope(x: &mut [f32], cos: &[f32], sin: &[f32]) {
    for (pair, (&c, &s)) in x.as_chunks_mut::<2>().0.iter_mut().zip(cos.iter().zip(sin)) {
        let (a, b) = (pair[0], pair[1]);
        pair[0] = a * c - b * s;
        pair[1] = b * c + a * s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_matches_scalar() {
        let x: Vec<f32> = (0..37).map(|i| (i as f32 * 0.37).sin()).collect();
        let w: Vec<f16> = (0..37)
            .map(|i| f16::from_f32((i as f32 * 0.11).cos()))
            .collect();
        let expected: f32 = x.iter().zip(&w).map(|(a, b)| a * b.to_f32()).sum();
        assert!((dot_f16(&x, &w) - expected).abs() < 1e-4);
    }

    #[test]
    fn softmax_matches_scalar() {
        for len in [1, 7, 8, 33, 1000] {
            let x: Vec<f32> = (0..len)
                .map(|i| (i as f32 * 0.7).sin() * 30.0 - 5.0)
                .collect();
            let max = x.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let e: Vec<f64> = x.iter().map(|&v| ((v - max) as f64).exp()).collect();
            let sum: f64 = e.iter().sum();
            let mut y = x.clone();
            softmax(&mut y);
            for (a, b) in y.iter().zip(&e) {
                let b = b / sum;
                assert!(
                    (*a as f64 - b).abs() <= 1e-6 * b.max(1e-30) + 1e-37,
                    "{a} vs {b}"
                );
            }
        }
    }

    #[test]
    fn attend_matches_scalar() {
        for (hd, len) in [(64, 37), (72, 5), (20, 9), (64, 1)] {
            let q: Vec<f32> = (0..hd).map(|i| (i as f32 * 0.3).sin()).collect();
            let keys: Vec<f32> = (0..hd * len).map(|i| (i as f32 * 0.17).cos()).collect();
            let values: Vec<f32> = (0..hd * len).map(|i| (i as f32 * 0.05).sin()).collect();
            let scale = 1.0 / (hd as f32).sqrt();
            let mut expected_scores: Vec<f32> = keys
                .chunks(hd)
                .map(|k| q.iter().zip(k).map(|(a, b)| a * b).sum::<f32>() * scale)
                .collect();
            softmax(&mut expected_scores);
            let mut expected = vec![0f32; hd];
            for (&w, v) in expected_scores.iter().zip(values.chunks(hd)) {
                for (o, &v) in expected.iter_mut().zip(v) {
                    *o += w * v;
                }
            }
            let mut scores = vec![0f32; len];
            let mut out = vec![f32::NAN; hd];
            attend(&q, &keys, &values, scale, &mut scores, &mut out);
            for (a, b) in out.iter().zip(&expected) {
                assert!((a - b).abs() < 1e-5, "hd {hd} len {len}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn matmul_matches_matvec() {
        // Ragged edges in every dimension, and enough rows and tokens for several blocks.
        for (rows, cols, n) in [(300, 96, 5), (300, 101, 19), (1030, 640, 70), (7, 3, 1)] {
            matmul_case(rows, cols, n);
        }
    }

    fn matmul_case(rows: usize, cols: usize, n: usize) {
        let w = Matrix {
            rows,
            cols,
            data: (0..rows * cols)
                .map(|i| f16::from_f32(((i * 7919) % 1000) as f32 / 1000.0 - 0.5))
                .collect(),
        };
        let xs: Vec<f32> = (0..n * cols)
            .map(|i| ((i * 31) % 17) as f32 / 17.0)
            .collect();
        let mut ys = vec![0.0; n * rows];
        matmul(&w, &xs, &mut ys, n);
        let mut y = vec![0.0; rows];
        for t in 0..n {
            matvec(&w, &xs[t * cols..(t + 1) * cols], &mut y);
            // The batched kernel may sum in a different order.
            for (a, b) in ys[t * rows..(t + 1) * rows].iter().zip(&y) {
                assert!((a - b).abs() < 1e-5 * (1.0 + b.abs()), "{a} vs {b}");
            }
        }
    }
}
