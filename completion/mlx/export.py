"""Exports a training checkpoint's float32 master weights in a shipping precision."""

import argparse
import json
import shutil
from pathlib import Path

import mlx.core as mx

from model import load_checkpoint, save_checkpoint


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--checkpoint", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--dtype", choices=["float32", "bfloat16", "float16"], default="float16")
    args = p.parse_args()
    src = Path(args.checkpoint).expanduser()
    out = Path(args.out).expanduser()
    model, step = load_checkpoint(src)
    save_checkpoint(model, out, step, dtype=getattr(mx, args.dtype))
    # The config, languages included, comes with the weights; the tokenizer and its fingerprint
    # belong with them too.
    if (src / "tokenizer.json").exists():
        shutil.copy(src / "tokenizer.json", out / "tokenizer.json")
    state = json.loads((src / "state.json").read_text())
    if "tokenizer_fingerprint" in state:
        (out / "state.json").write_text(json.dumps({"step": step, "tokenizer_fingerprint": state["tokenizer_fingerprint"]}))


if __name__ == "__main__":
    main()
