//! Prefill on the SME matrix unit (Apple M4 and later), written in inline assembly since Rust
//! has no SME intrinsics.
//!
//! The unit computes outer products into the ZA array: one FMOPA adds `a bᵀ` for two vectors
//! of sixteen f32 into a 16x16 tile, and four tiles in flight keep it busy. So `Y = X Wᵀ` is
//! built a column of the inputs at a time, as outer products of sixteen tokens' values with
//! sixteen rows' weights. Both operands are packed to make those columns contiguous: tokens
//! and weight rows are transposed into panels of 32, so one column of a panel is two vectors,
//! and a block of 32 tokens by 32 rows takes four FMOPAs per column, one per tile. The
//! horizontal slices of a finished tile are sixteen adjacent outputs of one token, which
//! store straight into `Y`.
//!
//! Attention over a batch of queries is two such products per head. The scores come out
//! transposed, keys by queries, from `K` and the queries in the roles of `X` and `W`; that is
//! the layout the second product takes its inputs in, and it lets the softmax run down the
//! columns with vectors across queries and no horizontal sums.
//!
//! Streaming mode replaces the vector registers and forbids most NEON instructions, so each
//! kernel is one asm block from `smstart` to `smstop` that clobbers every vector and
//! predicate register; no compiler-generated code runs in between. The ZA state it turns on
//! is never live across a call, and no caller up the stack holds ZA state of its own, so
//! there is no lazy save to honour.

use super::{exp, pack_f16, pack_f32, pack_rows};
use crate::kernels::Matrix;
use half::{f16, slice::HalfFloatSliceExt};
use rayon::prelude::*;
use std::arch::aarch64::*;
use std::arch::asm;
use std::cell::RefCell;
use std::sync::OnceLock;

/// Tokens or weight rows per panel: two vectors of sixteen f32, at the 512-bit streaming
/// vector length the kernels are written for.
const PANEL: usize = 32;
/// The streaming vector length the kernels assume, in bytes.
const SVL_BYTES: u64 = 64;

/// Whether the CPU has SME2 at the vector length the kernels assume. `INFER_NO_SME` set to
/// anything turns them off, for comparisons.
pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        if std::env::var_os("INFER_NO_SME").is_some() || !has_sme2() {
            return false;
        }
        let svl: u64;
        // Safety: the CPU has SME, and RDSVL only reads the vector length.
        unsafe {
            asm!(".arch_extension sme", "rdsvl {0}, #1", out(reg) svl, options(nomem, nostack))
        };
        svl == SVL_BYTES
    })
}

#[cfg(target_os = "macos")]
fn has_sme2() -> bool {
    unsafe extern "C" {
        fn sysctlbyname(
            name: *const std::ffi::c_char,
            oldp: *mut std::ffi::c_void,
            oldlenp: *mut usize,
            newp: *mut std::ffi::c_void,
            newlen: usize,
        ) -> std::ffi::c_int;
    }
    let mut value: u32 = 0;
    let mut len = size_of::<u32>();
    // Safety: the name is NUL-terminated and the output is a u32 of the length given.
    let status = unsafe {
        sysctlbyname(
            c"hw.optional.arm.FEAT_SME2".as_ptr(),
            (&raw mut value).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    status == 0 && value != 0
}

#[cfg(target_os = "linux")]
fn has_sme2() -> bool {
    unsafe extern "C" {
        fn getauxval(kind: std::ffi::c_ulong) -> std::ffi::c_ulong;
    }
    const AT_HWCAP2: std::ffi::c_ulong = 26;
    const HWCAP2_SME2: std::ffi::c_ulong = 1 << 37;
    // Safety: getauxval has no preconditions.
    unsafe { getauxval(AT_HWCAP2) & HWCAP2_SME2 != 0 }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn has_sme2() -> bool {
    false
}

/// Runs `$body` in streaming mode with ZA on, clobbering every register streaming mode
/// touches.
macro_rules! streaming {
    ($($body:tt)*) => {
        asm!(
            ".arch_extension sme2",
            "smstart",
            $($body)*
            out("v0") _, out("v1") _, out("v2") _, out("v3") _,
            out("v4") _, out("v5") _, out("v6") _, out("v7") _,
            out("v8") _, out("v9") _, out("v10") _, out("v11") _,
            out("v12") _, out("v13") _, out("v14") _, out("v15") _,
            out("v16") _, out("v17") _, out("v18") _, out("v19") _,
            out("v20") _, out("v21") _, out("v22") _, out("v23") _,
            out("v24") _, out("v25") _, out("v26") _, out("v27") _,
            out("v28") _, out("v29") _, out("v30") _, out("v31") _,
            out("p0") _, out("p1") _, out("p2") _, out("p3") _,
            out("p4") _, out("p5") _, out("p6") _, out("p7") _,
            out("p8") _, out("p9") _, out("p10") _, out("p11") _,
            out("p12") _, out("p13") _, out("p14") _, out("p15") _,
            out("ffr") _,
            options(nostack),
        )
    };
}

/// `y[t][r] = Σₖ x[k][t] w[k][r]` for `t < tokens` and `r < rows`, both at most [`PANEL`]:
/// `x` and `w` are panels of `cols` columns, and row `t` of `y` starts `ldy` floats after
/// row `t - 1`.
///
/// # Safety
/// The CPU must have SME2 at a 512-bit vector length; `x` and `w` must hold `cols * PANEL`
/// values, and `y` the outputs written.
unsafe fn block(
    x: *const f32,
    w: *const f32,
    cols: usize,
    y: *mut f32,
    ldy: usize,
    tokens: usize,
    rows: usize,
) {
    debug_assert!((1..=PANEL).contains(&tokens) && (1..=PANEL).contains(&rows));
    // Tiles 0 and 1 hold the first sixteen tokens, 2 and 3 the rest; tiles 0 and 2 the
    // first sixteen rows, 1 and 3 the rest.
    let (low, high) = (tokens.min(16), tokens.saturating_sub(16));
    unsafe {
        streaming!(
            "ptrue p0.s",
            "ptrue pn8.s",
            // Store predicates for the two halves of the rows.
            "mov x9, #16",
            "whilelt p1.s, xzr, {rows}",
            "whilelt p2.s, x9, {rows}",
            "zero {{za}}",
            // Four columns per iteration, each two vectors of inputs and two of weights and
            // an FMOPA into every tile.
            "lsr x10, {cols}, #2",
            "cbz x10, 3f",
            "2:",
            "ld1w {{z0.s-z1.s}}, pn8/z, [{x}]",
            "ld1w {{z2.s-z3.s}}, pn8/z, [{w}]",
            "ld1w {{z4.s-z5.s}}, pn8/z, [{x}, #2, mul vl]",
            "ld1w {{z6.s-z7.s}}, pn8/z, [{w}, #2, mul vl]",
            "ld1w {{z8.s-z9.s}}, pn8/z, [{x}, #4, mul vl]",
            "ld1w {{z10.s-z11.s}}, pn8/z, [{w}, #4, mul vl]",
            "ld1w {{z12.s-z13.s}}, pn8/z, [{x}, #6, mul vl]",
            "ld1w {{z14.s-z15.s}}, pn8/z, [{w}, #6, mul vl]",
            "fmopa za0.s, p0/m, p0/m, z0.s, z2.s",
            "fmopa za1.s, p0/m, p0/m, z0.s, z3.s",
            "fmopa za2.s, p0/m, p0/m, z1.s, z2.s",
            "fmopa za3.s, p0/m, p0/m, z1.s, z3.s",
            "fmopa za0.s, p0/m, p0/m, z4.s, z6.s",
            "fmopa za1.s, p0/m, p0/m, z4.s, z7.s",
            "fmopa za2.s, p0/m, p0/m, z5.s, z6.s",
            "fmopa za3.s, p0/m, p0/m, z5.s, z7.s",
            "fmopa za0.s, p0/m, p0/m, z8.s, z10.s",
            "fmopa za1.s, p0/m, p0/m, z8.s, z11.s",
            "fmopa za2.s, p0/m, p0/m, z9.s, z10.s",
            "fmopa za3.s, p0/m, p0/m, z9.s, z11.s",
            "fmopa za0.s, p0/m, p0/m, z12.s, z14.s",
            "fmopa za1.s, p0/m, p0/m, z12.s, z15.s",
            "fmopa za2.s, p0/m, p0/m, z13.s, z14.s",
            "fmopa za3.s, p0/m, p0/m, z13.s, z15.s",
            "add {x}, {x}, #512",
            "add {w}, {w}, #512",
            "subs x10, x10, #1",
            "b.ne 2b",
            // The remaining columns one at a time.
            "3:",
            "ands x10, {cols}, #3",
            "b.eq 4f",
            "8:",
            "ld1w {{z0.s-z1.s}}, pn8/z, [{x}]",
            "ld1w {{z2.s-z3.s}}, pn8/z, [{w}]",
            "fmopa za0.s, p0/m, p0/m, z0.s, z2.s",
            "fmopa za1.s, p0/m, p0/m, z0.s, z3.s",
            "fmopa za2.s, p0/m, p0/m, z1.s, z2.s",
            "fmopa za3.s, p0/m, p0/m, z1.s, z3.s",
            "add {x}, {x}, #128",
            "add {w}, {w}, #128",
            "subs x10, x10, #1",
            "b.ne 8b",
            "4:",
            // Each horizontal slice is one token's outputs for sixteen rows.
            "mov x11, {y}",
            "mov w12, #0",
            "5:",
            "st1w {{za0h.s[w12, 0]}}, p1, [x11]",
            "add x13, x11, #64",
            "st1w {{za1h.s[w12, 0]}}, p2, [x13]",
            "add x11, x11, {ldy}",
            "add w12, w12, #1",
            "cmp x12, {low}",
            "b.lo 5b",
            "cbz {high}, 7f",
            "mov w12, #0",
            "6:",
            "st1w {{za2h.s[w12, 0]}}, p1, [x11]",
            "add x13, x11, #64",
            "st1w {{za3h.s[w12, 0]}}, p2, [x13]",
            "add x11, x11, {ldy}",
            "add w12, w12, #1",
            "cmp x12, {high}",
            "b.lo 6b",
            "7:",
            "smstop",
            x = inout(reg) x => _,
            w = inout(reg) w => _,
            cols = in(reg) cols,
            y = in(reg) y,
            ldy = in(reg) ldy * 4,
            rows = in(reg) rows,
            low = in(reg) low,
            high = in(reg) high,
            out("x9") _, out("x10") _, out("x11") _, out("x12") _, out("x13") _,
        );
    }
}

thread_local! {
    /// Weight rows converted to f32 and packed for the block a thread is working on.
    static SCRATCH: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
}

/// Weight rows per task, packed once and applied to every token panel in its block. The
/// matrix unit reads from L2, so this only needs to stay well inside that; smaller blocks
/// make enough tasks to share out in a short prefill.
const ROW_BLOCK: usize = 64;
/// Tokens per task.
const TOKEN_BLOCK: usize = 256;

/// `Y = X Wᵀ` for `n` vectors, as [`crate::kernels::matmul`].
pub fn matmul(w: &Matrix, xs: &[f32], ys: &mut [f32], n: usize) {
    let (rows, cols) = (w.rows, w.cols);
    // Every task reads every token panel of its block, so the inputs are packed once up front.
    let mut packed_xs = vec![0f32; n.div_ceil(PANEL) * PANEL * cols];
    packed_xs
        .par_chunks_mut(PANEL * cols)
        .enumerate()
        .for_each(|(p, dst)| {
            let t = p * PANEL;
            let tokens = PANEL.min(n - t);
            pack_f32::<PANEL>(&xs[t * cols..(t + tokens) * cols], tokens, cols, cols, dst);
        });
    let row_blocks = rows.div_ceil(ROW_BLOCK);
    let tok_blocks = n.div_ceil(TOKEN_BLOCK);
    let ys_ptr = ys.as_mut_ptr() as usize;
    let packed_xs = &packed_xs;
    (0..row_blocks * tok_blocks)
        .into_par_iter()
        .for_each(|task| {
            let (rb, tb) = (task / tok_blocks, task % tok_blocks);
            let r0 = rb * ROW_BLOCK;
            let r1 = (r0 + ROW_BLOCK).min(rows);
            let t0 = tb * TOKEN_BLOCK;
            let t1 = (t0 + TOKEN_BLOCK).min(n);
            SCRATCH.with_borrow_mut(|scratch| {
                scratch.resize(ROW_BLOCK * cols, 0.0);
                for (g, packed) in scratch
                    .chunks_exact_mut(PANEL * cols)
                    .take((r1 - r0).div_ceil(PANEL))
                    .enumerate()
                {
                    let r = r0 + g * PANEL;
                    // Safety: the rows packed, at most r1 - r, are in the matrix.
                    unsafe { pack_rows::<PANEL>(w, r, PANEL.min(r1 - r), packed) };
                }
                for t in (t0..t1).step_by(PANEL) {
                    let x = &packed_xs[t * cols..(t + PANEL) * cols];
                    for (g, r) in (r0..r1).step_by(PANEL).enumerate() {
                        // Safety: `available` checked the CPU, both panels are full, and the
                        // outputs are in `ys`, each (t, r) cell written by exactly one task.
                        unsafe {
                            block(
                                x.as_ptr(),
                                scratch.as_ptr().add(g * PANEL * cols),
                                cols,
                                (ys_ptr as *mut f32).add(t * rows + r),
                                rows,
                                PANEL.min(t1 - t),
                                PANEL.min(r1 - r),
                            )
                        };
                    }
                }
            });
        });
}

/// Causal attention for `n` queries in every head, as [`crate::kernels::attend_batch`], with
/// `hd` a multiple of [`PANEL`].
#[allow(clippy::too_many_arguments)]
pub fn attend(
    q: &[f32],
    q_stride: usize,
    keys: &[&[f16]],
    values: &[&[f16]],
    hd: usize,
    n: usize,
    scale: f32,
    out: &mut [f32],
    out_stride: usize,
) {
    assert!(hd.is_multiple_of(PANEL));
    let heads = keys.len();
    let len = keys[0].len() / hd;
    let start = len - n;
    assert!(q.len() >= (n - 1) * q_stride + heads * hd);
    assert!(out.len() >= (n - 1) * out_stride + heads * hd);
    // Keys transposed into panels of 32, as the queries will be.
    let key_panels = len.div_ceil(PANEL);
    let mut packed_keys = vec![0f32; heads * key_panels * PANEL * hd];
    packed_keys
        .par_chunks_mut(PANEL * hd)
        .enumerate()
        .for_each(|(i, dst)| {
            let (h, k0) = (i / key_panels, i % key_panels * PANEL);
            let kn = PANEL.min(len - k0);
            pack_f16::<PANEL>(&keys[h][k0 * hd..(k0 + kn) * hd], kn, hd, dst);
        });
    // Values are already a row per key, so they are only split into panels of 32 dimensions.
    let halves = hd / PANEL;
    let mut packed_values = vec![0f32; heads * halves * len * PANEL];
    packed_values
        .par_chunks_mut(len * PANEL)
        .enumerate()
        .for_each(|(i, dst)| {
            let (h, half) = (i / halves, i % halves);
            for (k, dst) in dst.as_chunks_mut::<PANEL>().0.iter_mut().enumerate() {
                values[h][k * hd + half * PANEL..][..PANEL].convert_to_f32_slice(dst);
            }
        });
    let (packed_keys, packed_values) = (&packed_keys, &packed_values);
    let q_panels = n.div_ceil(PANEL);
    let out_ptr = out.as_mut_ptr() as usize;
    // Later queries attend to more keys; they go first so the last tasks are short.
    (0..heads * q_panels).into_par_iter().for_each(|i| {
        let (h, t0) = (i % heads, (q_panels - 1 - i / heads) * PANEL);
        let queries = PANEL.min(n - t0);
        let first = start + t0;
        let keys_here = first + queries;
        let panels_here = keys_here.div_ceil(PANEL);
        ATTENTION_SCRATCH.with_borrow_mut(|scratch| {
            scratch.resize((hd + panels_here * PANEL) * PANEL, 0.0);
            let (packed_q, scores) = scratch.split_at_mut(hd * PANEL);
            pack_f32::<PANEL>(
                &q[t0 * q_stride + h * hd..],
                queries,
                hd,
                q_stride,
                packed_q,
            );
            for j in 0..panels_here {
                // Safety: `available` checked the CPU; both panels are full and the scores
                // have room for every key panel.
                unsafe {
                    block(
                        packed_keys.as_ptr().add((h * key_panels + j) * PANEL * hd),
                        packed_q.as_ptr(),
                        hd,
                        scores.as_mut_ptr().add(j * PANEL * PANEL),
                        PANEL,
                        PANEL,
                        PANEL,
                    )
                };
            }
            let inv_sums = softmax_columns(scores, keys_here, first, queries, scale);
            for half in 0..halves {
                let y = out_ptr as *mut f32;
                // Safety: the weights cover `keys_here` keys and the values `len`; each task
                // writes only its own queries' outputs for its own head.
                unsafe {
                    block(
                        scores.as_ptr(),
                        packed_values
                            .as_ptr()
                            .add((h * halves + half) * len * PANEL),
                        keys_here,
                        y.add(t0 * out_stride + h * hd + half * PANEL),
                        out_stride,
                        queries,
                        PANEL,
                    )
                };
            }
            for (t, &inv) in inv_sums[..queries].iter().enumerate() {
                // Safety: as for the products, these are this task's outputs.
                let row = unsafe {
                    std::slice::from_raw_parts_mut(
                        (out_ptr as *mut f32).add((t0 + t) * out_stride + h * hd),
                        hd,
                    )
                };
                for o in row {
                    *o *= inv;
                }
            }
        });
    });
}

thread_local! {
    /// A panel of queries and its scores against every key, for the attention task a thread is
    /// working on.
    static ATTENTION_SCRATCH: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
}

/// Turns scores, `keys` rows of [`PANEL`] queries, into attention weights in place, scaled by
/// `scale` before the softmax. Query `c` is at position `first + c` and sees the keys up to
/// it; its weights for later keys become zero. The weights are left unnormalized and the
/// reciprocals of their sums returned, one per query, since scaling the outputs afterwards
/// is cheaper than another pass over the weights.
fn softmax_columns(
    scores: &mut [f32],
    keys: usize,
    first: usize,
    queries: usize,
    scale: f32,
) -> [f32; PANEL] {
    assert!(scores.len() >= keys * PANEL && keys >= first + queries);
    let mut inv_sums = [0f32; PANEL];
    let sp = scores.as_mut_ptr();
    // Four queries at a time, one per lane.
    for c in (0..queries).step_by(4) {
        unsafe {
            let positions = vaddq_u32(
                vdupq_n_u32((first + c) as u32),
                vld1q_u32([0u32, 1, 2, 3].as_ptr()),
            );
            let visible = |k: usize| vcleq_u32(vdupq_n_u32(k as u32), positions);
            // Keys up to `first + c` are visible to every lane; the next three to some.
            let all_see = first + c + 1;
            let any_see = (first + c + 4).min(keys);
            let mut max = vdupq_n_f32(f32::NEG_INFINITY);
            for k in 0..all_see {
                max = vmaxq_f32(max, vld1q_f32(sp.add(k * PANEL + c)));
            }
            for k in all_see..any_see {
                let s = vld1q_f32(sp.add(k * PANEL + c));
                max = vbslq_f32(visible(k), vmaxq_f32(max, s), max);
            }
            let shift = vmulq_n_f32(max, scale);
            let mut sum = vdupq_n_f32(0.0);
            for k in 0..all_see {
                let p = sp.add(k * PANEL + c);
                let e = exp(vfmsq_n_f32(vnegq_f32(shift), vld1q_f32(p), -scale));
                vst1q_f32(p, e);
                sum = vaddq_f32(sum, e);
            }
            for k in all_see..keys {
                let p = sp.add(k * PANEL + c);
                let e = exp(vfmsq_n_f32(vnegq_f32(shift), vld1q_f32(p), -scale));
                let e = vreinterpretq_f32_u32(vandq_u32(vreinterpretq_u32_f32(e), visible(k)));
                vst1q_f32(p, e);
                sum = vaddq_f32(sum, e);
            }
            vst1q_f32(
                inv_sums.as_mut_ptr().add(c),
                vdivq_f32(vdupq_n_f32(1.0), sum),
            );
        }
    }
    inv_sums
}
