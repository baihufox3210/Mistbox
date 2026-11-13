//! Duplicate-code detection by winnowing (as in MOSS): tokens are normalised
//! (identifiers → `I`, numbers → `N`, strings → `S`; keywords and operators
//! kept), hashed in k-grams, and a sparse but guaranteed subset of hashes is
//! kept per file. Two places sharing a run of fingerprints along the same
//! diagonal are a clone, even after renaming variables or changing constants.

use serde::Serialize;

use super::Lang;

/// k-gram length in tokens.
pub const K: usize = 24;
/// Winnowing window: any match of ≥ K + W - 1 tokens is guaranteed to share a fingerprint.
pub const W: usize = 8;
/// Smallest clone worth reporting, in tokens (≈ 5–8 lines of C).
pub const MIN_TOKENS: u32 = 50;
const MIN_FPS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tok {
    pub hash: u32,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Fp {
    pub hash: i64,
    /// Index of the k-gram's first token.
    pub tok: u32,
    pub line_start: u32,
    pub line_end: u32,
}

const KEYWORDS: &[&str] = &[
    "if",
    "else",
    "for",
    "while",
    "do",
    "switch",
    "case",
    "default",
    "break",
    "continue",
    "return",
    "goto",
    "struct",
    "union",
    "enum",
    "typedef",
    "class",
    "public",
    "private",
    "protected",
    "template",
    "typename",
    "namespace",
    "const",
    "static",
    "volatile",
    "inline",
    "extern",
    "unsigned",
    "signed",
    "void",
    "int",
    "char",
    "short",
    "long",
    "float",
    "double",
    "bool",
    "auto",
    "sizeof",
    "new",
    "delete",
    "try",
    "catch",
    "throw",
    "def",
    "lambda",
    "in",
    "is",
    "not",
    "and",
    "or",
    "pass",
    "with",
    "yield",
    "import",
    "from",
    "as",
    "elif",
    "except",
    "finally",
    "raise",
    "True",
    "False",
    "None",
    "self",
    "this",
    "nullptr",
    "NULL",
    "true",
    "false",
    "fn",
    "let",
    "mut",
    "match",
    "impl",
    "pub",
    "use",
    "func",
    "var",
    "function",
];

fn fnv(s: &[u8]) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in s {
        h ^= *b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

/// Normalised tokens with their line numbers; comments and whitespace dropped.
pub fn tokenize(lang: Lang, src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let hash_comment = lang == Lang::Python;
    let semi_comment = lang == Lang::Pio;
    let mut out = Vec::new();
    let mut line = 1u32;
    let mut i = 0;
    let push = |out: &mut Vec<Tok>, s: &[u8], line: u32| out.push(Tok { hash: fnv(s), line });
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            i += 1;
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else if (c == b'/' && b.get(i + 1) == Some(&b'/'))
            || (hash_comment && c == b'#')
            || (semi_comment && c == b';')
        {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                if b[i] == b'\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 2;
        } else if c == b'"' || c == b'\'' {
            let start_line = line;
            let triple =
                lang == Lang::Python && b.get(i + 1) == Some(&c) && b.get(i + 2) == Some(&c);
            i += if triple { 3 } else { 1 };
            while i < b.len() {
                if b[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if b[i] == b'\n' {
                    if !triple {
                        break;
                    }
                    line += 1;
                }
                if b[i] == c && (!triple || (b.get(i + 1) == Some(&c) && b.get(i + 2) == Some(&c)))
                {
                    i += if triple { 3 } else { 1 };
                    break;
                }
                i += 1;
            }
            push(&mut out, b"S", start_line);
        } else if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            let s = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] >= 0x80) {
                i += 1;
            }
            let w = &src[s..i];
            if KEYWORDS.contains(&w) {
                push(&mut out, w.as_bytes(), line);
            } else {
                push(&mut out, b"I", line);
            }
        } else if c.is_ascii_digit() {
            while i < b.len()
                && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || b[i] == b'\'' || b[i] == b'_')
            {
                i += 1;
            }
            push(&mut out, b"N", line);
        } else {
            push(&mut out, &b[i..i + 1], line);
            i += 1;
        }
    }
    out
}

fn mix(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

/// Winnowed k-gram fingerprints.
pub fn fingerprints(toks: &[Tok]) -> Vec<Fp> {
    if toks.len() < K {
        return Vec::new();
    }
    let grams: Vec<u64> = (0..=toks.len() - K)
        .map(|i| {
            mix(toks[i..i + K].iter().fold(0u64, |h, t| {
                h.wrapping_mul(1_000_003).wrapping_add(t.hash as u64)
            }))
        })
        .collect();
    let mut out: Vec<Fp> = Vec::new();
    let mut last: Option<usize> = None;
    let windows = grams.len().saturating_sub(W) + 1;
    for w in 0..windows {
        let end = (w + W).min(grams.len());
        // Rightmost minimum in the window.
        let mut m = w;
        for j in w..end {
            if grams[j] <= grams[m] {
                m = j;
            }
        }
        if last != Some(m) {
            last = Some(m);
            out.push(Fp {
                hash: grams[m] as i64,
                tok: m as u32,
                line_start: toks[m].line,
                line_end: toks[m + K - 1].line,
            });
        }
    }
    out
}

/// A shared fingerprint between file A and file B.
#[derive(Debug, Clone, Copy)]
pub struct Match {
    pub a_tok: u32,
    pub a_start: u32,
    pub a_end: u32,
    pub b_tok: u32,
    pub b_start: u32,
    pub b_end: u32,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Region {
    /// Line range in A.
    pub a_start: u32,
    pub a_end: u32,
    /// Line range in B.
    pub b_start: u32,
    pub b_end: u32,
    /// Approximate length of the duplicated run, in tokens.
    pub tokens: u32,
}

/// Chain matches along diagonals into clone regions. With `same_file`, A and B
/// are the same file: self-matches and mirrored / overlapping pairs are dropped.
pub fn regions(ms: Vec<Match>, same_file: bool) -> Vec<Region> {
    regions_with(ms, same_file, MIN_TOKENS, MIN_FPS)
}

/// [`regions`] with explicit thresholds (shorter pasted snippets use lower ones).
pub fn regions_with(
    mut ms: Vec<Match>,
    same_file: bool,
    min_tokens: u32,
    min_fps: u32,
) -> Vec<Region> {
    if same_file {
        ms.retain(|m| m.a_tok < m.b_tok);
    }
    ms.sort_by_key(|m| (m.a_tok, m.b_tok));
    struct Chain {
        first_a: u32,
        first_b: u32,
        last: Match,
        n: u32,
        r: Region,
    }
    let gap = (K + 2 * W) as i64;
    let slack = 8i64;
    let mut chains: Vec<Chain> = Vec::new();
    for m in ms {
        let found = chains.iter_mut().rev().find(|c| {
            let da = m.a_tok as i64 - c.last.a_tok as i64;
            let db = m.b_tok as i64 - c.last.b_tok as i64;
            da > 0 && da <= gap && (db - da).abs() <= slack
        });
        match found {
            Some(c) => {
                c.last = m;
                c.n += 1;
                c.r.a_start = c.r.a_start.min(m.a_start);
                c.r.a_end = c.r.a_end.max(m.a_end);
                c.r.b_start = c.r.b_start.min(m.b_start);
                c.r.b_end = c.r.b_end.max(m.b_end);
            }
            None => chains.push(Chain {
                first_a: m.a_tok,
                first_b: m.b_tok,
                last: m,
                n: 1,
                r: Region {
                    a_start: m.a_start,
                    a_end: m.a_end,
                    b_start: m.b_start,
                    b_end: m.b_end,
                    tokens: 0,
                },
            }),
        }
    }
    let mut out: Vec<Region> = chains
        .into_iter()
        .filter_map(|c| {
            let tokens = c.last.a_tok + K as u32 - c.first_a;
            let overlap = same_file && c.first_b < c.last.a_tok + K as u32;
            (c.n >= min_fps && tokens >= min_tokens && !overlap).then_some(Region { tokens, ..c.r })
        })
        .collect();
    out.sort_by_key(|r| (std::cmp::Reverse(r.tokens), r.a_start));
    out
}

/// Clone regions between two fingerprint sets (used for pasted snippets and tests).
pub fn compare(a: &[Fp], b: &[Fp], same_file: bool) -> Vec<Region> {
    let mut by_hash: std::collections::HashMap<i64, Vec<&Fp>> = std::collections::HashMap::new();
    for f in b {
        by_hash.entry(f.hash).or_default().push(f);
    }
    let mut ms = Vec::new();
    for x in a {
        for y in by_hash.get(&x.hash).into_iter().flatten() {
            if same_file && x.tok == y.tok {
                continue;
            }
            ms.push(Match {
                a_tok: x.tok,
                a_start: x.line_start,
                a_end: x.line_end,
                b_tok: y.tok,
                b_start: y.line_start,
                b_end: y.line_end,
            });
        }
    }
    regions(ms, same_file)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = r#"
// copy one: fill a DMA buffer
static void fill_buffer(uint32_t *buf, int n, uint32_t color) {
    for (int i = 0; i < n; i++) {
        uint32_t r = (color >> 16) & 0xff;
        uint32_t g = (color >> 8) & 0xff;
        uint32_t b = color & 0xff;
        buf[i] = (g << 24) | (r << 16) | (b << 8);
    }
    dma_channel_transfer_from_buffer_now(chan, buf, n);
}
"#;

    #[test]
    fn renamed_copy_is_found_unrelated_is_not() {
        // Same code with different names, constants and comments.
        let b = r#"int unrelated(void) { return 42; }

/* second copy */
void paint(uint32_t *out, int count, uint32_t rgb) {
    for (int k = 0; k < count; k++) {
        uint32_t red = (rgb >> 16) & 0xFF;
        uint32_t green = (rgb >> 8) & 0xFF;
        uint32_t blue = rgb & 0xFF;
        out[k] = (green << 24) | (red << 16) | (blue << 8);
    }
    dma_channel_transfer_from_buffer_now(ch, out, count);
}
"#;
        let fa = fingerprints(&tokenize(Lang::C, A));
        let fb = fingerprints(&tokenize(Lang::C, b));
        let r = compare(&fa, &fb, false);
        assert_eq!(r.len(), 1, "{r:?}");
        assert!(r[0].a_start <= 4 && r[0].a_end >= 9, "{r:?}");
        assert!(r[0].b_start <= 6 && r[0].b_end >= 10, "{r:?}");

        let other =
            "int main(void) {\n  stdio_init_all();\n  while (true) { tight_loop_contents(); }\n}\n";
        assert!(compare(&fa, &fingerprints(&tokenize(Lang::C, other)), false).is_empty());
    }

    #[test]
    fn duplicate_within_one_file() {
        let twice = format!("{A}\n{}", A.replace("fill_buffer", "fill_again"));
        let f = fingerprints(&tokenize(Lang::C, &twice));
        let r = compare(&f, &f, true);
        assert_eq!(r.len(), 1, "{r:?}");
        assert!(r[0].a_end < r[0].b_start, "{r:?}");
    }

    #[test]
    fn comments_and_strings_do_not_matter() {
        let t = tokenize(
            Lang::Python,
            "x = 'a # b'  # comment\ns = \"\"\"multi\nline\"\"\"\ny = 1\n",
        );
        assert_eq!(t.len(), 9);
        assert_eq!(t.last().unwrap().line, 4);
        let p = tokenize(Lang::Pio, "out x, 1 ; c\n  jmp y-- loop // c\n");
        assert_eq!(p.len(), 9);
    }
}
