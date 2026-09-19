"""Trains the completion model with MLX."""

import argparse
import json
import math
import shutil
import time
from pathlib import Path

import mlx.core as mx
import mlx.nn as nn
import mlx.optimizers as optim
from mlx.utils import tree_flatten, tree_map, tree_unflatten

from data import Dataset, SplitMix, read_meta
from model import Model, ModelConfig, load_checkpoint, loss_fn, save_checkpoint
from shapes import load_shape, model_config, param_count, planned_steps

# Built-in defaults for runs without a shape file. A shape file's `train` section replaces
# these, and an explicit flag overrides both.
DEFAULTS = {
    "batch": 8,
    "lr": 6e-4,
    "warmup": 200,
    "weight_decay": 0.1,
    "eval_every": 200,
    "eval_batches": 16,
    "save_every": 500,
}


def parse_args():
    p = argparse.ArgumentParser(description="Train the code completion model with MLX")
    p.add_argument("--data", default="~/corpus/packed/s2048-16k")
    p.add_argument("--shape", help="shape file or name (see ../shapes); model flags below override it")
    p.add_argument("--out", help="checkpoint directory (default: ~/corpus/checkpoints/<shape name>)")
    p.add_argument("--d-model", type=int)
    p.add_argument("--layers", type=int)
    p.add_argument("--heads", type=int)
    p.add_argument("--d-ff", type=int)
    p.add_argument("--batch", type=int)
    p.add_argument("--steps", type=int, help="default: the shape's token budget at this batch size")
    p.add_argument("--lr", type=float)
    p.add_argument("--warmup", type=int)
    p.add_argument("--weight-decay", type=float)
    p.add_argument("--log-every", type=int, default=10)
    p.add_argument("--eval-every", type=int)
    p.add_argument("--eval-batches", type=int)
    p.add_argument("--save-every", type=int)
    p.add_argument("--resume", action="store_true", help="continue from the checkpoint in --out")
    p.add_argument("--seed", type=int, default=1)
    p.add_argument("--dtype", choices=["float32", "bfloat16"], default="bfloat16", help="compute dtype; parameters stay float32")
    p.add_argument("--no-compile", action="store_true", help="run the training step eagerly")
    args = p.parse_args()
    resolve_plan(args)
    return args


PLAN_KEYS = ("batch", "steps", "lr", "warmup", "weight_decay", "eval_every", "eval_batches", "save_every")


def resolve_plan(args):
    """Fills unset flags from the checkpoint being resumed, else the shape file's plan, then
    the built-in defaults. A resumed run must keep its schedule, so the plan is stored with
    the checkpoint and wins over a shape file."""
    plan = {}
    args.shape_data = None
    if args.shape:
        args.shape_data = load_shape(args.shape)
        plan = dict(args.shape_data["train"])
        if args.out is None:
            args.out = f"~/corpus/checkpoints/{args.shape_data['name']}"
    elif args.out is None:
        raise SystemExit("--out is required without --shape")
    if args.resume:
        state = json.loads((Path(args.out).expanduser() / "state.json").read_text())
        plan.update(state.get("plan", {}))
    for key in PLAN_KEYS:
        if getattr(args, key) is None:
            setattr(args, key, plan.get(key, DEFAULTS.get(key)))
    if not args.resume and args.shape_data is None and None in (args.d_model, args.layers, args.heads, args.d_ff):
        raise SystemExit("give --shape or all of --d-model, --layers, --heads, --d-ff")


def save_training_state(optimizer, out: Path, step: int, args, meta: dict) -> None:
    """Optimizer moments, the plan, and the tokenizer, next to the model checkpoint. The
    optimizer state lets a resumed run continue exactly instead of restarting AdamW from zero
    at full learning rate; the tokenizer travels with the model because the weights are only
    meaningful with the vocabulary they were trained on."""
    mx.save_safetensors(str(out / "optimizer.safetensors"), dict(tree_flatten(optimizer.state)))
    tokenizer = Path(meta["tokenizer"]).expanduser()
    if tokenizer.exists() and not (out / "tokenizer.json").exists():
        shutil.copy(tokenizer, out / "tokenizer.json")
    plan = {key: getattr(args, key) for key in PLAN_KEYS}
    state = {"step": step, "plan": plan}
    if "tokenizer_fingerprint" in meta:
        state["tokenizer_fingerprint"] = meta["tokenizer_fingerprint"]
    (out / "state.json").write_text(json.dumps(state, indent=2))


def load_training_state(optimizer, out: Path) -> bool:
    path = out / "optimizer.safetensors"
    if not path.exists():
        return False
    optimizer.state = tree_unflatten(list(mx.load(str(path)).items()))
    return True


def batch_rows(seed: int, step: int, batch: int, rows: int) -> list[int]:
    """Rows for one step, from a generator seeded by the step so resumed and uninterrupted
    runs draw identical batches."""
    rng = SplitMix(seed * 1_000_003 + step)
    return [rng.below(rows) for _ in range(batch)]


def learning_rate_schedule(args):
    """Linear warmup then cosine decay to a tenth of the peak."""
    warmup = optim.linear_schedule(0.0, args.lr, args.warmup)
    decay = optim.cosine_decay(args.lr, max(args.steps - args.warmup, 1), end=0.1 * args.lr)
    return optim.join_schedules([warmup, decay], [args.warmup])


def evaluate(model: Model, data: Dataset, batch: int, batches: int) -> float:
    rng = SplitMix(12345)
    total = 0.0
    for _ in range(batches):
        rows = [rng.below(len(data)) for _ in range(batch)]
        inputs, targets, mask = (mx.array(a) for a in data.batch(rows))
        total += loss_fn(model, inputs, targets, mask).item()
    return total / batches


def format_duration(secs: float) -> str:
    s = int(secs)
    return f"{s // 3600}h{(s % 3600) // 60:02d}m" if s >= 3600 else f"{s // 60}m{s % 60:02d}s"


def main():
    args = parse_args()
    data_dir = Path(args.data).expanduser()
    out = Path(args.out).expanduser()
    meta = read_meta(data_dir)
    train = Dataset(data_dir / "train.bin", meta["seq_len"])
    valid = Dataset(data_dir / "valid.bin", meta["seq_len"])
    print(f"train {len(train)} rows, valid {len(valid)} rows, seq_len {meta['seq_len']}, vocab {meta['vocab_size']}")
    if args.steps is None:
        if args.shape_data is None:
            raise SystemExit("--steps is required without a shape file")
        args.steps = planned_steps(args.shape_data, args.batch, meta["seq_len"])
        print(f"shape {args.shape_data['name']}: {args.shape_data['train']['tokens'] / 1e9:.2f}B tokens planned -> {args.steps} steps of {args.batch} x {meta['seq_len'] - 1}")

    if args.resume:
        model, start_step = load_checkpoint(out)
        config = model.config
    else:
        if args.shape_data is not None:
            config = model_config(args.shape_data, meta["vocab_size"], meta["seq_len"])
            for key, attr in (("d_model", "d_model"), ("n_layers", "layers"), ("n_heads", "heads"), ("d_ff", "d_ff")):
                if getattr(args, attr) is not None:
                    setattr(config, key, getattr(args, attr))
        else:
            config = ModelConfig(
                vocab_size=meta["vocab_size"],
                d_model=args.d_model,
                n_layers=args.layers,
                n_heads=args.heads,
                d_ff=args.d_ff,
                max_seq_len=meta["seq_len"],
            )
        mx.random.seed(args.seed)
        model = Model(config)
        model.init_weights()
        start_step = 0
    model.compute_dtype = getattr(mx, args.dtype)
    mx.eval(model.parameters())
    params = model.num_params()
    print(
        f"model: {config.n_layers} layers, d_model {config.d_model}, {config.n_heads} heads, d_ff {config.d_ff}: "
        f"{params / 1e6:.1f}M parameters ({(params - config.vocab_size * config.d_model) / 1e6:.1f}M without embeddings), {args.dtype} compute"
    )

    optimizer = optim.AdamW(
        learning_rate=learning_rate_schedule(args),
        betas=[0.9, 0.999],
        eps=1e-5,
        weight_decay=args.weight_decay,
        bias_correction=True,
    )
    optimizer.init(model.trainable_parameters())
    optimizer.state["step"] = mx.array(start_step)
    if args.resume:
        if load_training_state(optimizer, out):
            print(f"resumed at step {start_step} with optimizer state")
        else:
            print(f"resumed at step {start_step} WITHOUT optimizer state (old checkpoint); expect a loss bump")
    step_fn = nn.value_and_grad(model, loss_fn)

    def train_step(inputs, targets, mask):
        loss, grads = step_fn(model, inputs, targets, mask)
        grads, _ = optim.clip_grad_norm(grads, 1.0)
        optimizer.update(model, grads)
        return loss

    if not args.no_compile:
        state = [model.state, optimizer.state]
        train_step = mx.compile(train_step, inputs=state, outputs=state)
    tokens_per_step = args.batch * (meta["seq_len"] - 1)
    started = time.time()
    window_start = time.time()
    window_loss = 0.0
    window_steps = 0

    for step in range(start_step, args.steps):
        rows = batch_rows(args.seed, step, args.batch, len(train))
        inputs, targets, mask = (mx.array(a) for a in train.batch(rows))
        loss = train_step(inputs, targets, mask)
        mx.eval(model.parameters(), optimizer.state, loss)
        window_loss += loss.item()
        window_steps += 1

        done = step + 1
        if done % args.log_every == 0 or done == args.steps:
            elapsed = time.time() - window_start
            print(
                f"step {done:>6}  loss {window_loss / window_steps:.4f}  lr {optimizer.learning_rate.item():.2e}  "
                f"{tokens_per_step * window_steps / elapsed:.0f} tok/s  {elapsed / window_steps:.2f} s/step  "
                f"elapsed {format_duration(time.time() - started)}",
                flush=True,
            )
            window_start = time.time()
            window_loss = 0.0
            window_steps = 0
        if done % args.eval_every == 0 or done == args.steps:
            val = evaluate(model, valid, args.batch, args.eval_batches)
            print(f"step {done:>6}  valid loss {val:.4f}  perplexity {math.exp(val):.2f}", flush=True)
        if done % args.save_every == 0 or done == args.steps:
            save_checkpoint(model, out, done)
            save_training_state(optimizer, out, done, args, meta)
            print(f"step {done:>6}  saved checkpoint to {out}", flush=True)


if __name__ == "__main__":
    main()
