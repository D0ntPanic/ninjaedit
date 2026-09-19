//! Numeric kernels. The f16 weight matrices are row-major `[rows, cols]`, and a matvec
//! computes one dot product per row.

use half::f16;
use rayon::prelude::*;

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
#[cfg(target_arch = "aarch64")]
pub fn dot_f16(x: &[f32], w: &[f16]) -> f32 {
    use std::arch::aarch64::*;
    debug_assert_eq!(x.len(), w.len());
    let n = x.len();
    let mut i = 0;
    // Four independent accumulators hide the FMA latency.
    let mut acc = unsafe { [vdupq_n_f32(0.0); 4] };
    unsafe {
        while i + 16 <= n {
            let w0 = vld1q_u16(w.as_ptr().add(i) as *const u16);
            let w1 = vld1q_u16(w.as_ptr().add(i + 8) as *const u16);
            let f0 = vcvt_f32_f16(vreinterpret_f16_u16(vget_low_u16(w0)));
            let f1 = vcvt_f32_f16(vreinterpret_f16_u16(vget_high_u16(w0)));
            let f2 = vcvt_f32_f16(vreinterpret_f16_u16(vget_low_u16(w1)));
            let f3 = vcvt_f32_f16(vreinterpret_f16_u16(vget_high_u16(w1)));
            let p = x.as_ptr().add(i);
            acc[0] = vfmaq_f32(acc[0], vld1q_f32(p), f0);
            acc[1] = vfmaq_f32(acc[1], vld1q_f32(p.add(4)), f1);
            acc[2] = vfmaq_f32(acc[2], vld1q_f32(p.add(8)), f2);
            acc[3] = vfmaq_f32(acc[3], vld1q_f32(p.add(12)), f3);
            i += 16;
        }
        let mut sum = vaddvq_f32(vaddq_f32(
            vaddq_f32(acc[0], acc[1]),
            vaddq_f32(acc[2], acc[3]),
        ));
        while i < n {
            sum += x[i] * w[i].to_f32();
            i += 1;
        }
        sum
    }
}

#[cfg(not(target_arch = "aarch64"))]
pub fn dot_f16(x: &[f32], w: &[f16]) -> f32 {
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
    fn matmul_matches_matvec() {
        let (rows, cols, n) = (300, 96, 5);
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
            assert_eq!(&ys[t * rows..(t + 1) * rows], &y[..]);
        }
    }
}
