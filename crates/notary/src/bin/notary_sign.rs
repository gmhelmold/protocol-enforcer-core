// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! `notary-sign` — the out-of-band witness that signs a session's transcript
//! root.
//!
//! Reads the ledger to recompute the root (read-only; never writes to it),
//! signs `DOMAIN ‖ LP(session_id) ‖ root` with a key supplied from custody, and
//! writes a `<session>.sig` sidecar. `notarize --verify-sig <pubkey>` checks it.
//!
//! KEY CUSTODY: the seed comes from `--key-seed` / `$PROTOCOL_NOTARY_SK` or a
//! `--key-file`, NOT from anything the gateway controls. Security holds only
//! while whoever can rewrite the ledger cannot obtain this seed. Running this on
//! the same host/principal as the gateway with the seed readable there defeats
//! the purpose — treat that as a downgrade to tamper-evident-only.

use std::path::PathBuf;

use clap::Parser;
use protocol_ledger::transcript_root_bytes;
use protocol_notary::{sign_root, signing_key_from_seed_hex, SigFile};

#[derive(Parser)]
#[command(name = "notary-sign", about = "Witness-sign a session transcript root")]
struct Args {
    /// Session id (the ledger file stem).
    session_id: String,

    /// Ledger directory (matches the gateway's PROTOCOL_LEDGER_DIR).
    #[arg(long, env = "PROTOCOL_LEDGER_DIR", default_value = "./ledger")]
    ledger_dir: PathBuf,

    /// 32-byte signing seed in hex. Prefer the env form so it stays off argv.
    #[arg(long, env = "PROTOCOL_NOTARY_SK")]
    key_seed: Option<String>,

    /// File containing the 32-byte signing seed in hex (alternative to --key-seed).
    #[arg(long)]
    key_file: Option<PathBuf>,

    /// Where to write the signature sidecar. Defaults to
    /// <ledger_dir>/<session>.sig.
    #[arg(long)]
    out: Option<PathBuf>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let seed_hex = match (args.key_seed, &args.key_file) {
        (Some(s), _) => s,
        (None, Some(p)) => std::fs::read_to_string(p)?.trim().to_string(),
        (None, None) => {
            eprintln!(
                "notary-sign: no signing key. Provide --key-seed / $PROTOCOL_NOTARY_SK \
                 or --key-file. The seed must live outside the ledger writer's reach."
            );
            std::process::exit(2);
        }
    };
    let sk = match signing_key_from_seed_hex(&seed_hex) {
        Some(k) => k,
        None => {
            eprintln!("notary-sign: signing key is not a valid 32-byte hex seed");
            std::process::exit(2);
        }
    };

    let root = match transcript_root_bytes(&args.session_id, &args.ledger_dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "notary-sign: cannot read ledger for '{}': {}",
                args.session_id, e
            );
            std::process::exit(2);
        }
    };

    let vk = sk.verifying_key();
    let sig = sign_root(&sk, &args.session_id, &root);
    let sig_file = SigFile::new(&args.session_id, &root, &vk, &sig);

    let out = args
        .out
        .unwrap_or_else(|| args.ledger_dir.join(format!("{}.sig", args.session_id)));
    std::fs::write(&out, serde_json::to_string_pretty(&sig_file)?)?;

    println!("session:         {}", sig_file.session_id);
    println!("transcript_root: {}", sig_file.transcript_root);
    println!("key_id:          {}", sig_file.key_id);
    println!("signature:       {}", out.display());
    Ok(())
}
