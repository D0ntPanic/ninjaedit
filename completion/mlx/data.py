"""Reads the packed rows written by the `packer` crate."""

import json
from pathlib import Path

import numpy as np

MASK_BIT = 0x8000


class Dataset:
    def __init__(self, path: Path, seq_len: int):
        raw = np.memmap(path, dtype=np.uint16, mode="r")
        if raw.size % seq_len != 0:
            raise ValueError(f"{path} is not a whole number of rows")
        self.rows = raw.reshape(-1, seq_len)
        self.seq_len = seq_len

    def __len__(self) -> int:
        return self.rows.shape[0]

    def batch(self, indices) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
        """Inputs, next-token targets and loss mask, each `[batch, seq_len - 1]`."""
        rows = self.rows[np.asarray(indices)]
        inputs = (rows[:, :-1] & np.uint16(0x7FFF)).astype(np.int32)
        targets_raw = rows[:, 1:]
        targets = (targets_raw & np.uint16(0x7FFF)).astype(np.int32)
        mask = ((targets_raw & MASK_BIT) == 0).astype(np.float32)
        return inputs, targets, mask


def read_meta(directory: Path) -> dict:
    return json.loads((directory / "meta.json").read_text())


class SplitMix:
    MASK = (1 << 64) - 1

    def __init__(self, seed: int):
        self.state = seed & self.MASK

    def next_u64(self) -> int:
        self.state = (self.state + 0x9E3779B97F4A7C15) & self.MASK
        z = self.state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & self.MASK
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & self.MASK
        return z ^ (z >> 31)

    def below(self, n: int) -> int:
        return self.next_u64() % n
