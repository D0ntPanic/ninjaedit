//! MinHash signatures over token shingles and locality-sensitive hashing to find near-duplicate files.

use twox_hash::XxHash3_64;

pub const HASHES: usize = 64;
pub type Signature = [u32; HASHES];

const SHINGLE: usize = 5;
const MIN_SHINGLES: usize = 32;
const BANDS: usize = 16;
const ROWS: usize = HASHES / BANDS;

/// Odd multipliers and offsets for the hash permutations, generated once from a fixed seed.
fn permutations() -> &'static [(u64, u64); HASHES] {
    static PERMS: std::sync::OnceLock<[(u64, u64); HASHES]> = std::sync::OnceLock::new();
    PERMS.get_or_init(|| {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        };
        std::array::from_fn(|_| (next() | 1, next()))
    })
}

/// Splits source into identifier runs and single punctuation characters, ignoring whitespace.
fn tokens(content: &str) -> impl Iterator<Item = &str> {
    let bytes = content.as_bytes();
    let mut pos = 0;
    std::iter::from_fn(move || {
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= bytes.len() {
            return None;
        }
        let start = pos;
        if bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_' || bytes[pos] >= 0x80 {
            while pos < bytes.len()
                && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_' || bytes[pos] >= 0x80)
            {
                pos += 1;
            }
        } else {
            pos += 1;
        }
        Some(&content[start..pos])
    })
}

/// Counts identifier runs and punctuation characters, a rough stand-in for BPE tokens.
pub fn token_count(content: &str) -> usize {
    tokens(content).count()
}

/// Computes the MinHash signature of a file, or `None` if it is too short to compare reliably.
pub fn signature(content: &str) -> Option<Signature> {
    let token_hashes: Vec<u64> = tokens(content)
        .map(|t| XxHash3_64::oneshot(t.as_bytes()))
        .collect();
    if token_hashes.len() < SHINGLE + MIN_SHINGLES {
        return None;
    }
    let perms = permutations();
    let mut sig = [u32::MAX; HASHES];
    for window in token_hashes.windows(SHINGLE) {
        let mut h = 0u64;
        for &t in window {
            h = (h.rotate_left(17) ^ t).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        }
        for (slot, &(a, b)) in sig.iter_mut().zip(perms) {
            let v = (h.wrapping_mul(a).wrapping_add(b) >> 32) as u32;
            if v < *slot {
                *slot = v;
            }
        }
    }
    Some(sig)
}

fn similarity(a: &Signature, b: &Signature) -> f64 {
    let equal = a.iter().zip(b).filter(|(x, y)| x == y).count();
    equal as f64 / HASHES as f64
}

struct UnionFind {
    parent: Vec<u32>,
}

impl UnionFind {
    fn find(&mut self, mut x: u32) -> u32 {
        while self.parent[x as usize] != x {
            let p = self.parent[x as usize];
            self.parent[x as usize] = self.parent[p as usize];
            x = p;
        }
        x
    }

    /// Unites two sets, keeping the smaller id as the root so the earliest file survives.
    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
        self.parent[hi as usize] = lo;
    }
}

/// Result of near-duplicate clustering.
pub struct Clusters {
    /// `dropped[id]` is true when the file is a near-duplicate of an earlier file.
    pub dropped: Vec<bool>,
    pub candidate_pairs: u64,
    pub confirmed_pairs: u64,
}

/// Finds near-duplicate files. `sigs[id]` is `None` for files too short to compare.
pub fn cluster(sigs: &[Option<Signature>], threshold: f64) -> Clusters {
    let mut buckets: Vec<(u64, u32)> = Vec::with_capacity(sigs.len() * BANDS);
    for (id, sig) in sigs.iter().enumerate() {
        let Some(sig) = sig else { continue };
        for (band, rows) in sig.chunks(ROWS).enumerate() {
            let mut key = [0u8; 4 + ROWS * 4];
            key[..4].copy_from_slice(&(band as u32).to_le_bytes());
            for (i, r) in rows.iter().enumerate() {
                key[4 + i * 4..8 + i * 4].copy_from_slice(&r.to_le_bytes());
            }
            buckets.push((XxHash3_64::oneshot(&key), id as u32));
        }
    }
    buckets.sort_unstable();

    let mut uf = UnionFind {
        parent: (0..sigs.len() as u32).collect(),
    };
    let mut candidate_pairs = 0u64;
    let mut confirmed_pairs = 0u64;
    let mut start = 0;
    while start < buckets.len() {
        let key = buckets[start].0;
        let mut end = start + 1;
        while end < buckets.len() && buckets[end].0 == key {
            end += 1;
        }
        let group = &buckets[start..end];
        // Compare each member to a bounded number of predecessors in the bucket. Union-find
        // makes the clusters transitive, so this finds large groups without quadratic work.
        for (i, &(_, id)) in group.iter().enumerate().skip(1) {
            let lo = i.saturating_sub(8);
            let mut others: Vec<u32> = group[lo..i].iter().map(|&(_, o)| o).collect();
            if lo > 0 {
                others.push(group[0].1);
            }
            for other in others {
                if other == id || uf.find(other) == uf.find(id) {
                    continue;
                }
                candidate_pairs += 1;
                let (Some(a), Some(b)) = (&sigs[id as usize], &sigs[other as usize]) else {
                    continue;
                };
                if similarity(a, b) >= threshold {
                    confirmed_pairs += 1;
                    uf.union(id, other);
                }
            }
        }
        start = end;
    }

    let dropped = (0..sigs.len() as u32).map(|id| uf.find(id) != id).collect();
    Clusters {
        dropped,
        candidate_pairs,
        confirmed_pairs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(seed: u32) -> String {
        let mut s = String::new();
        for i in 0..40 {
            s.push_str(&format!(
                "fn f{}(x: u32) -> u32 {{ x + {} }}\n",
                i + seed,
                i
            ));
        }
        s
    }

    #[test]
    fn detects_near_duplicates() {
        let a = sample(0);
        let mut b = sample(0);
        b.push_str("fn extra() {}\n");
        let c = sample(1000);
        let sigs = vec![signature(&a), signature(&b), signature(&c)];
        assert!(similarity(&sigs[0].unwrap(), &sigs[1].unwrap()) > 0.8);
        assert!(similarity(&sigs[0].unwrap(), &sigs[2].unwrap()) < 0.5);
        let clusters = cluster(&sigs, 0.8);
        assert_eq!(clusters.dropped, vec![false, true, false]);
    }
}
