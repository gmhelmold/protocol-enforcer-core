// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! RFC 6962 §2.1 / §2.1.1 Merkle Tree Hash and inclusion proofs over an ordered
//! sequence of byte leaves. Pure functions, no I/O.
//!
//! The domain-separation prefixes (`0x00` for leaves, `0x01` for internal
//! nodes) are mandatory: without them an internal node could be presented as
//! a leaf (second-preimage). The
//! Bitcoin duplicate-last-leaf variant is deliberately NOT used — it admits two
//! distinct trees with one root (CVE-2012-2459).

use sha2::{Digest, Sha256};

/// `MTH({}) = SHA-256("")` — the root of an empty leaf set (RFC 6962 §2.1).
fn empty_root() -> [u8; 32] {
    Sha256::digest([]).into()
}

/// Leaf hash `SHA-256(0x00 ‖ leaf)`.
fn leaf_hash(leaf: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update([0x00]);
    h.update(leaf);
    h.finalize().into()
}

/// Node hash `SHA-256(0x01 ‖ left ‖ right)`.
fn node_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update([0x01]);
    h.update(left);
    h.update(right);
    h.finalize().into()
}

/// The split point `k` from RFC 6962 §2.1: the largest power of two **strictly
/// smaller** than `n`. Only called with `n >= 2`, so `k >= 1` and `k < n`.
fn split_point(n: usize) -> usize {
    debug_assert!(n >= 2);
    let mut k = 1;
    while k << 1 < n {
        k <<= 1;
    }
    k
}

/// Merkle Tree Hash (RFC 6962 §2.1) over the ordered leaf inputs.
///
/// The tree is left-full, not balanced: a subtree with `n` leaves splits into a
/// left of `k` (the largest power of two `< n`) and a right of `n - k`.
pub fn root(leaves: &[Vec<u8>]) -> [u8; 32] {
    match leaves.len() {
        0 => empty_root(),
        1 => leaf_hash(&leaves[0]),
        n => {
            let k = split_point(n);
            node_hash(&root(&leaves[..k]), &root(&leaves[k..]))
        }
    }
}

/// Inclusion proof `PATH(index, leaves)` per RFC 6962 §2.1.1: the sibling hashes
/// from leaf `index` up to the root, ordered **leaf-upward** (closest sibling
/// first). Returns `None` if `index` is out of range.
///
/// The length is **at most** `⌈log₂ n⌉` and **varies per leaf** — for `n = 3`,
/// `PATH(2)` is a single element `[MTH(d0, d1)]`, because `d2` sits one level
/// higher than `d0`/`d1`. A consumer that validates the length as an equality is
/// wrong; use [`verify_proof`], which is size-aware.
pub fn proof(leaves: &[Vec<u8>], index: usize) -> Option<Vec<[u8; 32]>> {
    if index >= leaves.len() {
        return None;
    }
    Some(path(leaves, index))
}

/// `PATH(m, D[n])`, RFC 6962 §2.1.1. `m < n` is guaranteed by [`proof`].
fn path(leaves: &[Vec<u8>], m: usize) -> Vec<[u8; 32]> {
    let n = leaves.len();
    if n == 1 {
        return Vec::new();
    }
    let k = split_point(n);
    // The sibling for the top split is pushed LAST, so the vector reads
    // leaf-upward. [`verify_proof`] consumes it from the end to mirror this.
    if m < k {
        let mut p = path(&leaves[..k], m);
        p.push(root(&leaves[k..]));
        p
    } else {
        let mut p = path(&leaves[k..], m - k);
        p.push(root(&leaves[..k]));
        p
    }
}

/// Verify an inclusion proof: reconstruct the root from `leaf`, its `index`, the
/// tree size `n` and the leaf-upward sibling `path`, and compare to `root`.
///
/// Returns `false` for any inconsistency: index out of range, a `path` whose
/// length does not match the position of `index` in a tree of size `n`, a wrong
/// sibling, a wrong leaf, or a wrong `root`. The tree size is part of the check —
/// this is why the emitted proof object carries `tree_size`.
pub fn verify_proof(
    root: &[u8; 32],
    leaf: &[u8],
    index: usize,
    n: usize,
    path: &[[u8; 32]],
) -> bool {
    if index >= n {
        return false;
    }
    match recompute(leaf_hash(leaf), index, n, path) {
        Some(computed) => &computed == root,
        None => false,
    }
}

/// Reconstruct a subtree root from a leaf hash and its sibling path, mirroring
/// [`path`]: the top-split sibling is the LAST path element, so it is consumed
/// first. `None` on any length mismatch (path too long or too short for `n`).
fn recompute(node: [u8; 32], m: usize, n: usize, path: &[[u8; 32]]) -> Option<[u8; 32]> {
    if n == 1 {
        // A single-leaf (sub)tree has no siblings; any leftover path is wrong.
        return path.is_empty().then_some(node);
    }
    let (sib, rest) = path.split_last()?;
    let k = split_point(n);
    if m < k {
        let left = recompute(node, m, k, rest)?;
        Some(node_hash(&left, sib))
    } else {
        let right = recompute(node, m - k, n - k, rest)?;
        Some(node_hash(sib, &right))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(bytes: &[u8]) -> [u8; 32] {
        Sha256::digest(bytes).into()
    }

    /// Leaf hash spelled out (`00 ‖ d`), independent of `leaf_hash`.
    fn lh(d: &[u8]) -> [u8; 32] {
        let mut v = vec![0x00u8];
        v.extend_from_slice(d);
        h(&v)
    }

    /// Node hash spelled out (`01 ‖ l ‖ r`), independent of `node_hash`.
    fn nh(l: &[u8; 32], r: &[u8; 32]) -> [u8; 32] {
        let mut v = vec![0x01u8];
        v.extend_from_slice(l);
        v.extend_from_slice(r);
        h(&v)
    }

    fn leaves(items: &[&[u8]]) -> Vec<Vec<u8>> {
        items.iter().map(|s| s.to_vec()).collect()
    }

    // --- reproduces §2's expression trees for n ∈ {0,1,2,3,7} ---------
    // Each expected value is the exact expression tree from the SPEC, built from
    // the hand-spelled `lh`/`nh` (not from `root`), so this pins the definition.

    #[test]
    fn expr_tree_n0_is_sha256_empty() {
        assert_eq!(root(&[]), h(&[])); // MTH({}) = SHA-256("")
    }

    #[test]
    fn expr_tree_n1() {
        let d = leaves(&[b"d0"]);
        assert_eq!(root(&d), lh(b"d0")); // H(00‖d0)
    }

    #[test]
    fn expr_tree_n2() {
        let d = leaves(&[b"d0", b"d1"]);
        // H(01 ‖ H(00‖d0) ‖ H(00‖d1))
        assert_eq!(root(&d), nh(&lh(b"d0"), &lh(b"d1")));
    }

    #[test]
    fn expr_tree_n3_leaf_two_sits_higher() {
        let d = leaves(&[b"d0", b"d1", b"d2"]);
        // H(01 ‖ H(01 ‖ H(00‖d0) ‖ H(00‖d1)) ‖ H(00‖d2))
        let left = nh(&lh(b"d0"), &lh(b"d1"));
        assert_eq!(root(&d), nh(&left, &lh(b"d2")));
    }

    #[test]
    fn expr_tree_n7_left_full() {
        let d = leaves(&[b"d0", b"d1", b"d2", b"d3", b"d4", b"d5", b"d6"]);
        // H( 01 ‖ H(01 ‖ H(01‖L0‖L1) ‖ H(01‖L2‖L3))
        //       ‖ H(01 ‖ H(01‖L4‖L5) ‖ L6) )
        let l = |i: &[u8]| lh(i);
        let left = nh(&nh(&l(b"d0"), &l(b"d1")), &nh(&l(b"d2"), &l(b"d3")));
        let right = nh(&nh(&l(b"d4"), &l(b"d5")), &l(b"d6"));
        assert_eq!(root(&d), nh(&left, &right));
    }

    // --- the 0x00 / 0x01 prefixes are actually applied ----------------

    #[test]
    fn single_leaf_uses_leaf_prefix_not_raw() {
        let d = leaves(&[b"x"]);
        assert_eq!(root(&d), h(&[0x00, b'x'])); // 00‖x
        assert_ne!(root(&d), h(b"x")); // NOT the raw content
    }

    #[test]
    fn node_uses_node_prefix_second_preimage_resistant() {
        let d = leaves(&[b"a", b"b"]);
        let l0 = lh(b"a");
        let l1 = lh(b"b");
        // Correct: 01-prefixed node.
        assert_eq!(root(&d), h(&[&[0x01u8], &l0[..], &l1[..]].concat()));
        // A node presented WITHOUT the 0x01 prefix (as if it were a leaf's
        // content) must NOT collide with the real root.
        assert_ne!(root(&d), h(&[&l0[..], &l1[..]].concat()));
    }

    // --- agrees with an independent conforming implementation ---------
    // The reference is a genuinely different algorithm — the incremental,
    // stack-based construction a CT log uses when appending leaves one at a time
    // — checked against the top-down recursion in `root` on a deterministic
    // pseudo-random corpus across every size, not just powers of two.

    /// Independent conforming reference. Each new leaf pushes a size-1 subtree;
    /// two top subtrees of equal size merge into a perfect subtree (so the stack
    /// holds subtrees of strictly decreasing power-of-two sizes, the binary
    /// decomposition of `n`). Finally the peaks are "bagged" right-to-left,
    /// which reproduces RFC 6962's left-full tree for any `n`.
    fn reference_root(leaves: &[Vec<u8>]) -> [u8; 32] {
        if leaves.is_empty() {
            return h(&[]);
        }
        // stack of (subtree_root, size).
        let mut stack: Vec<([u8; 32], usize)> = Vec::new();
        for d in leaves {
            stack.push((lh(d), 1));
            while stack.len() >= 2 {
                let (_, s1) = stack[stack.len() - 1];
                let (_, s2) = stack[stack.len() - 2];
                if s1 != s2 {
                    break;
                }
                let (r, _) = stack.pop().unwrap();
                let (l, _) = stack.pop().unwrap();
                stack.push((nh(&l, &r), s1 * 2));
            }
        }
        // Bag the peaks: fold the two rightmost (smallest) subtrees upward until
        // one root remains — node(left, right) with the smaller tree on the right.
        while stack.len() > 1 {
            let (r, _) = stack.pop().unwrap();
            let (l, _) = stack.pop().unwrap();
            stack.push((nh(&l, &r), 0));
        }
        stack[0].0
    }

    #[test]
    fn agrees_with_independent_reference_on_every_size() {
        let mut state: u64 = 0x9E3779B97F4A7C15; // fixed seed, deterministic
        let mut rng = || {
            // xorshift64
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for n in 0..=40usize {
            let ls: Vec<Vec<u8>> = (0..n)
                .map(|_| {
                    let len = (rng() % 40) as usize;
                    (0..len).map(|_| (rng() & 0xff) as u8).collect()
                })
                .collect();
            assert_eq!(root(&ls), reference_root(&ls), "mismatch at n={n}");
        }
    }

    // --- inclusion proofs --------------------------------------------

    #[test]
    fn proof_n3_leaf2_has_length_one() {
        let d = leaves(&[b"d0", b"d1", b"d2"]);
        let p = proof(&d, 2).unwrap();
        assert_eq!(p.len(), 1, "PATH(2) for n=3 is a single element");
        assert_eq!(p[0], nh(&lh(b"d0"), &lh(b"d1")));
    }

    #[test]
    fn proof_out_of_range_is_none() {
        let d = leaves(&[b"d0", b"d1"]);
        assert!(proof(&d, 2).is_none());
    }

    #[test]
    fn proof_roundtrip_every_leaf_every_size() {
        for n in 1..=9usize {
            let ls: Vec<Vec<u8>> = (0..n).map(|i| format!("leaf-{i}").into_bytes()).collect();
            let r = root(&ls);
            for i in 0..n {
                let p = proof(&ls, i).unwrap();
                assert!(
                    verify_proof(&r, &ls[i], i, n, &p),
                    "roundtrip failed n={n} i={i}"
                );
            }
        }
    }

    #[test]
    fn verify_rejects_wrong_leaf() {
        let ls = leaves(&[b"d0", b"d1", b"d2", b"d3"]);
        let r = root(&ls);
        let p = proof(&ls, 1).unwrap();
        assert!(verify_proof(&r, b"d1", 1, 4, &p));
        assert!(!verify_proof(&r, b"WRONG", 1, 4, &p));
    }

    #[test]
    fn verify_rejects_wrong_index() {
        let ls = leaves(&[b"d0", b"d1", b"d2", b"d3"]);
        let r = root(&ls);
        let p = proof(&ls, 1).unwrap();
        // Right leaf bytes, wrong claimed index.
        assert!(!verify_proof(&r, b"d1", 2, 4, &p));
    }

    #[test]
    fn verify_rejects_wrong_tree_size() {
        // A wrong `n` is rejected whenever it changes the expected proof
        // structure. For a genuine size-4 proof of leaf 2 (path length 2),
        // claiming n=8 makes the path too short and n=3 makes it too long —
        // both structurally impossible, so both are rejected.
        //
        // An `n` that yields the *same* left/right structure for this index
        // (e.g. n=3 vs n=4 at index 1) is indistinguishable — the audit path
        // itself already commits to the sibling values, so that is a valid
        // proof, not a soundness gap. The proof object still carries
        // `tree_size` because the verifier needs it to reconstruct the
        // structure, and it is checked wherever the structure depends on it.
        let ls = leaves(&[b"d0", b"d1", b"d2", b"d3"]);
        let r = root(&ls);
        let p = proof(&ls, 2).unwrap();
        assert!(verify_proof(&r, b"d2", 2, 4, &p)); // honest size verifies
        assert!(!verify_proof(&r, b"d2", 2, 8, &p)); // too short for n=8
        assert!(!verify_proof(&r, b"d2", 2, 3, &p)); // too long for n=3
    }

    #[test]
    fn verify_rejects_each_tampered_sibling() {
        let ls = leaves(&[b"d0", b"d1", b"d2", b"d3", b"d4"]);
        let r = root(&ls);
        let p = proof(&ls, 0).unwrap();
        assert!(p.len() >= 2);
        for i in 0..p.len() {
            let mut bad = p.clone();
            bad[i][0] ^= 0x01; // flip one bit of sibling i
            assert!(
                !verify_proof(&r, &ls[0], 0, ls.len(), &bad),
                "tampered sibling {i} accepted"
            );
        }
    }

    // --- one-byte mutation changes the root ---------------------------------

    #[test]
    fn one_byte_mutation_changes_root() {
        let a = leaves(&[b"alpha", b"beta", b"gamma"]);
        let mut b = a.clone();
        b[1][0] ^= 0x01; // flip one bit in one leaf
        assert_ne!(root(&a), root(&b));
    }
}
