# completion

Experimental machine-learning code completion for ninjaedit. Everything here is
pure Rust: training and inference both use `burn`, so the editor itself can
fine-tune a model on a long-lived project without a Python toolchain.

Crates:

* `corpus` — builds a filtered, deduplicated Rust source corpus from a local
  Panamax mirror of crates.io. Output is sharded zstd-compressed JSONL with one
  crate per line, keeping the crate's file layout and dependency list so later
  stages can build repository-aware training examples.

None of these crates are default workspace members, so a plain `cargo build`
does not pay for their dependencies. Build with `cargo build --release -p corpus`.

* `tokenizer` — byte-level BPE tokenizer trained on the corpus. Line breaks are
  single tokens that carry the indent level of the following line, with the
  indent unit detected per file, so tab, two-space and four-space code all
  tokenize identically and decoding renders the user's preferred style.
  `tokenizer train` learns merges from a sample of the training split,
  `tokenizer eval` reports compression on a held-out split, and
  `tokenizer encode` shows how a file is tokenized.

The corpus library defines the train/valid/test split as a hash of the crate
name (98/1/1), so every stage agrees on which crates are held out.

* `packer` — tokenizes the corpus and packs it into fixed-length training rows.
  Each split is a flat file of `seq_len` little-endian u16 tokens per row; the
  high bit marks tokens excluded from the loss (padding and the crate/file name
  header). Half of the documents get a fill-in-the-middle transformation, split
  at character positions so the prefix can end mid-identifier like a real
  cursor, in both the PSM and SPM layouts. Long files are cut at line
  boundaries so every FIM document fits in one row; plain documents may be
  split across rows to fill gaps. `packer show` decodes a row for inspection.
