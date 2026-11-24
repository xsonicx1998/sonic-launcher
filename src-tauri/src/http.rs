//! Small HTTP helpers shared by art (Phase 2) and music (Phase 3).
//!
//! All requests go through `ureq` with rustls (no OpenSSL) and a
//! Sonic Launcher user-agent. Every helper degrades to `Err(String)` —
//! callers decide whether to fall back, never crash the UI.

use std::io::Read;
use std::time::Duration;

const USER_AGENT: &str = "Sonic Launcher/0.1";
const TIMEOUT_SECS: u64 = 30;

/// Percent-encode a URL path segment (RFC 3986 unreserved chars pass
/// through; everything else becomes `%XX`).
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// GET a URL and return the raw response bytes.
pub fn download_bytes(url: &str) -> Result<Vec<u8>, String> {
    download_bytes_with_header(url, None)
}

/// GET a URL with an optional `Authorization` header value.
pub fn download_bytes_with_header(url: &str, auth: Option<&str>) -> Result<Vec<u8>, String> {
    let mut req = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .timeout(Duration::from_secs(TIMEOUT_SECS));
    if let Some(a) = auth {
        req = req.set("Authorization", a);
    }
    let resp = req.call().map_err(|e| format!("GET {url} failed: {e}"))?;
    let mut bytes = Vec::new();
    resp.into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("reading {url} failed: {e}"))?;
    Ok(bytes)
}

/// GET a URL and parse the body as JSON.
pub fn download_json(url: &str, auth: Option<&str>) -> Result<serde_json::Value, String> {
    let bytes = download_bytes_with_header(url, auth)?;
    serde_json::from_slice(&bytes).map_err(|e| format!("parsing JSON from {url} failed: {e}"))
}

/// Outcome of a short, body-capped GET (small JSON lookups such as the
/// Steam store search). The body cap is enforced while reading, so an
/// oversized response can never fill memory.
pub enum CappedGet {
    /// HTTP 200 body, at most `max_bytes` long.
    Ok(Vec<u8>),
    /// HTTP 429 or 403 — the caller should abort the session's
    /// remaining lookups instead of hammering the endpoint.
    RateLimited,
    /// Any other failure (network, timeout, other status, oversize).
    Failed(String),
}

/// Short GET with a `timeout_secs` timeout and a `max_bytes` body cap.
/// Unlike `download_bytes_with_header` this does not read an unbounded
/// body and it distinguishes rate-limiting (429/403) from other
/// failures.
pub fn get_capped(url: &str, timeout_secs: u64, max_bytes: usize) -> CappedGet {
    let resp = match ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .timeout(Duration::from_secs(timeout_secs))
        .call()
    {
        Ok(r) => r,
        Err(ureq::Error::Status(429, _)) | Err(ureq::Error::Status(403, _)) => {
            return CappedGet::RateLimited;
        }
        Err(e) => return CappedGet::Failed(format!("GET {url} failed: {e}")),
    };
    let mut limited = resp.into_reader().take(max_bytes as u64 + 1);
    let mut bytes = Vec::new();
    if let Err(e) = limited.read_to_end(&mut bytes) {
        return CappedGet::Failed(format!("reading {url} failed: {e}"));
    }
    if bytes.len() > max_bytes {
        return CappedGet::Failed(format!("response from {url} exceeds {max_bytes} bytes"));
    }
    CappedGet::Ok(bytes)
}

/// GET a URL and write the body to `dest` (creating parent dirs).
/// Returns `Ok(true)` when a download happened, `Ok(false)` when the
/// destination file already exists and is non-empty (cache hit).
pub fn download_to(url: &str, dest: &std::path::Path) -> Result<bool, String> {
    download_to_with_header(url, dest, None)
}

pub fn download_to_with_header(
    url: &str,
    dest: &std::path::Path,
    auth: Option<&str>,
) -> Result<bool, String> {
    if let Ok(meta) = std::fs::metadata(dest) {
        if meta.len() > 0 {
            return Ok(false);
        }
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let bytes = download_bytes_with_header(url, auth)?;
    std::fs::write(dest, bytes).map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encode_keeps_unreserved_and_escapes_spaces() {
        assert_eq!(url_encode("Team Fortress 2"), "Team%20Fortress%202");
        assert_eq!(url_encode("abc-_.~123"), "abc-_.~123");
        assert_eq!(url_encode("Café"), "Caf%C3%A9");
        assert_eq!(url_encode(""), "");
    }
}
