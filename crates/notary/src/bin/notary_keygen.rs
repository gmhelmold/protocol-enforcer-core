// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! `notary-keygen` — mint an Ed25519 witness keypair for transcript-root
//! signing.
//!
//! Prints the 32-byte seed (the SECRET), the public key, and the key id, all in
//! hex. The seed must be stored where the ledger writer (the gateway) cannot
//! read it — that separation is the entire point of the witness (see
//! `protocol_notary`'s crate docs). This tool never touches a ledger.

use ed25519_dalek::SigningKey;
use protocol_notary::key_id;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| format!("getrandom failed: {e}"))?;
    let sk = SigningKey::from_bytes(&seed);
    let vk = sk.verifying_key();

    let seed_hex = to_hex(&seed);
    let pub_hex = to_hex(vk.as_bytes());

    // stderr carries the human warning; stdout carries only the machine fields
    // so `notary-keygen | ...` stays clean.
    eprintln!("SECRET seed — store outside the ledger writer's reach, never commit:");
    println!("seed:   {seed_hex}");
    println!("pubkey: {pub_hex}");
    println!("key_id: {}", key_id(&vk));
    Ok(())
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}
