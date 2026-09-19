//! The model: weight loading, the forward pass over a key/value cache, and the session that
//! keeps the cache aligned with an editor's token sequence.

use crate::kernels::{Matrix, matmul, matvec, rms_norm, rope, silu, softmax};
use anyhow::{Context, Result, bail};
use half::f16;
use rayon::prelude::*;
use safetensors::{Dtype, SafeTensors};
use serde::Deserialize;
use std::fs;
use std::path::Path;

const RMS_EPS: f32 = 1e-5;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub vocab_size: usize,
    pub d_model: usize,
    pub n_layers: usize,
    pub n_heads: usize,
    pub d_ff: usize,
    pub max_seq_len: usize,
    pub rope_theta: f32,
}

impl Config {
    pub fn head_dim(&self) -> usize {
        self.d_model / self.n_heads
    }
}

pub struct Layer {
    attn_norm: Vec<f32>,
    /// `[3 * d_model, d_model]`: q, k and v projections stacked.
    wqkv: Matrix,
    wo: Matrix,
    ffn_norm: Vec<f32>,
    /// `[2 * d_ff, d_model]`: gate then up.
    wgu: Matrix,
    /// `[d_model, d_ff]`.
    wdown: Matrix,
}

pub struct Model {
    pub config: Config,
    /// Decode is memory-bound and saturates with a few threads; prefill is compute-bound and
    /// uses every core. Each forward runs inside the pool that suits its batch size.
    decode_pool: rayon::ThreadPool,
    prefill_pool: rayon::ThreadPool,
    /// `[vocab, d_model]`, used both for lookup and as the tied output projection.
    embedding: Matrix,
    layers: Vec<Layer>,
    norm: Vec<f32>,
    /// Rotary tables, `[max_seq_len, head_dim / 2]`.
    rope_cos: Vec<f32>,
    rope_sin: Vec<f32>,
}

/// Reads a tensor as f16, whatever precision it was stored in.
fn read_f16(tensors: &SafeTensors, name: &str) -> Result<(Vec<usize>, Vec<f16>)> {
    let view = tensors
        .tensor(name)
        .with_context(|| format!("missing tensor {name}"))?;
    let bytes = view.data();
    let values = match view.dtype() {
        Dtype::F16 => bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| f16::from_bits(u16::from_le_bytes(*b)))
            .collect(),
        Dtype::BF16 => bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| f16::from_f32(half::bf16::from_bits(u16::from_le_bytes(*b)).to_f32()))
            .collect(),
        Dtype::F32 => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f16::from_f32(f32::from_le_bytes(*b)))
            .collect(),
        other => bail!("tensor {name} has unsupported dtype {other:?}"),
    };
    Ok((view.shape().to_vec(), values))
}

fn read_f32(tensors: &SafeTensors, name: &str) -> Result<Vec<f32>> {
    let (_, values) = read_f16(tensors, name)?;
    Ok(values.into_iter().map(f16::to_f32).collect())
}

/// Loads a linear weight `[out, in]`.
fn read_linear(tensors: &SafeTensors, name: &str) -> Result<Matrix> {
    let (shape, data) = read_f16(tensors, name)?;
    let [d_out, d_in] = shape[..] else {
        bail!("{name} is not 2-D")
    };
    Ok(Matrix {
        rows: d_out,
        cols: d_in,
        data,
    })
}

fn stack(parts: Vec<Matrix>) -> Matrix {
    let cols = parts[0].cols;
    let rows = parts.iter().map(|m| m.rows).sum();
    let mut data = Vec::with_capacity(rows * cols);
    for m in parts {
        assert_eq!(m.cols, cols);
        data.extend_from_slice(&m.data);
    }
    Matrix { rows, cols, data }
}

impl Model {
    /// A model with pseudo-random weights, for tests.
    pub fn random(config: Config, seed: u64) -> Model {
        let mut state = seed;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f32 / (1u64 << 53) as f32 * 0.2 - 0.1
        };
        let mut matrix = |rows: usize, cols: usize| Matrix {
            rows,
            cols,
            data: (0..rows * cols).map(|_| f16::from_f32(next())).collect(),
        };
        let d = config.d_model;
        let layers = (0..config.n_layers)
            .map(|_| Layer {
                attn_norm: vec![1.0; d],
                wqkv: matrix(3 * d, d),
                wo: matrix(d, d),
                ffn_norm: vec![1.0; d],
                wgu: matrix(2 * config.d_ff, d),
                wdown: matrix(d, config.d_ff),
            })
            .collect();
        let embedding = matrix(config.vocab_size, d);
        let (rope_cos, rope_sin) = rope_tables(&config);
        let (decode_pool, prefill_pool) = thread_pools();
        Model {
            norm: vec![1.0; d],
            config,
            decode_pool,
            prefill_pool,
            embedding,
            layers,
            rope_cos,
            rope_sin,
        }
    }

    /// Loads `config.json` and `model.safetensors` from a checkpoint directory.
    pub fn load(dir: &Path) -> Result<Model> {
        let config: Config = serde_json::from_str(&fs::read_to_string(dir.join("config.json"))?)?;
        let bytes = fs::read(dir.join("model.safetensors"))
            .with_context(|| format!("reading weights in {}", dir.display()))?;
        let tensors = SafeTensors::deserialize(&bytes)?;
        let (shape, values) = read_f16(&tensors, "embedding.weight")?;
        if shape != [config.vocab_size, config.d_model] {
            bail!("embedding shape {shape:?} does not match config");
        }
        let embedding = Matrix {
            rows: config.vocab_size,
            cols: config.d_model,
            data: values,
        };
        let mut layers = Vec::with_capacity(config.n_layers);
        for i in 0..config.n_layers {
            let p = |s: &str| format!("layers.{i}.{s}");
            layers.push(Layer {
                attn_norm: read_f32(&tensors, &p("attn_norm.weight"))?,
                wqkv: stack(vec![
                    read_linear(&tensors, &p("attn.wq.weight"))?,
                    read_linear(&tensors, &p("attn.wk.weight"))?,
                    read_linear(&tensors, &p("attn.wv.weight"))?,
                ]),
                wo: read_linear(&tensors, &p("attn.wo.weight"))?,
                ffn_norm: read_f32(&tensors, &p("ffn_norm.weight"))?,
                wgu: stack(vec![
                    read_linear(&tensors, &p("ffn.gate.weight"))?,
                    read_linear(&tensors, &p("ffn.up.weight"))?,
                ]),
                wdown: read_linear(&tensors, &p("ffn.down.weight"))?,
            });
        }
        let (rope_cos, rope_sin) = rope_tables(&config);
        let (decode_pool, prefill_pool) = thread_pools();
        Ok(Model {
            norm: read_f32(&tensors, "norm.weight")?,
            config,
            decode_pool,
            prefill_pool,
            embedding,
            layers,
            rope_cos,
            rope_sin,
        })
    }

    /// Total weight bytes read per generated token.
    pub fn weight_bytes(&self) -> usize {
        self.embedding.bytes() * 2
            + self
                .layers
                .iter()
                .map(|l| l.wqkv.bytes() + l.wo.bytes() + l.wgu.bytes() + l.wdown.bytes())
                .sum::<usize>()
    }

    pub fn new_cache(&self) -> KvCache {
        KvCache::new(&self.config)
    }

    /// Runs `tokens` at positions `cache.len()..` and returns logits for every token if
    /// `all_logits`, otherwise only for the last one. The cache is extended.
    pub fn forward(&self, tokens: &[u32], cache: &mut KvCache, all_logits: bool) -> Vec<f32> {
        let pool = if tokens.len() == 1 {
            &self.decode_pool
        } else {
            &self.prefill_pool
        };
        pool.install(|| self.forward_inner(tokens, cache, all_logits))
    }

    pub fn threads(&self) -> (usize, usize) {
        (
            self.decode_pool.current_num_threads(),
            self.prefill_pool.current_num_threads(),
        )
    }

    fn forward_inner(&self, tokens: &[u32], cache: &mut KvCache, all_logits: bool) -> Vec<f32> {
        let c = &self.config;
        let (d, n, heads, hd) = (c.d_model, tokens.len(), c.n_heads, c.head_dim());
        let start = cache.len;
        assert!(start + n <= c.max_seq_len, "context overflow");
        let half_dim = hd / 2;
        let scale = 1.0 / (hd as f32).sqrt();

        // Residual stream, one row per token.
        let mut x = vec![0f32; n * d];
        for (t, &tok) in tokens.iter().enumerate() {
            for (o, &w) in x[t * d..(t + 1) * d]
                .iter_mut()
                .zip(self.embedding.row(tok as usize))
            {
                *o = w.to_f32();
            }
        }
        let mut h = vec![0f32; n * d];
        let mut qkv = vec![0f32; n * 3 * d];
        let mut attn = vec![0f32; n * d];
        let mut o = vec![0f32; n * d];
        let mut gu = vec![0f32; n * 2 * c.d_ff];
        let mut act = vec![0f32; n * c.d_ff];

        for (li, layer) in self.layers.iter().enumerate() {
            for t in 0..n {
                rms_norm(
                    &x[t * d..(t + 1) * d],
                    &layer.attn_norm,
                    RMS_EPS,
                    &mut h[t * d..(t + 1) * d],
                );
            }
            project(&layer.wqkv, &h, &mut qkv, n);
            // Rotary embeddings on q and k, then k and v into the cache.
            let (k_cache, v_cache) = cache.layer_mut(li);
            for t in 0..n {
                let pos = start + t;
                let (cos, sin) = (
                    &self.rope_cos[pos * half_dim..(pos + 1) * half_dim],
                    &self.rope_sin[pos * half_dim..(pos + 1) * half_dim],
                );
                let row = &mut qkv[t * 3 * d..(t + 1) * 3 * d];
                for head in 0..heads {
                    rope(&mut row[head * hd..(head + 1) * hd], cos, sin);
                    rope(&mut row[d + head * hd..d + (head + 1) * hd], cos, sin);
                }
                k_cache[pos * d..(pos + 1) * d].copy_from_slice(&row[d..2 * d]);
                v_cache[pos * d..(pos + 1) * d].copy_from_slice(&row[2 * d..3 * d]);
            }
            // Attention: every (token, head) pair is independent.
            let k_cache: &[f32] = k_cache;
            let v_cache: &[f32] = v_cache;
            let qkv_ref = &qkv;
            attn.par_chunks_mut(hd).enumerate().for_each(|(idx, out)| {
                let (t, head) = (idx / heads, idx % heads);
                let pos = start + t;
                let q = &qkv_ref[t * 3 * d + head * hd..t * 3 * d + (head + 1) * hd];
                let mut scores = vec![0f32; pos + 1];
                for (p, s) in scores.iter_mut().enumerate() {
                    let k = &k_cache[p * d + head * hd..p * d + (head + 1) * hd];
                    *s = q.iter().zip(k).map(|(a, b)| a * b).sum::<f32>() * scale;
                }
                softmax(&mut scores);
                out.fill(0.0);
                for (p, &w) in scores.iter().enumerate() {
                    let v = &v_cache[p * d + head * hd..p * d + (head + 1) * hd];
                    for (o, &vv) in out.iter_mut().zip(v) {
                        *o += w * vv;
                    }
                }
            });
            project(&layer.wo, &attn, &mut o, n);
            for (xv, ov) in x.iter_mut().zip(&o) {
                *xv += ov;
            }
            for t in 0..n {
                rms_norm(
                    &x[t * d..(t + 1) * d],
                    &layer.ffn_norm,
                    RMS_EPS,
                    &mut h[t * d..(t + 1) * d],
                );
            }
            project(&layer.wgu, &h, &mut gu, n);
            let ff = c.d_ff;
            for t in 0..n {
                let row = &gu[t * 2 * ff..(t + 1) * 2 * ff];
                for (a, (&g, &u)) in act[t * ff..(t + 1) * ff]
                    .iter_mut()
                    .zip(row[..ff].iter().zip(&row[ff..]))
                {
                    *a = silu(g) * u;
                }
            }
            project(&layer.wdown, &act, &mut o, n);
            for (xv, ov) in x.iter_mut().zip(&o) {
                *xv += ov;
            }
        }
        cache.len += n;

        let first = if all_logits { 0 } else { n - 1 };
        let mut logits = vec![0f32; (n - first) * c.vocab_size];
        for t in first..n {
            rms_norm(
                &x[t * d..(t + 1) * d],
                &self.norm,
                RMS_EPS,
                &mut h[t * d..(t + 1) * d],
            );
        }
        project(&self.embedding, &h[first * d..], &mut logits, n - first);
        logits
    }
}

/// Threads for decode, beyond which memory bandwidth on Apple silicon is already saturated
/// and dispatch overhead grows. Overridable with `INFER_DECODE_THREADS`.
const DECODE_THREADS: usize = 4;

fn thread_pools() -> (rayon::ThreadPool, rayon::ThreadPool) {
    let available = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let decode = std::env::var("INFER_DECODE_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DECODE_THREADS)
        .clamp(1, available);
    let prefill = std::env::var("INFER_PREFILL_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(available)
        .clamp(1, available);
    let build = |n: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .expect("thread pool")
    };
    (build(decode), build(prefill))
}

/// Cosine and sine tables for the interleaved rotary embedding, `[max_seq_len, head_dim / 2]`.
fn rope_tables(config: &Config) -> (Vec<f32>, Vec<f32>) {
    let half_dim = config.head_dim() / 2;
    let mut cos = Vec::with_capacity(config.max_seq_len * half_dim);
    let mut sin = Vec::with_capacity(config.max_seq_len * half_dim);
    for pos in 0..config.max_seq_len {
        for i in 0..half_dim {
            let theta =
                1.0 / (config.rope_theta as f64).powf(2.0 * i as f64 / config.head_dim() as f64);
            let angle = pos as f64 * theta;
            cos.push(angle.cos() as f32);
            sin.push(angle.sin() as f32);
        }
    }
    (cos, sin)
}

/// Applies a weight matrix to `n` vectors, using the single-vector path when `n == 1`.
fn project(w: &Matrix, xs: &[f32], ys: &mut [f32], n: usize) {
    if n == 1 {
        matvec(w, &xs[..w.cols], &mut ys[..w.rows]);
    } else {
        matmul(w, &xs[..n * w.cols], &mut ys[..n * w.rows], n);
    }
}

/// Keys and values for every layer, `[max_seq_len, d_model]` per layer, f32.
pub struct KvCache {
    k: Vec<Vec<f32>>,
    v: Vec<Vec<f32>>,
    len: usize,
}

impl KvCache {
    fn new(config: &Config) -> KvCache {
        let size = config.max_seq_len * config.d_model;
        KvCache {
            k: (0..config.n_layers).map(|_| vec![0.0; size]).collect(),
            v: (0..config.n_layers).map(|_| vec![0.0; size]).collect(),
            len: 0,
        }
    }

    fn layer_mut(&mut self, layer: usize) -> (&mut [f32], &mut [f32]) {
        (&mut self.k[layer], &mut self.v[layer])
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn truncate(&mut self, len: usize) {
        self.len = self.len.min(len);
    }
}

/// A model context tracking the editor's token sequence and its cache together.
/// Feeding runs only the new tokens, deletions roll back to the common prefix,
/// and overflow keeps the most recent window.
pub struct Session<'a> {
    model: &'a Model,
    tokens: Vec<u32>,
    cache: KvCache,
    last_logits: Option<Vec<f32>>,
}

impl<'a> Session<'a> {
    pub fn new(model: &'a Model) -> Session<'a> {
        Session {
            model,
            tokens: Vec::new(),
            cache: model.new_cache(),
            last_logits: None,
        }
    }

    pub fn tokens(&self) -> &[u32] {
        &self.tokens
    }

    pub fn last_logits(&self) -> Option<&[f32]> {
        self.last_logits.as_deref()
    }

    pub fn feed(&mut self, new: &[u32]) {
        if new.is_empty() {
            return;
        }
        let max = self.model.config.max_seq_len;
        if self.tokens.len() + new.len() > max {
            let mut all = std::mem::take(&mut self.tokens);
            all.extend_from_slice(new);
            let keep = all.len() - max;
            self.cache.truncate(0);
            self.last_logits = None;
            return self.feed(&all[keep..]);
        }
        self.last_logits = Some(self.model.forward(new, &mut self.cache, false));
        self.tokens.extend_from_slice(new);
    }

    pub fn truncate(&mut self, len: usize) {
        if len >= self.tokens.len() {
            return;
        }
        self.tokens.truncate(len);
        self.cache.truncate(len);
        self.last_logits = None;
    }

    pub fn sync(&mut self, tokens: &[u32]) {
        let common = self
            .tokens
            .iter()
            .zip(tokens)
            .take_while(|(a, b)| a == b)
            .count();
        if common == tokens.len() && common == self.tokens.len() && self.last_logits.is_some() {
            return;
        }
        let keep = if common == tokens.len() {
            common.saturating_sub(1)
        } else {
            common
        };
        self.truncate(keep);
        self.feed(&tokens[keep..]);
    }

    pub fn generate(&mut self, max_tokens: usize, stop: &[u32]) -> Vec<u32> {
        let mut out = Vec::new();
        for _ in 0..max_tokens {
            let Some(logits) = &self.last_logits else {
                break;
            };
            let next = argmax(logits);
            if stop.contains(&next) {
                break;
            }
            out.push(next);
            self.feed(&[next]);
        }
        out
    }
}

/// Token ids the completion loop needs to interpret the model's output, and the bytes each
/// token renders to.
#[derive(Clone, Debug)]
pub struct TokenSet {
    pub eom: u32,
    pub eos: u32,
    /// The line-break tokens, one per indent level.
    pub newline: std::ops::Range<u32>,
    /// What every token renders to, indexed by id, with line breaks including their
    /// indentation and specials empty. Constrains generation over a partially typed token.
    pub render: Vec<Vec<u8>>,
}

/// Limits and thresholds for a completion. An editor's "eagerness" setting maps onto these.
#[derive(Clone, Debug)]
pub struct CompletionOptions {
    pub max_lines: usize,
    pub max_tokens: usize,
    /// Stop at a line end when the model gives the end-of-middle token at least this much
    /// probability, even if it is not the top choice.
    pub stop_threshold: f32,
    /// Discard a line whose confidence is below this; for the first line that means no
    /// completion is offered.
    pub min_line_confidence: f32,
}

impl Default for CompletionOptions {
    fn default() -> Self {
        CompletionOptions {
            max_lines: 8,
            max_tokens: 256,
            stop_threshold: 0.2,
            min_line_confidence: 0.0,
        }
    }
}

/// One generated line: its content tokens, the line-break token that followed it if the
/// completion continued, and how sure the model was.
#[derive(Clone, Debug, Default)]
pub struct Line {
    pub tokens: Vec<u32>,
    pub newline: Option<u32>,
    /// Geometric mean of the content tokens' probabilities, 0 to 1.
    pub confidence: f32,
    /// Probability of the least likely content token.
    pub min_prob: f32,
    /// Probability of the end-of-middle token where this line ended.
    pub stop_prob: f32,
    logprob_sum: f32,
}

impl Line {
    fn push(&mut self, token: u32, prob: f32) {
        self.tokens.push(token);
        self.logprob_sum += prob.max(1e-30).ln();
        self.min_prob = if self.tokens.len() == 1 {
            prob
        } else {
            self.min_prob.min(prob)
        };
        self.confidence = (self.logprob_sum / self.tokens.len() as f32).exp();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The model chose the end-of-middle or end-of-text token.
    EndToken,
    /// The end-of-middle probability at a line end reached the threshold.
    StopThreshold,
    /// The next line would have repeated the first line of the suffix.
    SuffixDuplicate,
    /// The next line repeated an earlier generated line.
    Repetition,
    /// The next line's confidence was below the minimum.
    LowConfidence,
    MaxLines,
    MaxTokens,
}

#[derive(Clone, Debug)]
pub struct Completion {
    pub lines: Vec<Line>,
    pub reason: StopReason,
}

impl Completion {
    /// Every generated token in order, line breaks included, for rendering as one piece.
    pub fn tokens(&self) -> Vec<u32> {
        let mut out = Vec::new();
        for line in &self.lines {
            out.extend_from_slice(&line.tokens);
            out.extend(line.newline);
        }
        out
    }
}

fn softmax_in_place(x: &mut [f32]) {
    let max = x.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0;
    for v in x.iter_mut() {
        *v = (*v - max).exp();
        sum += *v;
    }
    for v in x.iter_mut() {
        *v /= sum;
    }
}

/// The most likely token allowed while `remaining` typed bytes are unaccounted for: one whose
/// rendering starts with them, or is a prefix of them. Returns the token, its probability
/// renormalized over the allowed tokens, and how many of the bytes it covers. Every byte value
/// is a token, so this only fails when the render table is empty.
fn constrained_argmax(
    probs: &[f32],
    render: &[Vec<u8>],
    remaining: &[u8],
) -> Option<(u32, f32, usize)> {
    let mut best: Option<(usize, f32)> = None;
    let mut total = 0.0;
    for (id, (bytes, &p)) in render.iter().zip(probs).enumerate() {
        if bytes.is_empty() || !(bytes.starts_with(remaining) || remaining.starts_with(bytes)) {
            continue;
        }
        total += p;
        if best.is_none_or(|(_, b)| p > b) {
            best = Some((id, p));
        }
    }
    best.map(|(id, p)| (id as u32, p / total, render[id].len().min(remaining.len())))
}

impl Session<'_> {
    /// Greedy completion as a sequence of lines with confidence, stopping at the end token,
    /// at a line end where the end token is likely enough, when the next line would duplicate
    /// the suffix's first line (`suffix_line`, content tokens only) or an earlier line, or at
    /// the limits. The session's context ends with the last kept line's content, so accepting
    /// that text costs no recomputation.
    ///
    /// `partial` is text the user has typed past the end of the context that is not in it: the
    /// incomplete last token, which would encode differently once finished. The context should
    /// end at a pre-token boundary (`tokenizer::pretok::last_piece_start`). Until the generated
    /// text covers the partial, only tokens consistent with it are considered, so the model
    /// picks the whole token the user is typing rather than continuing a fragment it never saw
    /// in training. The completion's text therefore begins with the partial, and its first
    /// line's confidence is over the renormalized choices.
    pub fn complete(
        &mut self,
        tokens: &TokenSet,
        opts: &CompletionOptions,
        suffix_line: &[u32],
        partial: &[u8],
    ) -> Completion {
        let mut lines: Vec<Line> = Vec::new();
        let mut current = Line::default();
        let mut generated = 0usize;
        let mut probs: Vec<f32> = Vec::new();
        let mut remaining = partial;
        // Context length at the end of the last kept line, before its line break was fed.
        let mut kept_len = self.tokens.len();
        loop {
            let Some(logits) = self.last_logits.as_deref() else {
                return Completion {
                    lines,
                    reason: StopReason::EndToken,
                };
            };
            probs.clear();
            probs.extend_from_slice(logits);
            softmax_in_place(&mut probs);
            let p_eom = probs[tokens.eom as usize];
            let (next, p_next) = if remaining.is_empty() {
                let next = argmax(&probs);
                (next, probs[next as usize])
            } else {
                let Some((next, p, covered)) =
                    constrained_argmax(&probs, &tokens.render, remaining)
                else {
                    return Completion {
                        lines,
                        reason: StopReason::EndToken,
                    };
                };
                remaining = &remaining[covered..];
                (next, p)
            };
            if next == tokens.eom || next == tokens.eos {
                if !current.tokens.is_empty() {
                    current.stop_prob = p_next;
                    lines.push(current);
                }
                return Completion {
                    lines,
                    reason: StopReason::EndToken,
                };
            }
            if tokens.newline.contains(&next) {
                current.stop_prob = p_eom;
                if !current.tokens.is_empty() {
                    let rejected = if current.tokens == suffix_line {
                        Some(StopReason::SuffixDuplicate)
                    } else if lines.iter().any(|l| l.tokens == current.tokens)
                        || near_duplicate(&current, lines.last())
                    {
                        Some(StopReason::Repetition)
                    } else if current.confidence < opts.min_line_confidence {
                        Some(StopReason::LowConfidence)
                    } else {
                        None
                    };
                    if let Some(reason) = rejected {
                        // Drop the line and leave the context at the end of the kept text.
                        self.truncate(kept_len);
                        if let Some(last) = lines.last_mut() {
                            last.newline = None;
                        }
                        return Completion { lines, reason };
                    }
                }
                lines.push(current);
                current = Line::default();
                if p_eom >= opts.stop_threshold {
                    return Completion {
                        lines,
                        reason: StopReason::StopThreshold,
                    };
                }
                if lines.len() >= opts.max_lines {
                    return Completion {
                        lines,
                        reason: StopReason::MaxLines,
                    };
                }
                lines.last_mut().unwrap().newline = Some(next);
                kept_len = self.tokens.len();
                self.feed(&[next]);
                continue;
            }
            current.push(next, p_next);
            generated += 1;
            self.feed(&[next]);
            if generated >= opts.max_tokens {
                lines.push(current);
                return Completion {
                    lines,
                    reason: StopReason::MaxTokens,
                };
            }
        }
    }
}

/// Whether `line` is the previous line with at most one token changed, the shape of a
/// degenerate loop such as `let d = 4; let d = 5; ...`.
fn near_duplicate(line: &Line, previous: Option<&Line>) -> bool {
    let Some(prev) = previous else { return false };
    line.tokens.len() >= 3
        && line.tokens.len() == prev.tokens.len()
        && line
            .tokens
            .iter()
            .zip(&prev.tokens)
            .filter(|(a, b)| a != b)
            .count()
            <= 1
}

pub fn argmax(x: &[f32]) -> u32 {
    let mut best = 0;
    for (i, &v) in x.iter().enumerate() {
        if v > x[best] {
            best = i;
        }
    }
    best as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Model {
        Model::random(
            Config {
                vocab_size: 64,
                d_model: 32,
                n_layers: 2,
                n_heads: 4,
                d_ff: 64,
                max_seq_len: 64,
                rope_theta: 10000.0,
            },
            7,
        )
    }

    fn tokens(n: usize, seed: u32) -> Vec<u32> {
        (0..n as u32)
            .map(|i| (i * 7919 + seed * 104729) % 64)
            .collect()
    }

    fn max_diff(a: &[f32], b: &[f32]) -> f32 {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn incremental_matches_whole_sequence() {
        let model = tiny();
        let toks = tokens(40, 1);
        let mut whole = model.new_cache();
        let all = model.forward(&toks, &mut whole, true);
        let vocab = model.config.vocab_size;
        let mut session = Session::new(&model);
        session.feed(&toks[..25]);
        for i in 25..toks.len() {
            session.feed(&toks[i..i + 1]);
            let expected = &all[i * vocab..(i + 1) * vocab];
            assert!(
                max_diff(session.last_logits().unwrap(), expected) < 1e-4,
                "position {i}"
            );
        }
    }

    #[test]
    fn sync_reuses_prefix_and_handles_deletions() {
        let model = tiny();
        let a = tokens(40, 1);
        let mut session = Session::new(&model);
        session.sync(&a);
        let mut b = a[..30].to_vec();
        b.extend(tokens(6, 2));
        session.sync(&b);
        assert_eq!(session.tokens(), &b[..]);
        let mut fresh = model.new_cache();
        let expected = model.forward(&b, &mut fresh, false);
        assert!(max_diff(session.last_logits().unwrap(), &expected) < 1e-4);
        let c = &b[..20];
        session.sync(c);
        let mut fresh = model.new_cache();
        let expected = model.forward(c, &mut fresh, false);
        assert!(max_diff(session.last_logits().unwrap(), &expected) < 1e-4);
    }

    #[test]
    fn overflow_keeps_recent_window() {
        let model = tiny();
        let toks = tokens(100, 3);
        let mut session = Session::new(&model);
        session.feed(&toks);
        assert_eq!(session.tokens(), &toks[36..]);
        let mut fresh = model.new_cache();
        let expected = model.forward(&toks[36..], &mut fresh, false);
        assert!(max_diff(session.last_logits().unwrap(), &expected) < 1e-4);
    }

    #[test]
    fn completion_covers_the_partial() {
        let model = tiny();
        let vocab = model.config.vocab_size;
        // Token 2 is a line break; ids 3.. render to two-letter strings, 0 and 1 to nothing.
        let render: Vec<Vec<u8>> = (0..vocab)
            .map(|id| match id {
                0 | 1 => Vec::new(),
                2 => b"\n".to_vec(),
                _ => vec![b'a' + (id % 26) as u8, b'a' + (id / 26 % 26) as u8],
            })
            .collect();
        let tokens = TokenSet {
            eom: 0,
            eos: 1,
            newline: 2..3,
            render,
        };
        let opts = CompletionOptions {
            max_tokens: 6,
            stop_threshold: 2.0,
            ..CompletionOptions::default()
        };
        for partial in [&b"d"[..], b"dab", b"dadbdc", b"\nab"] {
            let mut session = Session::new(&model);
            session.feed(&[5, 6, 7]);
            let completion = session.complete(&tokens, &opts, &[], partial);
            let text: Vec<u8> = completion
                .tokens()
                .iter()
                .flat_map(|&t| tokens.render[t as usize].clone())
                .collect();
            assert!(
                text.starts_with(partial),
                "{:?} does not start with {:?}",
                String::from_utf8_lossy(&text),
                String::from_utf8_lossy(partial)
            );
            // The line holding the healed tokens has a renormalized confidence.
            let first = completion
                .lines
                .iter()
                .find(|l| !l.tokens.is_empty())
                .unwrap();
            assert!(first.confidence > 0.0 && first.confidence <= 1.0);
        }
    }
}
