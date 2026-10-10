//! The seal that ties a ledger event to the key agents cannot read. The seal
//! is an HMAC-SHA256 under the rerun cache key over the event's kind, time,
//! parents and data, so a copied or hand-written proof fails the check.

use std::io::Write;
use std::path::{Path, PathBuf};

use hmac::{Hmac, KeyInit, Mac};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::Sha256;

use crate::error::{Error, Result};
use crate::ledger::{Event, Kind};

pub const KEY_FOLDER: &str = "hidden-checks";
pub const KEY_FILE: &str = ".cache-key";
pub const SEAL_FIELD: &str = "seal";

/// The key file under the rvow home, created on first use.
pub fn key_path(home: &Path) -> PathBuf {
    home.join(KEY_FOLDER).join(KEY_FILE)
}

/// The key, or `None` when it does not exist or is the wrong size.
pub fn read_key(home: &Path) -> Option<Vec<u8>> {
    std::fs::read(key_path(home)).ok().filter(|k| k.len() == 32)
}

/// The key, created with mode 0600 on first use.
pub fn key(home: &Path) -> Result<Vec<u8>> {
    if let Some(k) = read_key(home) {
        return Ok(k);
    }
    let path = key_path(home);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    let secret = Error::random::<32>()?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut file = opts.open(&path).map_err(|e| Error::io(&path, e))?;
    file.write_all(&secret).map_err(|e| Error::io(&path, e))?;
    Ok(secret.to_vec())
}

/// The hex HMAC-SHA256 of `text` under `key`.
pub fn mac(key: &[u8], text: &str) -> Result<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|e| Error::internal(format!("bad cache key: {e}")))?;
    mac.update(text.as_bytes());
    Ok(mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect())
}

/// Whether `hex` is the HMAC of `text` under `key`.
pub fn verify(key: &[u8], text: &str, hex: &str) -> bool {
    let Some(bytes) = hex_decode(hex) else { return false };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else { return false };
    mac.update(text.as_bytes());
    mac.verify_slice(&bytes).is_ok()
}

/// The payload a seal covers: the event without its seal field.
#[derive(Serialize)]
struct Payload<'a> {
    kind: Kind,
    time: u64,
    parents: &'a [String],
    data: &'a Map<String, Value>,
}

fn payload(kind: Kind, time: u64, parents: &[String], data: &Map<String, Value>) -> Result<String> {
    serde_json::to_string(&Payload { kind, time, parents, data }).map_err(|e| Error::internal(format!("event does not serialize: {e}")))
}

/// The seal of an event whose kind, time, parents and data are already chosen.
pub fn seal(key: &[u8], kind: Kind, time: u64, parents: &[String], data: &Map<String, Value>) -> Result<String> {
    mac(key, &payload(kind, time, parents, data)?)
}

/// Whether `ev` carries a seal that checks under `key`.
pub fn checks(key: &[u8], ev: &Event) -> bool {
    let Some(hex) = ev.data.get(SEAL_FIELD).and_then(Value::as_str) else { return false };
    let mut data = ev.data.clone();
    data.remove(SEAL_FIELD);
    payload(ev.kind, ev.time, &ev.parents, &data).is_ok_and(|text| verify(key, &text, hex))
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}
