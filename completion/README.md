# completion

A machine-learning code completion engine for ninjaedit. The training is
performed in MLX with Python, but inference is pure Rust so that the editor
doesn't need to embed a Python toolchain.

Crates:

* `corpus` — builds a filtered, deduplicated Rust source corpus from a local
  Panamax mirror of crates.io. Output is sharded zstd-compressed JSONL with one
  crate per line, keeping the crate's file layout and dependency list so later
  stages can build repository-aware training examples.

* `tokenizer` — byte-level BPE tokenizer trained on the corpus. Line breaks are
  single tokens that carry the indent level of the following line, with the
  indent unit detected per file, so tab, two-space and four-space code all
  tokenize identically and decoding renders the user's preferred style.
  `tokenizer train` learns merges from a sample of the training split,
  `tokenizer eval` reports compression on a held-out split, and
  `tokenizer encode` shows how a file is tokenized.

* `packer` — tokenizes the corpus and packs it into fixed-length training rows.
  Each split is a flat file of `seq_len` little-endian u16 tokens per row; the
  high bit marks tokens excluded from the loss (padding and the crate/file name
  header). Half of the documents get a fill-in-the-middle transformation, split
  at character positions so the prefix can end mid-identifier like a real
  cursor, in both the PSM and SPM layouts. Long files are cut at line
  boundaries so every FIM document fits in one row; plain documents may be
  split across rows to fill gaps. `packer show` decodes a row for inspection.

* `mlx` — the  trainer for the model. The model is a decoder-only transformer
  in the Llama shape (RMSNorm, rotary position embeddings, SwiGLU, no biases,
  tied embeddings). `train.py` trains and checkpoints as safetensors.
  The Python environment is driven through mise tasks: `mise.toml` pins
  Python 3.13 and uv, `pyproject.toml` and `uv.lock` declare the dependencies,
  and every task depends on `setup`, which runs `uv sync`. On a fresh machine,
  `mise install` once and then `mise run train -- --shape 70m`, or
  `mise run export||shapes -- <args>`; the venv and packages appear on first
  use.

* `infer` — the dedicated CPU engine, with minimal dependencies. It loads the
  f16 safetensors directly, keeps weights in f16 and converts inside a NEON
  dot-product kernel, stores each projection transposed so every output is a
  contiguous dot product, fuses the q/k/v and gate/up projections, batches
  prefill, and runs attention over a flat f32 cache. It keeps two thread
  pools: four threads for decode, which is memory-bound and saturates early,
  and every core for prefill, which is compute-bound. Decode on the 34M model
  is 0.65 ms/token (131 GB/s of weights), and prefill runs at about
  5,400 tok/s. `Session::complete` returns a completion as lines, each with
  a confidence (geometric mean token probability), the least likely token's
  probability, and the end-of-middle probability at its end; it stops at the
  end token, at a line end where the end token clears a threshold, when the
  next line would duplicate the suffix's first line or a previous line, or
  at the limits, and leaves the context at the end of the kept text so
  accepting it costs nothing. `infer bench` measures throughput,
  `infer sample` completes a prefix/suffix pair.

Model shapes live in `shapes/*.json`: each names an architecture and its
planned training run (batch, token budget, learning rate, warmup, intervals).
`train.py --shape 70m` uses one, deriving the step count from the token
budget and defaulting the checkpoint directory to the shape's name; any flag
overrides the file. `bench.py` and `init_random.py` take `--shape` too, and
`python shapes.py` prints a table of every shape with its parameter count.

A training checkpoint directory holds `model.safetensors` (float32 master
weights), `optimizer.safetensors` (AdamW moments), `config.json`,
`state.json` (step, the training plan, and the tokenizer fingerprint) and a
copy of the tokenizer as `tokenizer.json`. A model is only meaningful with
the vocabulary it was trained on, so the packer records the tokenizer's
fingerprint in `meta.json`, the trainer bundles the tokenizer with every
checkpoint, `export.py` carries it along, and the sampling commands load it
from the checkpoint and refuse a tokenizer whose fingerprint does not match.
`train.py --out DIR --resume` continues from it with the same schedule and
optimizer state; batches are seeded per step, so a resumed run sees the same
data as an uninterrupted one.

Precision: training uses mixed precision (float32 master weights and optimizer
state, bf16 compute), because pure bf16 parameters lose small updates and
train measurably worse. Checkpoints keep the float32 weights; `mlx/export.py`
writes an f16 copy for shipping.
