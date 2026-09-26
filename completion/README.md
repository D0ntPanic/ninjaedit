# completion

A machine-learning code completion engine for ninjaedit. The training is
performed in MLX with Python, but inference is pure Rust so that the editor
doesn't need to embed a Python toolchain.

Crates:

* `corpus` — builds a filtered, deduplicated Rust source corpus from a local
  Panamax mirror of crates.io or a Debian mirror. Output is sharded
  zstd-compressed JSONL.

* `tokenizer` — byte-level BPE tokenizer trained on the corpus. Line breaks are
  single tokens that carry the indent level of the following line, with the
  indent unit detected per file, so tab, two-space and four-space code all
  tokenize identically and decoding renders the user's preferred style.

* `packer` — tokenizes the corpus and packs it into fixed-length training rows.
  Each split is a flat file of `seq_len` little-endian u16 tokens per row; the
  high bit marks tokens excluded from the loss (padding and the crate/file name
  header). Half of the documents get a fill-in-the-middle transformation, split
  at pre-token boundaries, in the SPM layout the editor's prompts use or, with
  probability `--psm-rate`, in PSM (disabled by default). Inference never feeds
  a partial token (see `infer` below), so training does not spend examples on
  prefixes that end inside one.

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
  at run time with a portable fallback). Inference returns a completion as
  lines, each with a confidence (geometric mean token probability), the least
  likely token's probability, and the end-of-middle probability at its end.
  Inference also leaves the context at the end of the kept text so accepting
  it costs nothing. The context ends at the last pre-token boundary of the
  prefix (`pretok::last_piece_start`), and whatever the user has typed past
  it is passed as a partial token: BPE merges never cross pre-token
  boundaries, so everything before it encodes exactly as it will once the
  token is finished, and generation is constrained to tokens consistent with
  the partial until it is covered. The model therefore chooses the whole
  token being typed (`Option` for `Opti`) instead of continuing a fragment
  it rarely saw in training, and keystrokes inside a token leave the fed
  context, and its cache, untouched.

* `evaluation` — measures a model end to end, through the editor core: the
  project is opened as the editor opens it, files come from its index (so
  ignored files are skipped), and completions are requested by the editor
  model and answered by its `Completer`, with the model under evaluation
  overriding the settings. In each file, sections are deleted and written
  back a word at a time: finishing a line, writing a few new lines, and
  filling in a block body. Where a completion is right, its correct words
  are accepted in one go; where it's wrong, the next word is typed.
  The report gives the next word, partial line, whole line, multi-line
  and character acceptance rates, overall and by kind of section. These
  score the model's whole completions, whatever its confidence; beside
  them, the report gives how often the editor's confidence thresholds
  would offer a completion at all, how often what they offer is right,
  and its average length.

Model shapes live in `shapes/*.json`: each names an architecture and its
planned training run (batch, token budget, learning rate, warmup, intervals).
`train.py --shape 70m` uses one, deriving the step count from the token
budget and defaulting the checkpoint directory to the shape's name; any flag
overrides the file.

A training checkpoint directory holds `model.safetensors` (float32 master
weights), `optimizer.safetensors` (AdamW moments), `config.json`,
`state.json` (step, the training plan, and the tokenizer fingerprint) and a
copy of the tokenizer as `tokenizer.json`.
