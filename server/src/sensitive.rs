//! Heuristics for files that likely contain secrets. Sensitive files are
//! excluded from imports unless explicitly requested, flagged in the UI, and
//! hidden from API keys that were not granted `allow_sensitive`.

/// Why a file is considered sensitive, or `None`.
pub fn classify_name(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    if lower == ".env" || (lower.starts_with(".env.") && !matches!(&lower[5..], "example" | "sample" | "template" | "dist")) {
        return Some("環境變數檔 (.env)");
    }
    if matches!(lower.as_str(), "id_rsa" | "id_dsa" | "id_ecdsa" | "id_ed25519" | "id_ecdsa_sk" | "id_ed25519_sk") {
        return Some("SSH 私鑰");
    }
    if matches!(lower.as_str(), ".netrc" | ".pgpass" | ".pypirc" | ".npmrc" | ".git-credentials" | ".htpasswd" | "credentials" | "credentials.json" | "service-account.json") {
        return Some("認證資訊檔");
    }
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "pem" | "key" | "p8" | "ppk" | "asc" | "gpg" => Some("私鑰 / 金鑰檔"),
        "p12" | "pfx" | "jks" | "keystore" | "crt" | "cer" | "der" | "csr" => Some("憑證 / 金鑰庫"),
        "kdbx" | "kdb" => Some("密碼資料庫"),
        _ => None,
    }
}

/// Content check on the first bytes of a file (PEM private keys).
pub fn classify_content(head: &[u8]) -> Option<&'static str> {
    // A real PEM block: the header must start a line and be followed by
    // base64 (or encryption headers). Source code that merely mentions the
    // marker inside a string literal does not match.
    let s = String::from_utf8_lossy(head);
    let mut lines = s.lines();
    while let Some(line) = lines.next() {
        let t = line.trim_end();
        if t.starts_with("-----BEGIN ") && t.ends_with("PRIVATE KEY-----") {
            let next = lines.next().unwrap_or("").trim();
            let base64ish = !next.is_empty()
                && next.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='));
            if base64ish || next.starts_with("Proc-Type:") {
                return Some("內容含 PEM 私鑰");
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names() {
        assert!(classify_name(".env").is_some());
        assert!(classify_name(".env.production").is_some());
        assert!(classify_name(".env.example").is_none());
        assert!(classify_name("server.KEY").is_some());
        assert!(classify_name("id_ed25519").is_some());
        assert!(classify_name("id_ed25519.pub").is_none());
        assert!(classify_name("notes.md").is_none());
        assert!(classify_content(b"-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----\n").is_some());
        assert!(classify_content(b"-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\n").is_some());
        // Code that only mentions the markers is not a key.
        assert!(classify_content(br#"if s.contains("-----BEGIN") && s.contains("PRIVATE KEY") {"#).is_none());
        assert!(classify_content(b"write(\"-----BEGIN RSA PRIVATE KEY-----\\nabc\\n\")").is_none());
        assert!(classify_content(b"-----BEGIN RSA PRIVATE KEY-----\n").is_none());
    }
}
