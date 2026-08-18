// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Human-approval signatures — the *runtime* sibling
//! of the transcript-root witness in this crate's root module.
//!
//! Same custody story, different object: there the witness signs a finished
//! transcript so the ledger writer cannot forge history; here a HUMAN signs a
//! per-entry challenge so the AGENT cannot pass a gate on its own. Both live in
//! this crate so the workspace has exactly ONE Ed25519 implementation and one
//! hex codec — a second one would be a second thing to get wrong.
//!
//! The message is a plain ASCII string rather than the root module's
//! length-prefixed framing, because a human has to be able to eyeball what they
//! are signing (`pe-approval-v1:<session>:<macro>:<sub_state>:<challenge>`).
//! Injection-safety comes from the field alphabets, not from framing: session
//! ids are `[A-Za-z0-9_-]` (gateway-enforced), the challenge is hex, and the
//! macro/sub-state ids are profile-authored constants, so no field can contain
//! the `:` separator and shift the parse. The `pe-approval-v1:` prefix is the
//! domain tag: a change to this layout MUST bump it.

use ed25519_dalek::{Signer, SigningKey};
use protocol_ledger::to_hex;

use crate::{signature_from_hex, verifying_key_from_hex};

/// Domain-separation tag + version. Prefix of every approval message.
pub const APPROVAL_DOMAIN: &str = "pe-approval-v1";

/// Length in bytes of a challenge nonce (hex-encodes to 64 chars).
pub const CHALLENGE_BYTES: usize = 32;

/// The EXACT bytes signed and verified — the single realisation, called by the
/// signer (`protocol-enforcer approve`) and the verifier (the fsm gate) alike so
/// they cannot drift:
/// `pe-approval-v1:<session_id>:<macro_id>:<sub_state_id>:<challenge_hex>`.
pub fn approval_message(
    session_id: &str,
    macro_id: &str,
    sub_state_id: &str,
    challenge_hex: &str,
) -> String {
    format!("{APPROVAL_DOMAIN}:{session_id}:{macro_id}:{sub_state_id}:{challenge_hex}")
}

/// A fresh 32-byte challenge nonce in lowercase hex, from the OS CSPRNG.
/// `Err` only if the OS entropy source itself fails — fail CLOSED there rather
/// than degrade to a predictable nonce, which would let an agent pre-harvest a
/// signature for a gate it has not reached yet.
pub fn new_challenge_hex() -> Result<String, String> {
    let mut buf = [0u8; CHALLENGE_BYTES];
    getrandom::getrandom(&mut buf).map_err(|e| format!("getrandom failed: {e}"))?;
    Ok(to_hex(&buf))
}

/// Verify an approval signature straight from hex forms — a string-only surface
/// so the fsm never needs to depend on `ed25519-dalek` itself. Fail-closed:
/// `false` on bad key hex, bad signature hex, or any verification failure.
/// Uses `verify_strict` (via [`crate::verify_root`]'s sibling path) semantics by
/// going through `VerifyingKey::verify_strict`, rejecting the Ed25519
/// malleability / small-order edge cases.
pub fn verify_approval_hex(
    approver_pubkey_hex: &str,
    session_id: &str,
    macro_id: &str,
    sub_state_id: &str,
    challenge_hex: &str,
    signature_hex: &str,
) -> bool {
    let (Some(vk), Some(sig)) = (
        verifying_key_from_hex(approver_pubkey_hex),
        signature_from_hex(signature_hex),
    ) else {
        return false;
    };
    let msg = approval_message(session_id, macro_id, sub_state_id, challenge_hex);
    vk.verify_strict(msg.as_bytes(), &sig).is_ok()
}

/// Sign an approval with an in-memory key. The human-facing entry point is
/// [`sign_approval_hex`]; this exists for callers that already hold a key.
pub fn sign_approval(
    sk: &SigningKey,
    session_id: &str,
    macro_id: &str,
    sub_state_id: &str,
    challenge_hex: &str,
) -> String {
    let msg = approval_message(session_id, macro_id, sub_state_id, challenge_hex);
    to_hex(&sk.sign(msg.as_bytes()).to_bytes())
}

/// Sign an approval from a 32-byte hex seed, returning the hex signature.
/// `None` when the seed is not valid 32-byte hex.
pub fn sign_approval_hex(
    seed_hex: &str,
    session_id: &str,
    macro_id: &str,
    sub_state_id: &str,
    challenge_hex: &str,
) -> Option<String> {
    let sk = crate::signing_key_from_seed_hex(seed_hex)?;
    Some(sign_approval(
        &sk,
        session_id,
        macro_id,
        sub_state_id,
        challenge_hex,
    ))
}

/// The hex PUBLIC key for a 32-byte hex seed — what a profile's
/// `approver_pubkey` must be set to for that key to be accepted. `None` on bad
/// seed hex.
pub fn pubkey_hex_from_seed_hex(seed_hex: &str) -> Option<String> {
    crate::signing_key_from_seed_hex(seed_hex).map(|sk| to_hex(sk.verifying_key().as_bytes()))
}

/// Is this a syntactically well-formed `approver_pubkey`? (32-byte hex that
/// decodes to a valid Ed25519 point.) Used by the profile validator so an
/// unusable key is caught at authoring time, not at the gate.
pub fn is_valid_approver_pubkey(pubkey_hex: &str) -> bool {
    verifying_key_from_hex(pubkey_hex).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: &str = "0707070707070707070707070707070707070707070707070707070707070707";
    const OTHER_SEED: &str = "0909090909090909090909090909090909090909090909090909090909090909";

    #[test]
    fn message_is_the_exact_documented_format() {
        assert_eq!(
            approval_message("s1", "approve", "human_gate", "ab12"),
            "pe-approval-v1:s1:approve:human_gate:ab12"
        );
    }

    #[test]
    fn sign_then_verify_roundtrips() {
        let pk = pubkey_hex_from_seed_hex(SEED).unwrap();
        let sig = sign_approval_hex(SEED, "s1", "m", "sub", "beef").unwrap();
        assert!(verify_approval_hex(&pk, "s1", "m", "sub", "beef", &sig));
    }

    #[test]
    fn every_bound_field_is_load_bearing() {
        let pk = pubkey_hex_from_seed_hex(SEED).unwrap();
        let sig = sign_approval_hex(SEED, "s1", "m", "sub", "beef").unwrap();
        // Change any one of the four bound fields → invalid.
        assert!(!verify_approval_hex(&pk, "s2", "m", "sub", "beef", &sig));
        assert!(!verify_approval_hex(&pk, "s1", "m2", "sub", "beef", &sig));
        assert!(!verify_approval_hex(&pk, "s1", "m", "sub2", "beef", &sig));
        assert!(!verify_approval_hex(&pk, "s1", "m", "sub", "dead", &sig));
        // Wrong key → invalid.
        let other = pubkey_hex_from_seed_hex(OTHER_SEED).unwrap();
        assert!(!verify_approval_hex(&other, "s1", "m", "sub", "beef", &sig));
    }

    #[test]
    fn malformed_inputs_fail_closed() {
        let pk = pubkey_hex_from_seed_hex(SEED).unwrap();
        let sig = sign_approval_hex(SEED, "s1", "m", "sub", "beef").unwrap();
        assert!(!verify_approval_hex("zz", "s1", "m", "sub", "beef", &sig));
        assert!(!verify_approval_hex(
            &pk, "s1", "m", "sub", "beef", "nothex"
        ));
        assert!(!verify_approval_hex(&pk, "s1", "m", "sub", "beef", ""));
        assert!(sign_approval_hex("07", "s1", "m", "sub", "beef").is_none());
    }

    #[test]
    fn challenges_are_fresh_and_well_formed() {
        let a = new_challenge_hex().unwrap();
        let b = new_challenge_hex().unwrap();
        assert_eq!(a.len(), CHALLENGE_BYTES * 2);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "two challenges must not collide");
    }

    #[test]
    fn approver_pubkey_shape_is_checked() {
        assert!(is_valid_approver_pubkey(
            &pubkey_hex_from_seed_hex(SEED).unwrap()
        ));
        assert!(!is_valid_approver_pubkey("deadbeef")); // too short
        assert!(!is_valid_approver_pubkey(&"zz".repeat(32))); // not hex
    }
}
