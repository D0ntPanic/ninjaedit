//! Numeric kernels. Weight matrices are row-major `[rows, cols]`, in f16 or 8-bit, and a
//! matvec computes one dot product per row.

use half::f16;
use rayon::prelude::*;

#[cfg(target_arch = "aarch64")]
mod arm;
#[cfg(target_arch = "x86_64")]
mod x86;

/// A weight matrix, row-major, each row contiguous.
pub struct Matrix {
    pub rows: usize,
    pub cols: usize,
    pub data: Weights,
}

/// The values of a [`Matrix`], in the precision the checkpoint was exported in.
pub enum Weights {
    F16(Vec<f16>),
    Q8(Q8),
}

/// Symmetric 8-bit quantization: each row is split into groups of `group` values sharing an
/// f16 scale, and a weight is its signed byte times its group's scale. Groups are a multiple
/// of [`Q8::GROUP_MULTIPLE`] values and divide the row, so every vector a kernel loads lies
/// in one group.
pub struct Q8 {
    pub values: Vec<i8>,
    /// `[rows, cols / group]`.
    pub scales: Vec<f16>,
    pub group: usize,
}

impl Q8 {
    /// The widest vector of weights a kernel loads at once.
    pub const GROUP_MULTIPLE: usize = 16;

    /// Row `r` of a matrix `cols` wide: its values and its groups' scales.
    pub fn row(&self, r: usize, cols: usize) -> (&[i8], &[f16]) {
        let groups = cols / self.group;
        (
            &self.values[r * cols..(r + 1) * cols],
            &self.scales[r * groups..(r + 1) * groups],
        )
    }
}

impl Matrix {
    /// Bytes of weights, the scales of a quantized matrix included.
    pub fn bytes(&self) -> usize {
        match &self.data {
            Weights::F16(data) => data.len() * 2,
            Weights::Q8(q) => q.values.len() + q.scales.len() * 2,
        }
    }

    /// Row `r` as f32, into `out`.
    pub fn row_into(&self, r: usize, out: &mut [f32]) {
        debug_assert_eq!(out.len(), self.cols);
        match &self.data {
            Weights::F16(data) => {
                for (o, &w) in out
                    .iter_mut()
                    .zip(&data[r * self.cols..(r + 1) * self.cols])
                {
                    *o = w.to_f32();
                }
            }
            Weights::Q8(q) => {
                let (values, scales) = q.row(r, self.cols);
                for ((out, values), &s) in out
                    .chunks_mut(q.group)
                    .zip(values.chunks(q.group))
                    .zip(scales)
                {
                    let s = s.to_f32();
                    for (o, &v) in out.iter_mut().zip(values) {
                        *o = v as f32 * s;
                    }
                }
            }
        }
    }

    /// Row `r` dotted with `x`.
    pub fn dot_row(&self, x: &[f32], r: usize) -> f32 {
        match &self.data {
            Weights::F16(data) => dot_f16(x, &data[r * self.cols..(r + 1) * self.cols]),
            Weights::Q8(q) => {
                let (values, scales) = q.row(r, self.cols);
                dot_q8(x, values, scales, q.group)
            }
        }
    }

    /// The matrix quantized to 8 bits as the export script does it, for tests.
    pub fn quantize(&self, group: usize) -> Matrix {
        let mut row = vec![0f32; self.cols];
        let mut values = Vec::with_capacity(self.rows * self.cols);
        let mut scales = Vec::with_capacity(self.rows * self.cols / group);
        for r in 0..self.rows {
            self.row_into(r, &mut row);
            for g in row.chunks(group) {
                let max = g.iter().fold(0f32, |m, v| m.max(v.abs()));
                let scale = f16::from_f32(max / 127.0);
                let s = if scale.to_f32() == 0.0 {
                    1.0
                } else {
                    scale.to_f32()
                };
                scales.push(scale);
                values.extend(g.iter().map(|v| (v / s).round().clamp(-127.0, 127.0) as i8));
            }
        }
        Matrix {
            rows: self.rows,
            cols: self.cols,
            data: Weights::Q8(Q8 {
                values,
                scales,
                group,
            }),
        }
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
    let (chunks_x, rest_x) = x.as_chunks::<8>();
    let (chunks_w, rest_w) = w.as_chunks::<8>();
    for (cx, cw) in chunks_x.iter().zip(chunks_w) {
        for k in 0..8 {
            acc[k] += cx[k] * cw[k].to_f32();
        }
    }
    let mut sum: f32 = acc.iter().sum();
    for (a, b) in rest_x.iter().zip(rest_w) {
        sum += a * b.to_f32();
    }
    sum
}

/// Dot product of an f32 vector with a row of 8-bit weights, `w.len() / scales.len()` to a
/// scale, converting on the fly.
pub fn dot_q8(x: &[f32], w: &[i8], scales: &[f16], group: usize) -> f32 {
    debug_assert_eq!(x.len(), w.len());
    debug_assert_eq!(w.len(), scales.len() * group);
    #[cfg(target_arch = "aarch64")]
    return arm::dot_q8(x, w, scales, group);
    #[cfg(target_arch = "x86_64")]
    if x86::available() {
        // Safety: the features were detected.
        return unsafe { x86::dot_q8(x, w, scales, group) };
    }
    #[cfg(not(target_arch = "aarch64"))]
    dot_q8_portable(x, w, scales, group)
}

#[cfg(not(target_arch = "aarch64"))]
fn dot_q8_portable(x: &[f32], w: &[i8], scales: &[f16], group: usize) -> f32 {
    let mut sum = 0.0;
    for ((x, w), &s) in x.chunks(group).zip(w.chunks(group)).zip(scales) {
        let mut acc = [0f32; 8];
        for (cx, cw) in x.as_chunks::<8>().0.iter().zip(w.as_chunks::<8>().0) {
            for k in 0..8 {
                acc[k] += cx[k] * cw[k] as f32;
            }
        }
        sum += acc.iter().sum::<f32>() * s.to_f32();
    }
    sum
}

/// A vector quantized to 8 bits in groups, as [`quantize_input`] makes it.
pub struct QuantizedInput {
    pub values: Vec<i8>,
    pub scales: Vec<f32>,
}

/// Quantizes `x` symmetrically in groups of `group` values, each with an f32 scale, for the
/// integer dot products of [`dot_q8q8`].
pub fn quantize_input(x: &[f32], group: usize) -> QuantizedInput {
    let mut values = Vec::with_capacity(x.len());
    let mut scales = Vec::with_capacity(x.len() / group);
    for g in x.chunks(group) {
        let max = g.iter().fold(0f32, |m, v| m.max(v.abs()));
        let scale = max / 127.0;
        let inv = if scale == 0.0 { 0.0 } else { 1.0 / scale };
        values.extend(g.iter().map(|v| (v * inv).round() as i8));
        scales.push(scale);
    }
    QuantizedInput { values, scales }
}

/// Dot product of a quantized input with a row of 8-bit weights in the same groups. Each
/// group's products are summed exactly in integers and then scaled, so decode multiplies
/// bytes rather than converting every weight to f32.
pub fn dot_q8q8(x: &QuantizedInput, w: &[i8], scales: &[f16], group: usize) -> f32 {
    debug_assert_eq!(x.values.len(), w.len());
    debug_assert_eq!(w.len(), scales.len() * group);
    #[cfg(target_arch = "aarch64")]
    return arm::dot_q8q8(&x.values, &x.scales, w, scales, group);
    #[cfg(target_arch = "x86_64")]
    if x86::available() {
        // Safety: the features were detected.
        return unsafe { x86::dot_q8q8(&x.values, &x.scales, w, scales, group) };
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let mut sum = 0.0;
        for (((xv, wv), &xs), &ws) in x
            .values
            .chunks(group)
            .zip(w.chunks(group))
            .zip(&x.scales)
            .zip(scales)
        {
            let dot: i32 = xv.iter().zip(wv).map(|(&a, &b)| a as i32 * b as i32).sum();
            sum += dot as f32 * (ws.to_f32() * xs);
        }
        sum
    }
}

/// `y += a * x`, converting `x` from f16.
pub fn axpy_f16(a: f32, x: &[f16], y: &mut [f32]) {
    #[cfg(target_arch = "aarch64")]
    return arm::axpy_f16(a, x, y);
    #[cfg(target_arch = "x86_64")]
    if x86::available() {
        // Safety: the features were detected.
        return unsafe { x86::axpy_f16(a, x, y) };
    }
    #[cfg(not(target_arch = "aarch64"))]
    for (y, &x) in y.iter_mut().zip(x) {
        *y += a * x.to_f32();
    }
}

/// Attention for one query over one head's cache: `keys` and `values` hold `scores.len()`
/// positions of `q.len()` values each, in f16 like the weights, since at long context
/// reading the cache costs as much as reading the weights. `scores` is scratch for the
/// attention weights, and `out` receives the weighted sum of the values.
pub fn attend(
    q: &[f32],
    keys: &[f16],
    values: &[f16],
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
            *s = dot_f16(q, k) * scale;
        }
        softmax(scores);
        out.fill(0.0);
        for (&w, v) in scores.iter().zip(values.chunks(hd)) {
            axpy_f16(w, v, out);
        }
    }
}

/// Matrices smaller than this are handled on the calling thread; the work is too small to
/// pay for a parallel dispatch.
const PARALLEL_MIN_ELEMENTS: usize = 128 * 1024;

/// `y = W x` for one vector. An 8-bit matrix quantizes `x` in its groups and takes integer
/// dot products: decode reads each weight once and is otherwise bound by converting them.
pub fn matvec(w: &Matrix, x: &[f32], y: &mut [f32]) {
    debug_assert_eq!(x.len(), w.cols);
    debug_assert_eq!(y.len(), w.rows);
    match &w.data {
        Weights::F16(_) => matvec_rows(w, y, |r| w.dot_row(x, r)),
        Weights::Q8(q) => {
            let xq = quantize_input(x, q.group);
            matvec_rows(w, y, |r| {
                let (values, scales) = q.row(r, w.cols);
                dot_q8q8(&xq, values, scales, q.group)
            })
        }
    }
}

fn matvec_rows(w: &Matrix, y: &mut [f32], dot: impl Fn(usize) -> f32 + Sync) {
    if w.rows * w.cols < PARALLEL_MIN_ELEMENTS {
        for (r, out) in y.iter_mut().enumerate() {
            *out = dot(r);
        }
        return;
    }
    let chunk = w.rows.div_ceil(rayon::current_num_threads() * 4).max(8);
    y.par_chunks_mut(chunk).enumerate().for_each(|(c, ys)| {
        let base = c * chunk;
        for (j, out) in ys.iter_mut().enumerate() {
            *out = dot(base + j);
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
            for t in 0..n {
                let x = &xs[t * cols..(t + 1) * cols];
                // Safety: every (t, r) cell is written by exactly one task.
                unsafe {
                    *(ys_ptr as *mut f32).add(t * rows + r) = w.dot_row(x, r);
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
            let keys: Vec<f16> = (0..hd * len)
                .map(|i| f16::from_f32((i as f32 * 0.17).cos()))
                .collect();
            let values: Vec<f16> = (0..hd * len)
                .map(|i| f16::from_f32((i as f32 * 0.05).sin()))
                .collect();
            let scale = 1.0 / (hd as f32).sqrt();
            let mut expected_scores: Vec<f32> = keys
                .chunks(hd)
                .map(|k| q.iter().zip(k).map(|(a, b)| a * b.to_f32()).sum::<f32>() * scale)
                .collect();
            softmax(&mut expected_scores);
            let mut expected = vec![0f32; hd];
            for (&w, v) in expected_scores.iter().zip(values.chunks(hd)) {
                for (o, &v) in expected.iter_mut().zip(v) {
                    *o += w * v.to_f32();
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
            matmul_case(&f16_matrix(rows, cols), n);
        }
    }

    #[test]
    fn matmul_matches_matvec_q8() {
        for (rows, cols, n, group) in [(300, 96, 5, 32), (300, 160, 19, 16), (1030, 640, 70, 64)] {
            let w = f16_matrix(rows, cols).quantize(group);
            matmul_case(&w, n);
        }
    }

    #[test]
    fn dot_q8_matches_dequantized() {
        for (cols, group) in [(32, 32), (48, 16), (640, 32), (1728, 64)] {
            let w = f16_matrix(3, cols).quantize(group);
            let x: Vec<f32> = (0..cols).map(|i| (i as f32 * 0.37).sin()).collect();
            let mut row = vec![0f32; cols];
            for r in 0..3 {
                w.row_into(r, &mut row);
                let expected: f32 = x.iter().zip(&row).map(|(a, b)| a * b).sum();
                let got = w.dot_row(&x, r);
                assert!(
                    (got - expected).abs() < 1e-4 * (1.0 + expected.abs()),
                    "{cols}/{group} row {r}: {got} vs {expected}"
                );
            }
        }
    }

    #[test]
    fn dot_q8q8_matches_integer_reference() {
        for (cols, group) in [(32, 32), (48, 16), (640, 32), (1728, 64)] {
            let w = f16_matrix(3, cols).quantize(group);
            let Weights::Q8(q) = &w.data else {
                unreachable!()
            };
            let x: Vec<f32> = (0..cols).map(|i| (i as f32 * 0.37).sin() * 3.0).collect();
            let xq = quantize_input(&x, group);
            for r in 0..3 {
                let (values, scales) = q.row(r, cols);
                let expected: f64 = (xq.values.chunks(group).zip(values.chunks(group)))
                    .zip(xq.scales.iter().zip(scales))
                    .map(|((a, b), (&xs, &ws))| {
                        let dot: i64 = a.iter().zip(b).map(|(&a, &b)| a as i64 * b as i64).sum();
                        dot as f64 * ws.to_f32() as f64 * xs as f64
                    })
                    .sum();
                let got = dot_q8q8(&xq, values, scales, group) as f64;
                assert!(
                    (got - expected).abs() < 1e-5 * (1.0 + expected.abs()),
                    "{cols}/{group} row {r}: {got} vs {expected}"
                );
                // And close to the unquantized input's product, relative to the vectors' sizes
                // since the products cancel.
                let exact = w.dot_row(&x, r) as f64;
                let mut row = vec![0f32; cols];
                w.row_into(r, &mut row);
                let norm = |v: &[f32]| v.iter().map(|a| (a * a) as f64).sum::<f64>().sqrt();
                let bound = 1e-3 * norm(&x) * norm(&row);
                assert!(
                    (got - exact).abs() < bound,
                    "{got} vs {exact}, bound {bound}"
                );
            }
        }
    }

    #[test]
    fn quantization_error_is_within_half_a_step() {
        let f = f16_matrix(40, 96);
        let q = f.quantize(32);
        let (mut a, mut b) = (vec![0f32; 96], vec![0f32; 96]);
        let Weights::Q8(q8) = &q.data else {
            unreachable!()
        };
        for r in 0..40 {
            f.row_into(r, &mut a);
            q.row_into(r, &mut b);
            for (k, (x, y)) in a.iter().zip(&b).enumerate() {
                let step = q8.scales[r * 3 + k / 32].to_f32();
                assert!((x - y).abs() <= step * 0.501, "{x} vs {y}, step {step}");
            }
        }
    }

    fn f16_matrix(rows: usize, cols: usize) -> Matrix {
        Matrix {
            rows,
            cols,
            data: Weights::F16(
                (0..rows * cols)
                    .map(|i| f16::from_f32(((i * 7919) % 1000) as f32 / 1000.0 - 0.5))
                    .collect(),
            ),
        }
    }

    fn matmul_case(w: &Matrix, n: usize) {
        let (rows, cols) = (w.rows, w.cols);
        let xs: Vec<f32> = (0..n * cols)
            .map(|i| ((i * 31) % 17) as f32 / 17.0)
            .collect();
        let mut ys = vec![0.0; n * rows];
        matmul(w, &xs, &mut ys, n);
        let mut y = vec![0.0; rows];
        for t in 0..n {
            // The batched kernel computes what `dot_row` does, in f32 throughout; decode's matvec
            // of an 8-bit matrix quantizes its input as well.
            let x = &xs[t * cols..(t + 1) * cols];
            for (r, y) in y.iter_mut().enumerate() {
                *y = w.dot_row(x, r);
            }
            // The batched kernel may sum in a different order.
            for (a, b) in ys[t * rows..(t + 1) * rows].iter().zip(&y) {
                assert!((a - b).abs() < 1e-5 * (1.0 + b.abs()), "{a} vs {b}");
            }
        }
    }
}
