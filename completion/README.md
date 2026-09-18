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
