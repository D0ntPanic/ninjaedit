//! A CPU inference engine written for this one model rather than for tensors in general.
//!
//! Decode is memory-bound: every generated token reads every weight once. So the weights stay
//! in f16 in memory, are converted to f32 inside the dot-product kernel, and are laid out so
//! each output element is one contiguous dot product. A checkpoint exported as `q8` keeps
//! them in 8 bits with a scale per group of a row, half the bytes; converting each of those to
//! f32 would cost decode more than the bandwidth saves, so decode quantizes its input vector in
//! the same groups and takes integer dot products, while prefill, which is compute-bound,
//! converts blocks of rows to f32 as it does f16 ones. The key/value cache is f16 as well,
//! since at full context attention reads nearly as many bytes as the weights. Everything else
//! (norms, rotary embeddings, attention over the flat cache) is plain loops over f32 buffers.

pub mod kernels;
pub mod model;

pub use model::{
    Completion, CompletionOptions, Config, KvCache, Line, Model, Session, StopReason, TokenSet,
};
