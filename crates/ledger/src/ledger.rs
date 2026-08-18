// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Gustavo Schneiter
//! Ledger implementation

use crate::port::LedgerPort;
use protocol_types::{FsmEvent, LedgerError};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct Ledger {
    path: PathBuf,
    file: Arc<Mutex<File>>,
}

impl Ledger {
    pub fn new(session_id: &str, dir: &Path) -> Result<Self, LedgerError> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}.jsonl", session_id));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            file: Arc::new(Mutex::new(file)),
        })
    }

    /// Append one event. `&self` (not `&mut self`): the file lives behind
    /// `Arc<Mutex<File>>`, so appends are serialized internally and a shared
    /// `Arc<Ledger>` can be written from several threads without an external
    /// lock. The `Mutex` guarantees each append is atomic w.r.t. other appends
    /// in THIS process — no torn lines.
    ///
    /// Cross-PROCESS safety (several gateway processes sharing one
    /// `PROTOCOL_LEDGER_DIR`, e.g. an HTTP multi-instance deployment): the
    /// intra-process `Mutex` is not enough — each process holds its own open
    /// file description, so an OS advisory lock is taken for the duration of
    /// the write via `File::lock` (stable std file locking). The record is
    /// serialized once and written with a single `write_all`, so the whole
    /// line lands under one held lock; the lock is released before returning
    /// (and would auto-release on close regardless).
    ///
    /// Durability: after the line is written, the data is forced to physical
    /// storage with `sync_data` (fdatasync) before the advisory lock is
    /// released. `write_all` + `flush` alone only move the line into the OS
    /// page cache — a power-loss or hard crash immediately afterwards could
    /// still lose it. Because the sync happens before `append` returns, an
    /// `Ok` result means the event has actually hit stable storage.
    ///
    /// Sharing `PROTOCOL_LEDGER_DIR` across processes is supported only on a
    /// local POSIX filesystem: correctness rests on the per-append OS
    /// advisory lock (which serializes concurrent appends from different
    /// processes) plus the per-append fsync (which makes each append
    /// durable). Network/remote filesystems (NFS, SMB, and similar) are out
    /// of scope — POSIX advisory locks are not reliably enforced there.
    /// Deployers who need to share a ledger dir across hosts must use a
    /// local shared volume (e.g. a co-located disk) or a future durable
    /// backend; that is deferred, not solved by this crate.
    pub fn append(&self, event: &FsmEvent) -> Result<(), LedgerError> {
        let mut file = self
            .file
            .lock()
            .map_err(|_| LedgerError::Io(std::io::Error::other("Mutex poisoned")))?;
        file.lock().map_err(LedgerError::Io)?;
        let mut line = serde_json::to_string(event)?;
        line.push('\n');
        let write_res = file
            .write_all(line.as_bytes())
            .and_then(|()| file.flush())
            .and_then(|()| file.sync_data());
        // Best-effort release: on success or failure the advisory lock must be
        // dropped so other processes proceed; a failed unlock is non-fatal
        // (the lock also releases when the fd closes).
        let _ = file.unlock();
        write_res?;
        Ok(())
    }

    /// Replay every event in the session's ledger file.
    ///
    /// A malformed/partial LAST line (e.g. a crash mid-`write_all`, leaving a
    /// truncated final JSON object) is tolerated: it is dropped and the
    /// valid prefix of events is returned, with a warning logged to stderr.
    /// A malformed line anywhere else (real mid-file corruption — a later
    /// line parsed fine, so the file wasn't simply cut off) still hard-fails
    /// with `LedgerError::Corrupt`, exactly as before.
    pub fn replay(session_id: &str, dir: &Path) -> Result<Vec<FsmEvent>, LedgerError> {
        let path = dir.join(format!("{}.jsonl", session_id));
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&path)?;
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader.lines().collect::<Result<_, _>>()?;
        let last_non_empty = lines.iter().rposition(|l| !l.trim().is_empty());
        let mut events = Vec::new();

        for (line_num, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<FsmEvent>(trimmed) {
                Ok(event) => events.push(event),
                Err(e) => {
                    if Some(line_num) == last_non_empty {
                        // Malformed trailing line: tolerate it and return the
                        // valid prefix, as if the file ended before it.
                        eprintln!(
                            "warning: ledger '{}' has a malformed trailing line {} ({}); \
                             truncating to the last {} valid event(s)",
                            session_id,
                            line_num + 1,
                            e,
                            events.len()
                        );
                        return Ok(events);
                    }
                    return Err(LedgerError::Corrupt {
                        line: line_num + 1,
                        message: e.to_string(),
                    });
                }
            }
        }
        Ok(events)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Byte-exact transcript leaves for notarization (SPEC §1, NOT-2): the
    /// file's raw bytes split on `0x0A`, dropping zero-length segments. Every
    /// other byte is kept verbatim — including `\r`, including invalid UTF-8,
    /// including a torn final line left by a crash.
    ///
    /// This deliberately does NOT reuse `replay`'s `BufRead::lines()`
    /// (`ledger.rs:91-93`): that yields `String`, strips a trailing `\r`,
    /// `trim()`s, and fails the whole read on invalid UTF-8 — any of which would
    /// silently change the leaf set. The transcript root must commit to the
    /// bytes actually on disk, so this reads and splits raw `Vec<u8>`.
    ///
    /// Returns `Err` when the file is **absent** (NOT-4): a missing ledger is an
    /// error, never an empty tree. Unlike `replay`, which returns `Ok(empty)`
    /// for a non-existent path, mirroring that here would let a typo'd session
    /// id or a wrong `PROTOCOL_LEDGER_DIR` print a well-formed root that commits
    /// to nothing. The empty root is defined only for a file that *exists* and
    /// holds no non-empty line.
    ///
    /// Reads nothing beyond the file and mutates nothing (NOT-11).
    pub fn lines(session_id: &str, dir: &Path) -> Result<Vec<Vec<u8>>, LedgerError> {
        let path = dir.join(format!("{}.jsonl", session_id));
        let bytes = std::fs::read(&path)?;
        Ok(bytes
            .split(|&b| b == b'\n')
            .filter(|seg| !seg.is_empty())
            .map(<[u8]>::to_vec)
            .collect())
    }
}

impl LedgerPort for Ledger {
    fn append(&self, event: &FsmEvent) -> Result<(), LedgerError> {
        self.append(event)
    }

    fn replay(&self, session_id: &str) -> Result<Vec<FsmEvent>, LedgerError> {
        let dir = self.path().parent().unwrap_or_else(|| Path::new("."));
        Ledger::replay(session_id, dir)
    }
}

#[cfg(test)]
mod lines_tests {
    use super::*;
    use protocol_types::{FsmEvent, FsmEventType};

    // --- NOT-2: byte-exact leaves ------------------------------------------

    #[test]
    fn splits_on_lf_and_drops_empty_segments() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        // Blank line in the middle + trailing newline => two empty segments.
        std::fs::write(&p, b"a\n\nb\n").unwrap();
        let ls = Ledger::lines("s", dir.path()).unwrap();
        assert_eq!(ls, vec![b"a".to_vec(), b"b".to_vec()]);
    }

    #[test]
    fn keeps_carriage_return_and_torn_tail_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        // "a\r" keeps the CR; final "c" has no trailing LF (torn tail) yet is a leaf.
        std::fs::write(dir.path().join("s.jsonl"), b"a\r\nb\nc").unwrap();
        let ls = Ledger::lines("s", dir.path()).unwrap();
        assert_eq!(ls, vec![b"a\r".to_vec(), b"b".to_vec(), b"c".to_vec()]);
    }

    #[test]
    fn keeps_invalid_utf8_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        // 0xFF, 0xFE is not valid UTF-8 — BufRead::lines would fail; lines must not.
        std::fs::write(dir.path().join("s.jsonl"), [0xFF, 0xFE, b'\n', b'x']).unwrap();
        let ls = Ledger::lines("s", dir.path()).unwrap();
        assert_eq!(ls, vec![vec![0xFF, 0xFE], b"x".to_vec()]);
    }

    // --- NOT-4: missing vs. existing-empty ---------------------------------

    #[test]
    fn missing_file_is_err_not_empty() {
        let dir = tempfile::tempdir().unwrap();
        let r = Ledger::lines("does-not-exist", dir.path());
        assert!(matches!(r, Err(LedgerError::Io(_))));
    }

    #[test]
    fn existing_empty_file_yields_no_leaves() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("empty.jsonl"), b"").unwrap();
        assert_eq!(
            Ledger::lines("empty", dir.path()).unwrap(),
            Vec::<Vec<u8>>::new()
        );
    }

    // --- NOT-13: reads a pre-feature ledger --------------------------------

    #[test]
    fn reads_pre_feature_ledger_lines() {
        // A hand-written ledger with no new fields still yields one leaf per line
        // and each line still deserializes as an FsmEvent.
        let dir = tempfile::tempdir().unwrap();
        let ev = sample_event();
        let mut line = serde_json::to_string(&ev).unwrap();
        line.push('\n');
        std::fs::write(dir.path().join("old.jsonl"), line.as_bytes()).unwrap();

        let ls = Ledger::lines("old", dir.path()).unwrap();
        assert_eq!(ls.len(), 1);
        let back: FsmEvent = serde_json::from_slice(&ls[0]).unwrap();
        assert_eq!(back.session_id, ev.session_id);
    }

    // --- NOT-12: append framing is exactly `to_string(event) + "\n"` --------

    #[test]
    fn append_writes_exactly_json_plus_newline() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::new("frame", dir.path()).unwrap();
        let ev = sample_event();
        ledger.append(&ev).unwrap();

        let on_disk = std::fs::read(dir.path().join("frame.jsonl")).unwrap();
        let mut expected = serde_json::to_string(&ev).unwrap();
        expected.push('\n');
        assert_eq!(on_disk, expected.into_bytes());
    }

    fn sample_event() -> FsmEvent {
        // Fixed ids/timestamps so serialization is deterministic within a test.
        FsmEvent {
            event_id: uuid::Uuid::nil(),
            timestamp: chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap(),
            session_id: "frame".to_string(),
            event_type: FsmEventType::SessionStarted {
                pipeline_id: "p".to_string(),
                manifest_version: "1".to_string(),
                profile_sha256: None,
            },
            step_id: None,
            payload: serde_json::Value::Null,
        }
    }
}
