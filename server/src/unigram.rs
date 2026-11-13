//! Memory-light Unigram (SentencePiece) encoder for Hugging Face
//! `tokenizer.json` files such as XLM-R / multilingual-e5.
//!
//! The `tokenizers` crate builds a byte trie with a hash map per node, which
//! costs ~300 MB for XLM-R's 250k-piece vocabulary. This encoder reuses that
//! crate's normalizer and pre-tokenizer (so text is prepared identically) but
//! looks pieces up in one flat hash map (~20 MB) and runs the same Viterbi
//! search as `tokenizers::models::unigram::Unigram::encode_optimized`
//! (itself a port of SentencePiece), including unknown-piece fusing. Output
//! ids are verified against the reference tokenizer in tests.

use std::collections::HashMap;

use serde::Deserialize;
use tokenizers::{
    NormalizerWrapper, OffsetReferential, OffsetType, PreTokenizedString, PreTokenizerWrapper,
    tokenizer::{Normalizer, PreTokenizer},
};

/// SentencePiece / HF penalty for unknown characters.
const UNK_PENALTY: f64 = 10.0;

#[derive(Deserialize)]
struct TokenizerJson {
    #[serde(default)]
    added_tokens: Vec<AddedToken>,
    normalizer: Option<NormalizerWrapper>,
    pre_tokenizer: Option<PreTokenizerWrapper>,
    model: ModelJson,
}

#[derive(Deserialize)]
struct AddedToken {
    id: u32,
    content: String,
    #[serde(default)]
    special: bool,
}

#[derive(Deserialize)]
struct ModelJson {
    #[serde(rename = "type")]
    kind: String,
    unk_id: Option<usize>,
    vocab: Vec<(String, f64)>,
    #[serde(default)]
    byte_fallback: bool,
}

pub struct Unigram {
    /// Special tokens matched verbatim in the input before normalisation.
    specials: Vec<(String, u32)>,
    normalizer: Option<NormalizerWrapper>,
    pre_tokenizer: Option<PreTokenizerWrapper>,
    ids: HashMap<Box<str>, u32>,
    scores: Vec<f64>,
    unk_id: u32,
    unk_score: f64,
    max_piece_bytes: usize,
}

#[derive(Clone)]
struct Best {
    id: u32,
    score: f64,
    start: Option<usize>,
}

fn tk_err(e: tokenizers::Error) -> anyhow::Error {
    anyhow::anyhow!("tokenizer: {e}")
}

impl Unigram {
    pub fn from_json(bytes: &[u8]) -> anyhow::Result<Self> {
        let j: TokenizerJson = serde_json::from_slice(bytes)?;
        anyhow::ensure!(j.model.kind == "Unigram", "不是 Unigram 分詞器");
        anyhow::ensure!(!j.model.byte_fallback, "不支援 byte_fallback");
        let unk_id = j
            .model
            .unk_id
            .ok_or_else(|| anyhow::anyhow!("分詞器缺少 unk_id"))?;
        anyhow::ensure!(unk_id < j.model.vocab.len(), "unk_id 超出詞表");
        let mut ids = HashMap::with_capacity(j.model.vocab.len());
        let mut scores = Vec::with_capacity(j.model.vocab.len());
        let mut min_score = f64::INFINITY;
        let mut max_piece_bytes = 1;
        for (id, (piece, score)) in j.model.vocab.into_iter().enumerate() {
            min_score = min_score.min(score);
            max_piece_bytes = max_piece_bytes.max(piece.len());
            scores.push(score);
            // Later duplicates win, as in `tokenizers`.
            ids.insert(piece.into_boxed_str(), id as u32);
        }
        ids.shrink_to_fit();
        let mut specials: Vec<(String, u32)> = j
            .added_tokens
            .into_iter()
            .filter(|t| t.special && !t.content.is_empty())
            .map(|t| (t.content, t.id))
            .collect();
        // Longest first, so the leftmost-longest match wins.
        specials.sort_by_key(|t| std::cmp::Reverse(t.0.len()));
        Ok(Self {
            specials,
            normalizer: j.normalizer,
            pre_tokenizer: j.pre_tokenizer,
            ids,
            scores,
            unk_id: unk_id as u32,
            unk_score: min_score - UNK_PENALTY,
            max_piece_bytes,
        })
    }

    /// Token ids for `text` (no special tokens added). Special tokens written
    /// literally in the text (e.g. `<s>`) are kept as such, like `tokenizers`.
    pub fn encode(&self, text: &str) -> anyhow::Result<Vec<u32>> {
        let mut out = Vec::new();
        let mut seg_start = 0;
        let mut i = 0;
        while i < text.len() {
            if let Some((tok, id)) = self
                .specials
                .iter()
                .find(|(t, _)| text[i..].starts_with(t.as_str()))
            {
                self.encode_segment(&text[seg_start..i], &mut out)?;
                out.push(*id);
                i += tok.len();
                seg_start = i;
            } else {
                i += text[i..].chars().next().map_or(1, char::len_utf8);
            }
        }
        self.encode_segment(&text[seg_start..], &mut out)?;
        Ok(out)
    }

    fn encode_segment(&self, text: &str, out: &mut Vec<u32>) -> anyhow::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let mut pts = PreTokenizedString::from(text);
        if let Some(n) = &self.normalizer {
            pts.normalize(|s| n.normalize(s)).map_err(tk_err)?;
        }
        if let Some(p) = &self.pre_tokenizer {
            p.pre_tokenize(&mut pts).map_err(tk_err)?;
        }
        for (piece, _, _) in pts.get_splits(OffsetReferential::Normalized, OffsetType::None) {
            self.viterbi(piece, out);
        }
        Ok(())
    }

    fn viterbi(&self, s: &str, out: &mut Vec<u32>) {
        let size = s.len();
        if size == 0 {
            return;
        }
        let mut best = vec![
            Best {
                id: 0,
                score: 0.0,
                start: None
            };
            size + 1
        ];
        let relax = |best: &mut [Best], end: usize, id: u32, score: f64, start: usize| {
            let t = &mut best[end];
            if t.start.is_none() || score > t.score {
                *t = Best {
                    id,
                    score,
                    start: Some(start),
                };
            }
        };
        let mut i = 0;
        while i < size {
            let base = best[i].score;
            let mblen = s[i..].chars().next().map_or(1, char::len_utf8);
            let mut has_single = false;
            // Every vocabulary piece that is a prefix of s[i..], shortest first
            // (the order a trie's common-prefix search yields them).
            for (off, c) in s[i..].char_indices() {
                let end = i + off + c.len_utf8();
                if end - i > self.max_piece_bytes {
                    break;
                }
                if let Some(&id) = self.ids.get(&s[i..end]) {
                    relax(&mut best, end, id, base + self.scores[id as usize], i);
                    if end - i == mblen {
                        has_single = true;
                    }
                }
            }
            if !has_single {
                relax(&mut best, i + mblen, self.unk_id, base + self.unk_score, i);
            }
            i += mblen;
        }
        // Backtrack; consecutive unknown pieces are fused into one token.
        let mut rev: Vec<u32> = Vec::new();
        let mut unk_run: Option<(usize, usize)> = None; // byte range of fused unknowns
        let flush = |run: &mut Option<(usize, usize)>, rev: &mut Vec<u32>| {
            if let Some((a, b)) = run.take() {
                rev.push(self.ids.get(&s[a..b]).copied().unwrap_or(self.unk_id));
            }
        };
        let mut end = size;
        while end > 0 {
            let n = &best[end];
            let start = n.start.unwrap_or(0);
            if n.id == self.unk_id {
                unk_run = Some((start, unk_run.map_or(end, |(_, b)| b)));
            } else {
                flush(&mut unk_run, &mut rev);
                rev.push(n.id);
            }
            end = start;
        }
        flush(&mut unk_run, &mut rev);
        out.extend(rev.into_iter().rev());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compare with the reference implementation on a real tokenizer when
    /// `MISTBOX_TEST_TOKENIZER` points at a `tokenizer.json` (and optionally
    /// `MISTBOX_TEST_CORPUS` at a directory of text files).
    #[test]
    fn matches_reference_tokenizer() {
        let Ok(path) = std::env::var("MISTBOX_TEST_TOKENIZER") else {
            eprintln!("MISTBOX_TEST_TOKENIZER not set; skipping");
            return;
        };
        let bytes = std::fs::read(&path).unwrap();
        let ours = Unigram::from_json(&bytes).unwrap();
        let reference = tokenizers::Tokenizer::from_bytes(&bytes).unwrap();
        let mut samples: Vec<String> = [
            "passage: 雲端硬碟的備份與還原流程 Backup and restore.",
            "query: 如何用 curl 上傳檔案？",
            "ＡＢＣ①　全形文字 and émoji 🎉 test\tTab\nnewline  double  space",
            "",
            " ",
            "x",
            "a<s>b</s> <mask>c <pad><unk>",
            "𠀀𠀁 rare CJK ext-B, ﷽, \u{200b}zero-width, \u{0}nul",
            "fn main() { println!(\"{}\", 1 + 2); } // código ñandú Ωμέγα привет مرحبا नमस्ते",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Ok(dir) = std::env::var("MISTBOX_TEST_CORPUS") {
            for e in walkdir::WalkDir::new(dir)
                .into_iter()
                .filter_map(Result::ok)
            {
                if let Ok(t) = std::fs::read_to_string(e.path()) {
                    samples.extend(t.lines().map(str::to_string));
                    samples.push(t.chars().take(3000).collect());
                }
            }
        }
        let mut checked = 0;
        for s in &samples {
            let want: Vec<u32> = reference
                .encode(s.as_str(), false)
                .unwrap()
                .get_ids()
                .to_vec();
            let got = ours.encode(s).unwrap();
            if got != want {
                let i = got.iter().zip(&want).take_while(|(a, b)| a == b).count();
                let ctx = |v: &[u32]| v[i.saturating_sub(3)..(i + 5).min(v.len())].to_vec();
                let pieces = |v: &[u32]| {
                    v.iter()
                        .map(|&id| reference.id_to_token(id).unwrap_or_default())
                        .collect::<Vec<_>>()
                };
                panic!(
                    "mismatch at token {i}: ours {:?} {:?} / reference {:?} {:?}",
                    ctx(&got),
                    pieces(&ctx(&got)),
                    ctx(&want),
                    pieces(&ctx(&want))
                );
            }
            checked += 1;
        }
        eprintln!("unigram: {checked} samples identical to the reference tokenizer");
    }
}
