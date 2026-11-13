//! Note tags: a Markdown note links itself to rules, known errors, symbols or
//! error messages with a front-matter block at the top:
//!
//! ```markdown
//! ---
//! rules: [omp-race, bit-precedence]
//! known: [undefined-reference]
//! symbols: [dma_channel_configure]
//! errors: ["Failed to find a free participant index"]
//! tags: [rp2350, dma]
//! ---
//! # My note …
//! ```
//!
//! Lists may be `[a, b]`, `a, b`, or YAML items (`- a`) on the next lines.
//! `errors` entries are matched as case-insensitive substrings of pasted input,
//! so a note can teach the diagnose mode about errors it does not know yet.

/// (kind, value) with kind ∈ rule | known | symbol | error | tag.
pub type Tag = (&'static str, String);

fn kind_of(key: &str) -> Option<&'static str> {
    Some(match key.trim().to_ascii_lowercase().as_str() {
        "rules" | "rule" | "規則" => "rule",
        "known" | "known_errors" | "常見錯誤" => "known",
        "symbols" | "symbol" | "functions" | "function" | "函式" => "symbol",
        "errors" | "error" | "錯誤" | "錯誤訊息" => "error",
        "tags" | "tag" | "標籤" => "tag",
        _ => return None,
    })
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .or_else(|| s.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
        .unwrap_or(s);
    s.trim().to_string()
}

/// Drop a trailing YAML comment (` # …` outside quotes).
fn strip_comment(v: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut prev_space = true;
    for (i, c) in v.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if c == q => quote = None,
            (None, '#') if prev_space => return v[..i].trim_end(),
            _ => {}
        }
        prev_space = c.is_whitespace();
    }
    v
}

/// Split `[a, "b, c", d]` / `a, b` respecting quotes.
fn items(v: &str) -> Vec<String> {
    let v = v.trim();
    let v = v
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .unwrap_or(v);
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in v.chars() {
        match (quote, c) {
            (None, '"' | '\'') => {
                quote = Some(c);
                cur.push(c);
            }
            (Some(q), _) if c == q => {
                quote = None;
                cur.push(c);
            }
            (None, ',') => {
                out.push(unquote(&cur));
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    out.push(unquote(&cur));
    out.into_iter().filter(|s| !s.is_empty()).collect()
}

/// Tags from the front matter (empty when there is none).
pub fn tags(text: &str) -> Vec<Tag> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return Vec::new();
    }
    let mut out: Vec<Tag> = Vec::new();
    let mut current: Option<&'static str> = None;
    for line in lines.take(60) {
        let t = line.trim_end();
        if t == "---" || t == "..." {
            break;
        }
        if t.trim_start().starts_with('#') {
            continue; // YAML comment
        }
        if let Some(item) = t.trim_start().strip_prefix("- ") {
            if let Some(k) = current {
                out.extend(items(strip_comment(item)).into_iter().map(|v| (k, v)));
            }
            continue;
        }
        let Some((key, value)) = t.split_once(':') else {
            current = None;
            continue;
        };
        current = kind_of(key);
        if let Some(k) = current {
            out.extend(items(strip_comment(value)).into_iter().map(|v| (k, v)));
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|(k, v)| v.chars().count() <= 300 && seen.insert((*k, v.to_lowercase())));
    out.truncate(100);
    out
}

/// 1-based line where the body starts (after the front matter).
pub fn body_start(text: &str) -> u32 {
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return 1;
    }
    for (i, l) in lines.enumerate() {
        if matches!(l.trim_end(), "---" | "...") {
            return i as u32 + 3;
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_forms() {
        let md = "---\nrules: [omp-race, bit-precedence]\nknown: undefined-reference\nsymbols:\n  - dma_channel_configure\n  - \"pio_sm_init\"\nerrors: [\"Failed to find a free participant index\", 'RCLError: context, invalid']\ntitle: ignored\ntags: [rp2350]\n---\n# 標題\n內容\n";
        let t = tags(md);
        let has = |k: &str, v: &str| t.iter().any(|(a, b)| *a == k && b == v);
        assert!(
            has("rule", "omp-race") && has("rule", "bit-precedence"),
            "{t:?}"
        );
        assert!(has("known", "undefined-reference"));
        assert!(
            has("symbol", "dma_channel_configure") && has("symbol", "pio_sm_init"),
            "{t:?}"
        );
        assert!(has("error", "Failed to find a free participant index"));
        assert!(
            has("error", "RCLError: context, invalid"),
            "quoted commas stay: {t:?}"
        );
        assert!(has("tag", "rp2350"));
        assert_eq!(t.len(), 8);
        assert_eq!(body_start(md), 11);
        assert!(tags("# no front matter\nrules: [x]\n").is_empty());
        assert_eq!(body_start("# x\n"), 1);
        let c = tags("---\nrules: [omp-race, bit-precedence]   # 規則代號\nerrors: [\"a # b\"]  # 註解\n---\n");
        assert_eq!(c, vec![("rule", "omp-race".to_string()), ("rule", "bit-precedence".to_string()), ("error", "a # b".to_string())]);
        let zh = "---\n規則: omp-race\n錯誤訊息: [\"段錯誤\"]\n---\n";
        assert_eq!(
            tags(zh),
            vec![
                ("rule", "omp-race".to_string()),
                ("error", "段錯誤".to_string())
            ]
        );
    }
}
