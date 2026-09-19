"""Model shapes: JSON files describing an architecture and its planned training run.

A shape file has a `model` section (`d_model`, `n_layers`, `n_heads`, `d_ff`, optional
`rope_theta`) and a `train` section with the plan: `batch`, a `tokens` budget from which the
step count is derived, `lr`, `warmup`, `weight_decay`, and the evaluation and checkpoint
intervals. Command-line flags override any of these.
"""

import json
import math
import sys
from pathlib import Path

from model import ModelConfig

SHAPES_DIR = Path(__file__).resolve().parent.parent / "shapes"


def load_shape(path: str) -> dict:
    """Loads a shape by path, or by name from the shapes directory (`70m` -> `shapes/70m.json`)."""
    p = Path(path).expanduser()
    if not p.exists() and not p.suffix:
        p = SHAPES_DIR / f"{path}.json"
    return json.loads(p.read_text())


def model_config(shape: dict, vocab_size: int, max_seq_len: int) -> ModelConfig:
    m = shape["model"]
    return ModelConfig(
        vocab_size=vocab_size,
        d_model=m["d_model"],
        n_layers=m["n_layers"],
        n_heads=m["n_heads"],
        d_ff=m["d_ff"],
        max_seq_len=max_seq_len,
        rope_theta=m.get("rope_theta", 10000.0),
    )


def param_count(shape: dict, vocab_size: int) -> tuple[int, int]:
    """(total, non-embedding) parameter counts."""
    m = shape["model"]
    d, ff = m["d_model"], m["d_ff"]
    body = m["n_layers"] * (4 * d * d + 3 * d * ff)
    return vocab_size * d + body, body


def planned_steps(shape: dict, batch: int, seq_len: int) -> int:
    """Steps needed to see the plan's token budget at this batch and row length."""
    return math.ceil(shape["train"]["tokens"] / (batch * (seq_len - 1)))


def main():
    """Prints a table of the shapes given on the command line, or all of them."""
    paths = sys.argv[1:] or sorted(str(p) for p in SHAPES_DIR.glob("*.json"))
    print(f"{'shape':<8}{'d_model':>8}{'layers':>7}{'heads':>6}{'d_ff':>6}{'params':>9}{'non-emb':>9}{'batch':>6}{'tokens':>8}{'steps@2048':>11}{'lr':>8}")
    for path in paths:
        s = load_shape(path)
        total, body = param_count(s, 16384)
        t = s["train"]
        print(
            f"{s['name']:<8}{s['model']['d_model']:>8}{s['model']['n_layers']:>7}{s['model']['n_heads']:>6}{s['model']['d_ff']:>6}"
            f"{total / 1e6:>8.1f}M{body / 1e6:>8.1f}M{t['batch']:>6}{t['tokens'] / 1e9:>7.1f}B{planned_steps(s, t['batch'], 2048):>11}{t['lr']:>8.0e}"
        )


if __name__ == "__main__":
    main()
