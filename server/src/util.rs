use sha2::{Digest, Sha256};

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn ts_to_rfc3339(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn opt_ts(ts: Option<i64>) -> Option<String> {
    ts.map(ts_to_rfc3339)
}

/// `n` bytes from the OS CSPRNG, hex encoded.
pub fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).expect("OS random number generator unavailable");
    hex::encode(buf)
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(data.as_ref()))
}

/// Constant-time comparison of two strings.
pub fn ct_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

pub fn guess_mime(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    // A few types mime_guess gets wrong or misses for our previews.
    let special = match lower.rsplit_once('.').map(|(_, e)| e) {
        Some("md" | "markdown") => Some("text/markdown"),
        Some("tex" | "sty" | "cls") => Some("text/x-tex"),
        Some("ts" | "tsx") => Some("text/x-typescript"),
        Some("rs") => Some("text/x-rust"),
        Some("log") => Some("text/plain"),
        Some("toml") => Some("application/toml"),
        Some("yaml" | "yml") => Some("application/yaml"),
        _ => None,
    };
    if let Some(m) = special {
        return m.to_string();
    }
    mime_guess::from_path(&lower).first_raw().unwrap_or("application/octet-stream").to_string()
}

/// Build a Content-Disposition value safe for any file name (RFC 6266 / 5987).
pub fn content_disposition(kind: &str, name: &str) -> String {
    use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
    const ATTR: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');
    let fallback: String = name
        .chars()
        .map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' && c != '%' || c == ' ' { c } else { '_' })
        .collect();
    let fallback = if fallback.trim().is_empty() { "download".to_string() } else { fallback };
    format!("{kind}; filename=\"{fallback}\"; filename*=UTF-8''{}", utf8_percent_encode(name, ATTR))
}

/// Query-string boolean accepting `1/0`, `true/false`, `yes/no`, `on/off`.
pub fn flag<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let s: String = serde::Deserialize::deserialize(d)?;
    match s.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(serde::de::Error::custom("expected a boolean")),
    }
}
