//! Byte-pair encoding training over pre-token frequency counts.

use rustc_hash::FxHashMap;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

pub type Pair = (u32, u32);

/// Learns `num_merges` merges. `words` are distinct pre-tokens with their frequencies, already
/// mapped to initial symbol ids. Returns the merges in order; merge `i` creates id `first_id + i`.
pub fn train(
    words: Vec<(Vec<u32>, u64)>,
    first_id: u32,
    num_merges: usize,
    mut progress: impl FnMut(usize, Pair, u64),
) -> Vec<Pair> {
    let (mut symbols, counts): (Vec<Vec<u32>>, Vec<u64>) = words.into_iter().unzip();
    let mut pair_counts: FxHashMap<Pair, u64> = FxHashMap::default();
    let mut where_used: FxHashMap<Pair, Vec<u32>> = FxHashMap::default();
    for (w, word) in symbols.iter().enumerate() {
        for pair in word.windows(2) {
            let pair = (pair[0], pair[1]);
            *pair_counts.entry(pair).or_default() += counts[w];
            let list = where_used.entry(pair).or_default();
            if list.last() != Some(&(w as u32)) {
                list.push(w as u32);
            }
        }
    }
    let mut heap: BinaryHeap<(u64, Reverse<Pair>)> =
        pair_counts.iter().map(|(&p, &c)| (c, Reverse(p))).collect();

    let mut merges = Vec::with_capacity(num_merges);
    while merges.len() < num_merges {
        let Some((count, Reverse(pair))) = heap.pop() else {
            break;
        };
        let current = pair_counts.get(&pair).copied().unwrap_or(0);
        if current != count {
            // Stale entry; requeue with the current count if the pair still exists.
            if current > 0 {
                heap.push((current, Reverse(pair)));
            }
            continue;
        }
        if count == 0 {
            break;
        }
        let new_id = first_id + merges.len() as u32;
        merges.push(pair);
        pair_counts.remove(&pair);
        let (a, b) = pair;

        let mut touched: Vec<Pair> = Vec::new();
        let mut users = where_used.remove(&pair).unwrap_or_default();
        users.sort_unstable();
        users.dedup();
        for w in users {
            let wi = w as usize;
            let c = counts[wi];
            let word = &symbols[wi];
            let mut new_word = Vec::with_capacity(word.len());
            let mut i = 0;
            while i < word.len() {
                if i + 1 < word.len() && word[i] == a && word[i + 1] == b {
                    if let Some(&prev) = new_word.last() {
                        decrement(&mut pair_counts, (prev, a), c);
                        *pair_counts.entry((prev, new_id)).or_default() += c;
                        where_used.entry((prev, new_id)).or_default().push(w);
                        touched.push((prev, new_id));
                    }
                    if i + 2 < word.len() {
                        let next = word[i + 2];
                        decrement(&mut pair_counts, (b, next), c);
                        *pair_counts.entry((new_id, next)).or_default() += c;
                        where_used.entry((new_id, next)).or_default().push(w);
                        touched.push((new_id, next));
                    }
                    new_word.push(new_id);
                    i += 2;
                } else {
                    new_word.push(word[i]);
                    i += 1;
                }
            }
            symbols[wi] = new_word;
        }
        touched.sort_unstable();
        touched.dedup();
        for p in touched {
            if let Some(&c) = pair_counts.get(&p) {
                heap.push((c, Reverse(p)));
            }
        }
        progress(merges.len(), pair, count);
    }
    merges
}

fn decrement(counts: &mut FxHashMap<Pair, u64>, pair: Pair, by: u64) {
    if let Some(c) = counts.get_mut(&pair) {
        *c = c.saturating_sub(by);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_most_frequent_pair_first() {
        // "abab" x5, "abc" x3: (a,b) is the most frequent pair with count 13.
        let words = vec![(vec![0, 1, 0, 1], 5), (vec![0, 1, 2], 3)];
        let merges = train(words, 10, 3, |_, _, _| {});
        assert_eq!(merges[0], (0, 1));
        // After merging (a,b)=10: "10 10" x5 -> pair (10,10) count 5; "10 2" x3 -> (10,2) count 3.
        assert_eq!(merges[1], (10, 10));
        assert_eq!(merges[2], (10, 2));
    }
}
