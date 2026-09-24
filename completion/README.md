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
  at pre-token boundaries in both the PSM and SPM layouts. Inference never
  feeds a partial token (see `infer` below), so training does not spend
  examples on prefixes that end inside one, and the three parts of a
  document tokenize exactly as the whole does. Long files are cut at line
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
  f16 safetensors directly, keeps weights in f16 and converts inside the
  dot-product kernel (NEON on arm64; AVX2, FMA and F16C on x86_64, detected
  at run time with a portable fallback), stores each projection transposed so
  every output is a contiguous dot product, fuses the q/k/v and gate/up
  projections, batches prefill, and runs attention over an f32 cache kept
  contiguous per head. Prefill converts blocks of weight rows to f32 once and
  runs a register-tiled kernel over them (on arm64, rows packed eight wide
  and accumulated as outer products). It keeps two thread
  pools: a few threads for decode, which is memory-bound and saturates early
  (four on Apple silicon, eight on x86_64), and one per physical core for
  prefill, which is compute-bound and gains nothing from SMT siblings. On an
  M5 Max the 70M model decodes at 1.1 ms/token (140 GB/s of weights) and
  prefills at about 8,000 tok/s. On a Threadripper 3970X it decodes at
  4.5 ms/token (36 GB/s) and prefills at about 4,400 tok/s. `Session::complete` returns a completion as lines, each with
  a confidence (geometric mean token probability), the least likely token's
  probability, and the end-of-middle probability at its end; it stops at the
  end token, at a line end where the end token clears a threshold, when the
  next line would duplicate the suffix's first line or a previous line, or
  at the limits, and leaves the context at the end of the kept text so
  accepting it costs nothing. The context ends at the last pre-token boundary
  of the prefix (`pretok::last_piece_start`), and whatever the user has typed
  past it is passed as a partial token: BPE merges never cross pre-token
  boundaries, so everything before it encodes exactly as it will once the
  token is finished, and generation is constrained to tokens consistent with
  the partial until it is covered. The model therefore chooses the whole
  token being typed (`Option` for `Opti`) instead of continuing a fragment
  it rarely saw in training, and keystrokes inside a token leave the fed
  context, and its cache, untouched. `infer bench` measures throughput,
  `infer sample` completes a prefix/suffix pair; `--no-heal` feeds the prefix
  as typed for comparison.

* `evaluation` — measures a model end to end, through the editor core: the
  project is opened as the editor opens it, files come from its index (so
  ignored files are skipped), and completions are requested by the editor
  model and answered by its `Completer`, with the model under evaluation
  overriding the settings. In each file, sections are deleted and written
  back a word at a time: finishing a line, writing a few new lines, and
  filling in a block body. Where a completion is right, its correct words
  are accepted in one go; where it's wrong, the next word is typed.
  The report gives the next word, partial line, whole line, multi-line
  and character acceptance rates, overall and by kind of section.
  Everything random is seeded, so a run can be repeated exactly.
  `cargo run --release -p evaluation -- core/src --model ~/models/rust-70m
  --language rust` evaluates on the files under `core/src`. Sections are
  sampled in proportion to file size, `--hole-rate` per 100 lines of code
  (2 by default); it and `--max-files` bound the run. `--json` saves the
  results for comparison, and `--trace` prints every completion next to
  what was expected.

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
