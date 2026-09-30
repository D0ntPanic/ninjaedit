"""Exports a training checkpoint's float32 master weights in a shipping precision.

`--dtype q8` quantizes every matrix (the embedding and the projections) to 8 bits: each row is
split into groups of `--group-size` values sharing a float16 scale, max |w| / 127, and a
weight is stored as round(w / scale), a signed byte. A matrix `name.weight` is then int8 with
its scales beside it as `name.scales`, `[rows, cols / group]`; the norms stay float16.
"""

import argparse
import json
import shutil
from pathlib import Path

import mlx.core as mx
from mlx.utils import tree_flatten

from model import load_checkpoint, save_checkpoint


def quantize_q8(w: mx.array, group: int) -> tuple[mx.array, mx.array]:
    """The int8 values and float16 scales of a matrix, quantized symmetrically per group."""
    rows, cols = w.shape
    groups = w.astype(mx.float32).reshape(rows, cols // group, group)
    scales = (mx.abs(groups).max(axis=-1) / 127).astype(mx.float16)
    # Divide by the scale as stored, so the rounding is against what inference multiplies by.
    s = scales.astype(mx.float32)
    s = mx.where(s == 0, 1, s)
    q = mx.clip(mx.round(groups / s[..., None]), -127, 127).astype(mx.int8)
    return q.reshape(rows, cols), scales


def save_q8(model, out: Path, step: int, group: int) -> None:
    out.mkdir(parents=True, exist_ok=True)
    tensors = {}
    for name, value in tree_flatten(model.parameters()):
        if value.ndim != 2:
            tensors[name] = value.astype(mx.float16)
            continue
        if not name.endswith(".weight") or value.shape[1] % group:
            raise SystemExit(f"{name} {value.shape} can't be quantized in groups of {group}")
        q, scales = quantize_q8(value, group)
        tensors[name] = q
        tensors[name.removesuffix(".weight") + ".scales"] = scales
    mx.save_safetensors(str(out / "model.safetensors"), tensors)
    model.config.save(out / "config.json")
    (out / "state.json").write_text(json.dumps({"step": step}))


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--checkpoint", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--dtype", choices=["float32", "bfloat16", "float16", "q8"], default="float16")
    # Inference loads vectors of 16 weights, so a group must be a multiple of 16.
    p.add_argument("--group-size", type=int, default=32, help="values per scale for q8")
    args = p.parse_args()
    if args.group_size <= 0 or args.group_size % 16:
        p.error("--group-size must be a positive multiple of 16")
    src = Path(args.checkpoint).expanduser()
    out = Path(args.out).expanduser()
    model, step = load_checkpoint(src)
    if args.dtype == "q8":
        save_q8(model, out, step, args.group_size)
    else:
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
