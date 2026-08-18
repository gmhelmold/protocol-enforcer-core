// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Transcript-root witness signature (the notary's *seal*, not just its
//! evidence).
//!
//! The transcript root committed by the ledger is tamper-**evident**: anyone who
//! can rewrite the ledger recomputes a matching root, and `notarize --expect`
//! then passes over the forgery. This crate closes that gap with an Ed25519
//! **detached signature over the root, produced by a witness whose signing key
//! is held outside the ledger writer** (the gateway). The gateway can rewrite
//! the ledger and recompute a root; it cannot forge this signature, because it
//! never holds the key. That — key custody separated from the writer — is the
//! whole security property. If the same principal both writes the ledger and
//! signs, this buys nothing (see `notary-sign`'s custody notes).
//!
//! What it does NOT prove: that the captured reasoning is true, complete, or
//! good. The seal is integrity + non-repudiation of *what was captured* — the
//! passivity invariant. A liar's signed transcript is a signed record of the
//! lie, not a certificate of honesty.
//!
//! Pre-image framing follows the sibling `hugit` forge's convention
//! (length-prefixed fields handed straight to Ed25519, no extra hash layer) and
//! verification uses `verify_strict`, rejecting the Ed25519
//! malleability / small-order edge cases.

pub mod approval;

pub use approval::{
    approval_message, is_valid_approver_pubkey, new_challenge_hex, pubkey_hex_from_seed_hex,
    sign_approval, sign_approval_hex, verify_approval_hex, APPROVAL_DOMAIN,
};

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use protocol_ledger::to_hex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Domain-separation tag. Binds a signature to *this* purpose so it can never be
/// replayed as some other Ed25519 message in the system. Versioned: a change to
/// the pre-image layout MUST bump this.
pub const DOMAIN: &[u8] = b"protocol/transcript-root/v1";

/// Decode lowercase-or-upper hex into bytes. `None` on any odd length or
/// non-hex nibble — fail-closed, never a partial decode.
pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let b = s.as_bytes();
    let nib = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let mut i = 0;
    while i < b.len() {
        out.push((nib(b[i])? << 4) | nib(b[i + 1])?);
        i += 2;
    }
    Some(out)
}

fn push_lp(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(bytes);
}

/// The exact bytes signed and verified — the SINGLE realisation, called by both
/// the signer and the verifier so they cannot drift.
///
/// `DOMAIN ‖ LP(session_id) ‖ root32`, where `LP(x) = u32_be(len) ‖ x`. The raw
/// 32-byte root is appended (not its hex string). Signing the root transitively
/// covers everything the root commits to — every ledger line, and thus the
/// `profile_sha256` / `output_sha256` / artifact digests carried inside them —
/// so those need not be re-listed here.
pub fn root_preimage(session_id: &str, root: &[u8; 32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(DOMAIN.len() + 4 + session_id.len() + 32);
    buf.extend_from_slice(DOMAIN);
    push_lp(&mut buf, session_id.as_bytes());
    buf.extend_from_slice(root);
    buf
}

/// `key_id = lower_hex(SHA-256(pubkey_bytes))[..16]` — the same convention the
/// `hugit` keyset uses, so a witness key can be referenced compactly.
pub fn key_id(vk: &VerifyingKey) -> String {
    to_hex(&Sha256::digest(vk.as_bytes()))[..16].to_string()
}

/// Sign the transcript root for `session_id`.
pub fn sign_root(sk: &SigningKey, session_id: &str, root: &[u8; 32]) -> Signature {
    sk.sign(&root_preimage(session_id, root))
}

/// Verify a detached root signature with `verify_strict` (rejects malleable /
/// small-order signatures). Fail-closed: `false` on any mismatch.
pub fn verify_root(vk: &VerifyingKey, session_id: &str, root: &[u8; 32], sig: &Signature) -> bool {
    vk.verify_strict(&root_preimage(session_id, root), sig)
        .is_ok()
}

/// Load a signing key from a 32-byte seed in hex (the secret). `None` on bad
/// length/hex.
pub fn signing_key_from_seed_hex(seed_hex: &str) -> Option<SigningKey> {
    let arr: [u8; 32] = from_hex(seed_hex)?.try_into().ok()?;
    Some(SigningKey::from_bytes(&arr))
}

/// Parse a 32-byte Ed25519 public key from hex. `None` on bad length/hex/point.
pub fn verifying_key_from_hex(pubkey_hex: &str) -> Option<VerifyingKey> {
    let arr: [u8; 32] = from_hex(pubkey_hex)?.try_into().ok()?;
    VerifyingKey::from_bytes(&arr).ok()
}

/// Parse a 64-byte Ed25519 signature from hex. `None` on bad length/hex.
pub fn signature_from_hex(sig_hex: &str) -> Option<Signature> {
    let arr: [u8; 64] = from_hex(sig_hex)?.try_into().ok()?;
    Some(Signature::from_bytes(&arr))
}

/// Verify a root signature straight from hex forms — a string-only surface so
/// callers (e.g. the `notarize` verifier) need not depend on `ed25519-dalek`.
/// Fail-closed on any bad key/sig hex.
pub fn verify_root_hex(pubkey_hex: &str, session_id: &str, root: &[u8; 32], sig_hex: &str) -> bool {
    match (
        verifying_key_from_hex(pubkey_hex),
        signature_from_hex(sig_hex),
    ) {
        (Some(vk), Some(sig)) => verify_root(&vk, session_id, root, &sig),
        _ => false,
    }
}

/// `key_id` for a hex public key, or `None` if it does not parse.
pub fn key_id_from_hex(pubkey_hex: &str) -> Option<String> {
    verifying_key_from_hex(pubkey_hex).as_ref().map(key_id)
}

/// The detached-signature sidecar written next to a session's ledger. The
/// `pubkey` field is a *convenience* copy: a verifier MUST supply the public key
/// it trusts out of band, never adopt this one, or the signature proves nothing
/// (an attacker would just ship a matching key). Verifying against a
/// caller-supplied key whose `key_id` differs from this one is a mismatch worth
/// surfacing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SigFile {
    pub domain: String,
    pub session_id: String,
    pub transcript_root: String,
    pub key_id: String,
    pub pubkey: String,
    pub sig: String,
}

impl SigFile {
    pub fn new(session_id: &str, root: &[u8; 32], vk: &VerifyingKey, sig: &Signature) -> Self {
        SigFile {
            domain: String::from_utf8_lossy(DOMAIN).into_owned(),
            session_id: session_id.to_string(),
            transcript_root: to_hex(root),
            key_id: key_id(vk),
            pubkey: to_hex(vk.as_bytes()),
            sig: to_hex(&sig.to_bytes()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }

    #[test]
    fn hex_roundtrip_and_reject() {
        assert_eq!(from_hex("00ff10").unwrap(), vec![0x00, 0xff, 0x10]);
        assert_eq!(from_hex("AB").unwrap(), vec![0xab]);
        assert!(from_hex("abc").is_none()); // odd
        assert!(from_hex("zz").is_none()); // non-hex
    }

    #[test]
    fn sign_then_verify_roundtrips() {
        let sk = seed(7);
        let vk = sk.verifying_key();
        let root = [42u8; 32];
        let sig = sign_root(&sk, "sess", &root);
        assert!(verify_root(&vk, "sess", &root, &sig));
    }

    #[test]
    fn wrong_session_root_or_key_fails() {
        let sk = seed(7);
        let vk = sk.verifying_key();
        let root = [42u8; 32];
        let sig = sign_root(&sk, "sess", &root);

        // different session id → domain/LP changes → invalid
        assert!(!verify_root(&vk, "other", &root, &sig));
        // one-bit root change → invalid (this is the tamper case)
        let mut tampered = root;
        tampered[0] ^= 1;
        assert!(!verify_root(&vk, "sess", &tampered, &sig));
        // wrong key → invalid
        let other = seed(9).verifying_key();
        assert!(!verify_root(&other, "sess", &root, &sig));
    }

    #[test]
    fn preimage_is_domain_separated() {
        // The domain tag is a prefix, so a message crafted without it cannot
        // collide with a signed root pre-image.
        let pre = root_preimage("s", &[0u8; 32]);
        assert!(pre.starts_with(DOMAIN));
    }

    #[test]
    fn sigfile_hex_fields_parse_back() {
        let sk = seed(3);
        let vk = sk.verifying_key();
        let root = [1u8; 32];
        let sig = sign_root(&sk, "s", &root);
        let sf = SigFile::new("s", &root, &vk, &sig);
        assert_eq!(sf.domain, "protocol/transcript-root/v1");
        assert_eq!(verifying_key_from_hex(&sf.pubkey).unwrap(), vk);
        let parsed_sig = signature_from_hex(&sf.sig).unwrap();
        assert!(verify_root(&vk, "s", &root, &parsed_sig));
        assert_eq!(sf.key_id, key_id(&vk));
        assert_eq!(sf.key_id.len(), 16);
    }

    #[test]
    fn seed_hex_loads_key() {
        let hex = "07".repeat(32);
        let sk = signing_key_from_seed_hex(&hex).unwrap();
        assert_eq!(sk.to_bytes(), [7u8; 32]);
        assert!(signing_key_from_seed_hex("07").is_none()); // too short
    }
}
