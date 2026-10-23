use std::path::Path;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::ledger::{self, Kind};
use crate::store::Project;
use crate::vow::sha256_hex;

/// Legacy payload retained for callers. Signature-enforcing gates reject it.
pub fn confirm_payload(amendment: &str, decision: &str) -> String {
    serde_json::json!({ "amendment": amendment, "decision": decision }).to_string()
}

/// Binds a decision to portable project identity and the complete amendment
/// event. Store paths and storage mode are not part of project identity.
pub fn bound_confirm_payload(project: &Project, change: &crate::id::Id, event: &ledger::Event, decision: &str) -> Result<String> {
    if event.kind != Kind::Amend
        || !matches!(decision, "accept" | "reject")
        || event.hash.is_empty()
        || !event.data.get("snapshot").is_some_and(Value::is_object)
    {
        return Err(Error::new(
            "confirmation",
            "the confirmation does not name an amendment snapshot and decision",
            "confirm a pending amendment with --accept or --reject",
        ));
    }
    let mut roots = project.meta.roots.clone();
    roots.sort();
    roots.dedup();
    Ok(serde_json::json!({
        "version": 2,
        "project": { "roots": roots, "remote": project.meta.remote },
        "change": change.to_string(),
        "amendment_event": event.hash,
        "event": event,
        "snapshot": sha256_hex(event.data["snapshot"].to_string().as_bytes()),
        "decision": decision,
    })
    .to_string())
}

pub(super) fn amendment_ancestor(events: &[ledger::Event], confirmation: &ledger::Event, amendment: &ledger::Event) -> bool {
    let mut waiting = confirmation.parents.clone();
    let mut seen = std::collections::HashSet::new();
    while let Some(hash) = waiting.pop() {
        if hash == amendment.hash {
            return true;
        }
        if seen.insert(hash.clone())
            && let Some(event) = events.iter().find(|e| e.hash == hash)
        {
            waiting.extend(event.parents.iter().cloned());
        }
    }
    false
}

/// Signs `payload` with `ssh-keygen -Y sign` using the private key at `key`,
/// in the `rvow` namespace. Returns the PEM signature text.
pub fn sign(key: &Path, payload: &str) -> Result<String> {
    let nonce: String = Error::random::<16>()?.iter().map(|b| format!("{b:02x}")).collect();
    let tmp = std::env::temp_dir().join(format!("rvow-sign-{nonce}"));
    std::fs::create_dir(&tmp).map_err(|e| Error::io(&tmp, e))?;
    let msg = tmp.join("msg");
    let result = (|| {
        std::fs::write(&msg, payload).map_err(|e| Error::io(&msg, e))?;
        let st = std::process::Command::new("ssh-keygen")
            .env("SSH_ASKPASS_REQUIRE", "never")
            .args(["-Y", "sign", "-P", "", "-f"])
            .arg(key)
            .args(["-n", "rvow"])
            .arg(&msg)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| Error::new("ssh", format!("cannot run ssh-keygen: {e}"), "install OpenSSH"))?;
        if !st.success() {
            return Err(Error::new("sign", "ssh-keygen -Y sign failed", "check that the key exists and is an ssh signing key"));
        }
        let sig = tmp.join("msg.sig");
        std::fs::read_to_string(&sig).map_err(|e| Error::io(&sig, e))
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// Whether `signature` is a valid `rvow`-namespace signature of `payload` by
/// the signer principal in `allowed_signers`. False on any failure: no
/// signer, no signature, a wrong key or a wrong message.
pub(super) fn signature_ok(allowed_signers: &Path, signer: &str, payload: &str, signature: &str) -> bool {
    if signer.is_empty() || signature.is_empty() {
        return false;
    }
    let Ok(nonce) = Error::random::<16>() else { return false };
    let nonce: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let tmp = std::env::temp_dir().join(format!("rvow-verify-{nonce}"));
    if std::fs::create_dir(&tmp).is_err() {
        return false;
    }
    let sig = tmp.join("sig");
    if std::fs::write(&sig, signature).is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
        return false;
    }
    use std::io::Write;
    let mut child = match std::process::Command::new("ssh-keygen")
        .args(["-Y", "verify", "-I", signer, "-n", "rvow", "-f"])
        .arg(allowed_signers)
        .args(["-s"])
        .arg(&sig)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return false;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.as_bytes());
    }
    let status = child.wait();
    let _ = std::fs::remove_dir_all(&tmp);
    status.is_ok_and(|s| s.success())
}
