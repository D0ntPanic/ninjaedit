"""A decoder-only transformer in the Llama shape: RMSNorm, rotary position embeddings,
SwiGLU feed-forward, no biases, and tied input/output embeddings."""

import json
import math
from dataclasses import asdict, dataclass, field
from pathlib import Path

import mlx.core as mx
import mlx.nn as nn
from mlx.utils import tree_flatten, tree_unflatten

RMS_EPS = 1e-5


@dataclass
class ModelConfig:
    vocab_size: int
    d_model: int = 384
    n_layers: int = 6
    n_heads: int = 6
    d_ff: int = 1024
    max_seq_len: int = 2048
    rope_theta: float = 10000.0
    # The languages the model is trained on, from the packed data's `meta.json`. Every model
    # lists at least one: the editor picks a model for a file by them. With several, training
    # documents open with `<lang>` and the language, and so must prompts.
    languages: list[str] = field(default_factory=list)

    def save(self, path: Path) -> None:
        if not self.languages:
            raise ValueError("a model config must list the languages it is trained on")
        path.write_text(json.dumps(asdict(self), indent=2))

    @staticmethod
    def load(path: Path) -> "ModelConfig":
        config = ModelConfig(**json.loads(path.read_text()))
        if not config.languages:
            raise SystemExit(f"{path} lists no languages; add the ones the model was trained on, as \"languages\": [\"rust\"]")
        return config

    @property
    def head_dim(self) -> int:
        return self.d_model // self.n_heads


# Mixed precision: parameters and optimizer state stay float32, and every layer casts its
# weights to `dtype` on the fly, so activations and matmuls run in bf16 while updates keep
# full precision. Pure bf16 parameters lose small updates to rounding and train worse.


def linear(x: mx.array, layer: nn.Linear, dtype) -> mx.array:
    return x @ layer.weight.astype(dtype).T


def rms_norm(x: mx.array, layer: nn.RMSNorm, dtype) -> mx.array:
    return mx.fast.rms_norm(x, layer.weight.astype(dtype), RMS_EPS)


class Attention(nn.Module):
    def __init__(self, config: ModelConfig):
        super().__init__()
        d = config.d_model
        self.wq = nn.Linear(d, d, bias=False)
        self.wk = nn.Linear(d, d, bias=False)
        self.wv = nn.Linear(d, d, bias=False)
        self.wo = nn.Linear(d, d, bias=False)
        self.n_heads = config.n_heads
        self.head_dim = config.head_dim
        self.rope_theta = config.rope_theta

    def __call__(self, x: mx.array, dtype) -> mx.array:
        b, s, d = x.shape
        heads = lambda t: t.reshape(b, s, self.n_heads, self.head_dim).transpose(0, 2, 1, 3)
        rope = lambda t: mx.fast.rope(t, self.head_dim, traditional=True, base=self.rope_theta, scale=1.0, offset=0)
        q = rope(heads(linear(x, self.wq, dtype)))
        k = rope(heads(linear(x, self.wk, dtype)))
        v = heads(linear(x, self.wv, dtype))
        out = mx.fast.scaled_dot_product_attention(q, k, v, scale=1.0 / math.sqrt(self.head_dim), mask="causal")
        return linear(out.transpose(0, 2, 1, 3).reshape(b, s, d), self.wo, dtype)


class FeedForward(nn.Module):
    def __init__(self, config: ModelConfig):
        super().__init__()
        self.gate = nn.Linear(config.d_model, config.d_ff, bias=False)
        self.up = nn.Linear(config.d_model, config.d_ff, bias=False)
        self.down = nn.Linear(config.d_ff, config.d_model, bias=False)

    def __call__(self, x: mx.array, dtype) -> mx.array:
        return linear(nn.silu(linear(x, self.gate, dtype)) * linear(x, self.up, dtype), self.down, dtype)


class Block(nn.Module):
    def __init__(self, config: ModelConfig):
        super().__init__()
        self.attn_norm = nn.RMSNorm(config.d_model, eps=RMS_EPS)
        self.attn = Attention(config)
        self.ffn_norm = nn.RMSNorm(config.d_model, eps=RMS_EPS)
        self.ffn = FeedForward(config)

    def __call__(self, x: mx.array, dtype) -> mx.array:
        h = x + self.attn(rms_norm(x, self.attn_norm, dtype), dtype)
        return h + self.ffn(rms_norm(h, self.ffn_norm, dtype), dtype)


class Model(nn.Module):
    def __init__(self, config: ModelConfig, compute_dtype=mx.float32):
        super().__init__()
        self.config = config
        self.compute_dtype = compute_dtype
        self.embedding = nn.Embedding(config.vocab_size, config.d_model)
        self.layers = [Block(config) for _ in range(config.n_layers)]
        self.norm = nn.RMSNorm(config.d_model, eps=RMS_EPS)

    def __call__(self, tokens: mx.array) -> mx.array:
        dtype = self.compute_dtype
        weight = self.embedding.weight.astype(dtype)
        x = weight[tokens]
        for layer in self.layers:
            x = layer(x, dtype)
        # Logits in float32 so the loss and its softmax are exact.
        return (rms_norm(x, self.norm, dtype) @ weight.T).astype(mx.float32)

    def init_weights(self) -> None:
        """Normal(0, 0.02) everywhere, with residual projections scaled down by depth."""
        std = 0.02
        residual_std = 0.02 / math.sqrt(2.0 * self.config.n_layers)
        normal = nn.init.normal(mean=0.0, std=std)
        residual = nn.init.normal(mean=0.0, std=residual_std)
        updates = {}
        for name, value in tree_flatten(self.parameters()):
            if name.endswith(".attn.wo.weight") or name.endswith(".ffn.down.weight"):
                updates[name] = residual(value)
            elif name.endswith(".weight") and "norm" not in name:
                updates[name] = normal(value)
        self.update(tree_unflatten(list(updates.items())))

    def num_params(self) -> int:
        return sum(v.size for _, v in tree_flatten(self.parameters()))


def loss_fn(model: Model, inputs: mx.array, targets: mx.array, mask: mx.array) -> mx.array:
    logits = model(inputs)
    ce = nn.losses.cross_entropy(logits, targets, reduction="none")
    return (ce * mask).sum() / mask.sum()


def is_linear(name: str) -> bool:
    return name.endswith(".weight") and "norm" not in name and name != "embedding.weight"


def save_checkpoint(model: Model, out: Path, step: int, dtype=None) -> None:
    """Writes `model.safetensors`, plus config and state. Training checkpoints
    keep the float32 master weights; pass `dtype=mx.bfloat16` to export weights
    for shipping."""
    out.mkdir(parents=True, exist_ok=True)
    tensors = {}
    for name, value in tree_flatten(model.parameters()):
        if dtype is not None:
            value = value.astype(dtype)
        tensors[name] = value
    mx.save_safetensors(str(out / "model.safetensors"), tensors)
    model.config.save(out / "config.json")
    (out / "state.json").write_text(json.dumps({"step": step}))


def load_checkpoint(path: Path) -> tuple[Model, int]:
    config = ModelConfig.load(path / "config.json")
    model = Model(config)
    tensors = mx.load(str(path / "model.safetensors"))
    updates = []
    for name, value in tensors.items():
        updates.append((name, value))
    model.update(tree_unflatten(updates))
    step = json.loads((path / "state.json").read_text())["step"]
    return model, step
