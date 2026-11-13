//! Encoding-aware text decoding for previews. The stored file is never
//! modified; decoding happens only for display.

use encoding_rs::Encoding;
use serde::Serialize;

#[derive(Serialize, Debug)]
pub struct TextPreview {
    /// Encoding used for this response.
    pub encoding: String,
    /// Encoding detected automatically (BOM / UTF-8 validation / statistical guess).
    pub detected: String,
    /// True if the raw bytes are valid UTF-8.
    pub valid_utf8: bool,
    /// True if decoding produced replacement characters (U+FFFD).
    pub had_errors: bool,
    /// Looks like binary data (NUL bytes) — not suitable for text display.
    pub binary: bool,
    /// Only the first `limit` bytes were decoded.
    pub truncated: bool,
    pub size: u64,
    pub limit: u64,
    pub text: String,
}

/// Encodings offered in the UI.
pub const CHOICES: &[&str] = &[
    "UTF-8", "Big5", "GB18030", "GBK", "Shift_JIS", "EUC-JP", "EUC-KR", "UTF-16LE", "UTF-16BE", "windows-1252", "ISO-8859-1",
];

pub fn looks_binary(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(8192)];
    if head.starts_with(&[0xFF, 0xFE]) || head.starts_with(&[0xFE, 0xFF]) {
        return false;
    }
    head.contains(&0)
}

/// Trim a possibly truncated buffer so we don't cut a UTF-8 sequence in half.
fn trim_partial_utf8(bytes: &[u8]) -> &[u8] {
    match std::str::from_utf8(bytes) {
        Ok(_) => bytes,
        Err(e) if e.error_len().is_none() => &bytes[..e.valid_up_to()],
        Err(_) => bytes,
    }
}

pub fn detect(bytes: &[u8]) -> &'static Encoding {
    if let Some((enc, _)) = Encoding::for_bom(bytes) {
        return enc;
    }
    if std::str::from_utf8(trim_partial_utf8(bytes)).is_ok() {
        return encoding_rs::UTF_8;
    }
    let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    det.feed(bytes, true);
    det.guess(None, chardetng::Utf8Detection::Deny)
}

pub fn decode(bytes: &[u8], requested: Option<&str>, size: u64, limit: u64, truncated: bool) -> Result<TextPreview, String> {
    let bytes = if truncated { trim_partial_utf8(bytes) } else { bytes };
    let detected = detect(bytes);
    let enc = match requested.filter(|r| !r.is_empty() && *r != "auto") {
        Some(label) => Encoding::for_label(label.as_bytes()).ok_or_else(|| format!("不支援的編碼：{label}"))?,
        None => detected,
    };
    let binary = requested.is_none_or(|r| r == "auto" || r.is_empty()) && looks_binary(bytes);
    let (text, used, had_errors) = if binary { (Default::default(), enc, false) } else { enc.decode(bytes) };
    Ok(TextPreview {
        encoding: used.name().to_string(),
        detected: detected.name().to_string(),
        valid_utf8: std::str::from_utf8(bytes).is_ok(),
        had_errors,
        binary,
        truncated,
        size,
        limit,
        text: text.into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_chinese() {
        let p = decode("你好，世界\n\tindent".as_bytes(), None, 0, 0, false).unwrap();
        assert_eq!(p.encoding, "UTF-8");
        assert!(p.valid_utf8 && !p.had_errors && !p.binary);
        assert!(p.text.contains("你好") && p.text.contains('\t'));
    }

    #[test]
    fn big5_detected_and_selectable() {
        let (bytes, _, _) = encoding_rs::BIG5.encode("繁體中文測試文件，這是一段比較長的內容以便偵測編碼。");
        let p = decode(&bytes, None, 0, 0, false).unwrap();
        assert!(!p.valid_utf8);
        let p = decode(&bytes, Some("big5"), 0, 0, false).unwrap();
        assert_eq!(p.encoding, "Big5");
        assert!(p.text.contains("繁體中文"));
        assert!(!p.had_errors);
    }

    #[test]
    fn binary_and_truncation() {
        assert!(decode(&[0x89, b'P', b'N', b'G', 0, 0, 1], None, 0, 0, false).unwrap().binary);
        let s = "中文".as_bytes();
        let p = decode(&s[..4], None, 6, 4, true).unwrap();
        assert_eq!(p.text, "中");
        assert!(!p.had_errors);
    }
}
