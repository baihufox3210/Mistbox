//! Diagnose / optimise mode, lookup side: map a parsed input to places in
//! the drive (stack frames, symbol definitions, message strings, clones,
//! similar code, callers / callees), the analysis findings there, known fixes,
//! and related Markdown notes. Everything stays inside `scope` (the caller's
//! root); only analysed files are looked at, so sensitive files never appear.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use sqlx::SqlitePool;

use super::diagnose::{Kind, Parsed};
use super::embed::CodeSemantic;
use super::index::with_subtree;
use super::knowledge::{self, Solution};
use super::{Lang, analyze, clones, rule};

const EXCERPT_BEFORE: u32 = 4;
const EXCERPT_AFTER: u32 = 4;
const MAX_LOCATIONS: usize = 24;
const MAX_NOTES: usize = 8;
const SIMILAR_MIN: f32 = 0.35;

#[derive(Serialize, Debug, Clone)]
pub struct Line {
    pub n: u32,
    pub text: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct Location {
    pub node_id: String,
    /// Line to jump to.
    pub line: u32,
    /// Highlighted range.
    pub line_start: u32,
    pub line_end: u32,
    /// stack | symbol | text | clone | similar | callee | caller
    pub reason: &'static str,
    pub label: String,
    pub score: f32,
    pub excerpt: Vec<Line>,
}

#[derive(Serialize, Debug, Clone)]
pub struct Finding {
    /// `None`: in the pasted snippet.
    pub node_id: Option<String>,
    pub rule: String,
    pub severity: String,
    pub line: u32,
    pub message: String,
    pub symbol: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct Note {
    pub node_id: String,
    pub line: u32,
    pub heading: Option<String>,
    pub excerpt: String,
    pub matched: Vec<String>,
    pub score: f32,
}

#[derive(Serialize, Debug)]
pub struct Report {
    #[serde(flatten)]
    pub parsed: Parsed,
    pub locations: Vec<Location>,
    pub findings: Vec<Finding>,
    pub solutions: Vec<Solution>,
    pub notes: Vec<Note>,
}

pub struct Ctx<'a> {
    pub db: &'a SqlitePool,
    /// The caller's root: results never leave it.
    pub scope: &'a str,
    pub sem: Option<&'a CodeSemantic>,
}

/// Lazily loaded file texts and scope checks.
struct Files<'a> {
    ctx: &'a Ctx<'a>,
    text: HashMap<String, Option<String>>,
    within: HashMap<String, bool>,
}

impl<'a> Files<'a> {
    fn new(ctx: &'a Ctx<'a>) -> Self {
        Self {
            ctx,
            text: HashMap::new(),
            within: HashMap::new(),
        }
    }

    async fn allowed(&mut self, id: &str) -> anyhow::Result<bool> {
        if let Some(b) = self.within.get(id) {
            return Ok(*b);
        }
        let b = crate::nodes::is_within(self.ctx.db, id, self.ctx.scope)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        self.within.insert(id.to_string(), b);
        Ok(b)
    }

    async fn text(&mut self, id: &str) -> anyhow::Result<Option<&str>> {
        if !self.text.contains_key(id) {
            let t: Option<String> =
                sqlx::query_scalar("SELECT body FROM code_text WHERE node_id = ?")
                    .bind(id)
                    .fetch_optional(self.ctx.db)
                    .await?;
            self.text.insert(id.to_string(), t);
        }
        Ok(self.text[id].as_deref())
    }

    async fn excerpt(&mut self, id: &str, from: u32, to: u32) -> anyhow::Result<Vec<Line>> {
        let Some(t) = self.text(id).await? else {
            return Ok(Vec::new());
        };
        let a = from.saturating_sub(EXCERPT_BEFORE).max(1);
        let b = (to + EXCERPT_AFTER).min(from + 24);
        Ok(t.lines()
            .enumerate()
            .skip(a as usize - 1)
            .take((b + 1 - a) as usize)
            .map(|(i, l)| Line {
                n: i as u32 + 1,
                text: l.chars().take(400).collect(),
            })
            .collect())
    }
}

fn phrase(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

/// Files (in scope) whose text contains `term`; `notes` selects Markdown or code.
async fn fts(
    ctx: &Ctx<'_>,
    term: &str,
    notes: bool,
    limit: i64,
) -> anyhow::Result<Vec<(String, String)>> {
    if term.chars().count() < 3 {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(with_subtree!(
        "SELECT t.node_id, t.body FROM code_text t JOIN sub ON sub.id = t.node_id
         WHERE t.body MATCH ?2 AND (t.lang = 'markdown') = ?3 LIMIT ?4"
    ))
    .bind(ctx.scope)
    .bind(phrase(term))
    .bind(notes)
    .bind(limit)
    .fetch_all(ctx.db)
    .await?)
}

/// First line (1-based) containing `term`, case-insensitively.
fn find_line(body: &str, term: &str) -> Option<u32> {
    let t = term.to_lowercase();
    body.lines()
        .position(|l| l.to_lowercase().contains(&t))
        .map(|i| i as u32 + 1)
}

/// How many trailing path segments match (`a/b/led.c` vs `/x/b/led.c` → 2).
fn suffix_score(frame_path: &str, node_path: &str) -> usize {
    let a: Vec<&str> = frame_path
        .split(['/', '\\'])
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    let b: Vec<&str> = node_path.split('/').filter(|s| !s.is_empty()).collect();
    a.iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

struct Out<'a> {
    files: Files<'a>,
    locs: Vec<Location>,
}

impl Out<'_> {
    #[allow(clippy::too_many_arguments)]
    async fn add(
        &mut self,
        node_id: &str,
        line: u32,
        start: u32,
        end: u32,
        reason: &'static str,
        label: String,
        score: f32,
    ) -> anyhow::Result<()> {
        if let Some(l) = self.locs.iter_mut().find(|l| {
            l.node_id == node_id && l.line_start <= line && line <= l.line_end.max(l.line_start)
        }) {
            if score > l.score {
                l.score = score;
                l.reason = reason;
                l.label = label;
            }
            return Ok(());
        }
        let excerpt = self.files.excerpt(node_id, start, end).await?;
        self.locs.push(Location {
            node_id: node_id.to_string(),
            line,
            line_start: start,
            line_end: end,
            reason,
            label,
            score,
            excerpt,
        });
        Ok(())
    }
}

pub async fn resolve(ctx: &Ctx<'_>, parsed: Parsed) -> anyhow::Result<Report> {
    let mut out = Out {
        files: Files::new(ctx),
        locs: Vec::new(),
    };
    let mut findings: Vec<Finding> = Vec::new();
    let mut note_terms: Vec<(String, f32)> = Vec::new();

    match parsed.kind {
        Kind::Error => {
            // 1. Stack frames / compiler diagnostics → files by name, best path suffix.
            let mut seen = HashSet::new();
            for f in parsed
                .frames
                .iter()
                .filter(|f| f.file.is_some() && f.line.is_some())
                .take(12)
            {
                let (file, line) = (f.file.as_deref().unwrap(), f.line.unwrap());
                if !seen.insert((file.to_string(), line)) {
                    continue;
                }
                let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
                let cands: Vec<String> = sqlx::query_scalar(
                    "SELECT c.node_id FROM code_files c JOIN nodes n ON n.id = c.node_id WHERE n.name = ? AND c.status = 'indexed' LIMIT 50",
                )
                .bind(name)
                .fetch_all(ctx.db)
                .await?;
                let mut scored = Vec::new();
                for id in cands {
                    if !out.files.allowed(&id).await? {
                        continue;
                    }
                    let path = crate::nodes::path_of(ctx.db, &id, ctx.scope)
                        .await
                        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                    scored.push((suffix_score(file, &path), id));
                }
                let best = scored.iter().map(|s| s.0).max().unwrap_or(0);
                for (s, id) in scored.into_iter().filter(|s| s.0 == best).take(3) {
                    let label = match (&f.message, f.kind) {
                        (Some(m), _) => m.chars().take(160).collect(),
                        (None, "frame") => format!(
                            "堆疊第 {} 層{}",
                            f.input_line,
                            f.func
                                .as_deref()
                                .map(|x| format!("：{x}"))
                                .unwrap_or_default()
                        ),
                        (None, k) => k.to_string(),
                    };
                    out.add(&id, line, line, line, "stack", label, 3.0 + s as f32 * 0.2)
                        .await?;
                }
            }
            // 2. Symbols named in messages → definitions.
            for sym in parsed.symbols.iter().take(8) {
                for d in super::index::symbols(ctx.db, sym, ctx.scope, 3).await? {
                    let (a, b) = (d.line_start as u32, d.line_end as u32);
                    out.add(
                        &d.node_id,
                        a,
                        a,
                        b.min(a + 12),
                        "symbol",
                        format!("`{}` 的定義（{}）", d.name, d.kind),
                        2.0,
                    )
                    .await?;
                }
                note_terms.push((sym.clone(), 2.0));
            }
            // 3. Runtime message strings → where they are printed / raised.
            for frag in parsed.fragments.iter().take(4) {
                for (id, body) in fts(ctx, frag, false, 10).await? {
                    if let Some(l) = find_line(&body, frag) {
                        out.add(&id, l, l, l, "text", format!("訊息「{frag}」來自這裡"), 2.5)
                            .await?;
                    }
                }
                note_terms.push((frag.clone(), 1.5));
            }
            for f in &parsed.frames {
                if f.kind == "exception"
                    && let Some(exc) = f.message.as_deref().and_then(|m| m.split(':').next())
                {
                    let exc = exc.rsplit('.').next().unwrap_or(exc).to_string();
                    note_terms.push((exc, 1.5));
                }
            }
            // 4. Similar code (error text as a natural-language query).
            if let Some(sem) = ctx.sem {
                let q: String = parsed
                    .frames
                    .iter()
                    .filter_map(|f| f.message.clone())
                    .chain(parsed.fragments.iter().cloned())
                    .collect::<Vec<_>>()
                    .join("\n");
                let q = if q.trim().is_empty() {
                    parsed.formatted.chars().take(600).collect()
                } else {
                    q
                };
                similar(ctx, sem, &q, 4, &mut out).await?;
            }
            // Analysis findings near the located lines.
            let spots: Vec<(String, u32)> = out
                .locs
                .iter()
                .filter(|l| matches!(l.reason, "stack" | "text"))
                .map(|l| (l.node_id.clone(), l.line))
                .collect();
            for (id, line) in spots {
                let rows: Vec<(String, String, i64, String, Option<String>)> = sqlx::query_as(
                    "SELECT rule, severity, line, message, symbol FROM code_findings WHERE node_id = ? AND line BETWEEN ? AND ?",
                )
                .bind(&id)
                .bind(line.saturating_sub(15))
                .bind(line + 15)
                .fetch_all(ctx.db)
                .await?;
                for (r, sev, l, msg, sym) in rows {
                    if !findings.iter().any(|f| {
                        f.node_id.as_deref() == Some(&id) && f.line == l as u32 && f.rule == r
                    }) {
                        findings.push(Finding {
                            node_id: Some(id.clone()),
                            rule: r,
                            severity: sev,
                            line: l as u32,
                            message: msg,
                            symbol: sym,
                        });
                    }
                }
            }
        }
        Kind::Code => {
            let lang = parsed.lang.and_then(Lang::parse).unwrap_or(Lang::Generic);
            let off = parsed.first_line.saturating_sub(1);
            let a = tokio::task::spawn_blocking({
                let text = parsed.formatted.clone();
                move || analyze(lang, &text)
            })
            .await?;
            for f in &a.findings {
                findings.push(Finding {
                    node_id: None,
                    rule: f.rule.to_string(),
                    severity: f.severity.as_str().to_string(),
                    line: f.line + off,
                    message: f.message.clone(),
                    symbol: f.symbol.clone(),
                });
            }
            // 1. Where this code lives (copies, renamed copies).
            let mut by_node: HashMap<String, Vec<clones::Match>> = HashMap::new();
            for fp in &a.fingerprints {
                let rows: Vec<(String, i64, i64, i64)> =
                    sqlx::query_as("SELECT node_id, tok, line_start, line_end FROM code_fp WHERE hash = ? LIMIT 50")
                        .bind(fp.hash)
                        .fetch_all(ctx.db)
                        .await?;
                for (id, tok, s, e) in rows {
                    by_node.entry(id).or_default().push(clones::Match {
                        a_tok: fp.tok,
                        a_start: fp.line_start,
                        a_end: fp.line_end,
                        b_tok: tok as u32,
                        b_start: s as u32,
                        b_end: e as u32,
                    });
                }
            }
            let min_tokens = (a.tokens as u32 * 6 / 10).clamp(clones::K as u32, clones::MIN_TOKENS);
            for (id, ms) in by_node {
                if !out.files.allowed(&id).await? {
                    continue;
                }
                for r in clones::regions_with(ms, false, min_tokens, 2)
                    .into_iter()
                    .take(2)
                {
                    let cover = (r.tokens as f32 / a.tokens.max(1) as f32).min(1.0);
                    out.add(
                        &id,
                        r.b_start,
                        r.b_start,
                        r.b_end,
                        "clone",
                        format!("這段程式碼在這裡（約 {:.0}% 相同結構）", cover * 100.0),
                        3.0 + cover,
                    )
                    .await?;
                }
            }
            // 2. Short snippets: literal lines.
            if !out.locs.iter().any(|l| l.reason == "clone") {
                let mut lines: Vec<&str> = parsed
                    .formatted
                    .lines()
                    .map(str::trim)
                    .filter(|l| {
                        l.chars().count() >= 12 && !l.starts_with("//") && !l.starts_with('#')
                    })
                    .collect();
                lines.sort_by_key(|l| std::cmp::Reverse(l.len()));
                for l in lines.into_iter().take(3) {
                    for (id, body) in fts(ctx, l, false, 5).await? {
                        if let Some(n) = find_line(&body, l) {
                            out.add(&id, n, n, n, "text", "相同的程式碼行".into(), 2.6)
                                .await?;
                        }
                    }
                }
            }
            // 3. Similar code by meaning.
            if let Some(sem) = ctx.sem {
                similar(ctx, sem, &parsed.formatted, 5, &mut out).await?;
            }
            // 4. Related functionality: what it calls, who calls what it defines.
            let defined: HashSet<String> = a
                .symbols
                .iter()
                .map(|s| {
                    s.name
                        .rsplit([':', '.'])
                        .next()
                        .unwrap_or(&s.name)
                        .to_string()
                })
                .collect();
            let mut callees: Vec<String> = Vec::new();
            for r in &a.refs {
                if !defined.contains(&r.name) && r.name.len() >= 3 && !callees.contains(&r.name) {
                    callees.push(r.name.clone());
                }
            }
            for c in callees.iter().take(8) {
                for d in super::index::symbols(ctx.db, c, ctx.scope, 2).await? {
                    let (s, e) = (d.line_start as u32, d.line_end as u32);
                    out.add(
                        &d.node_id,
                        s,
                        s,
                        e.min(s + 12),
                        "callee",
                        format!("呼叫的 `{}`", d.name),
                        1.2,
                    )
                    .await?;
                }
            }
            for name in defined.iter().take(6) {
                let rows: Vec<(String, i64, Option<String>)> = sqlx::query_as(with_subtree!(
                    "SELECT r.node_id, r.line, r.in_symbol FROM code_refs r JOIN sub ON sub.id = r.node_id WHERE r.name = ?2 LIMIT 5"
                ))
                .bind(ctx.scope)
                .bind(name)
                .fetch_all(ctx.db)
                .await?;
                for (id, l, caller) in rows {
                    let l = l as u32;
                    let who = caller
                        .map(|c| format!("`{c}`"))
                        .unwrap_or_else(|| "這裡".into());
                    out.add(&id, l, l, l, "caller", format!("{who} 呼叫 `{name}`"), 1.0)
                        .await?;
                }
                note_terms.push((name.clone(), 2.0));
            }
            for c in callees.iter().take(6) {
                note_terms.push((c.clone(), 1.0));
            }
        }
    }

    // Solutions: known errors, then the rules behind findings.
    let mut solutions = if parsed.kind == Kind::Error {
        knowledge::known(&parsed.formatted)
    } else {
        Vec::new()
    };
    let mut rules: Vec<&str> = findings.iter().map(|f| f.rule.as_str()).collect();
    rules.extend(solutions.iter().filter_map(|s| s.rule));
    let mut seen_rules = HashSet::new();
    for r in rules {
        if !seen_rules.insert(r.to_string()) {
            continue;
        }
        note_terms.push((r.to_string(), 3.0));
        if solutions.iter().any(|s| s.source == "rule" && s.id == r) {
            continue;
        }
        if let Some(rule) = rule(r) {
            solutions.push(Solution {
                source: "rule",
                id: rule.id.into(),
                title: rule.title.into(),
                steps: vec![rule.hint.into()],
                rule: Some(rule.id),
            });
        }
    }

    let known_ids: Vec<String> = solutions
        .iter()
        .filter(|x| x.source == "known")
        .map(|x| x.id.clone())
        .collect();
    let notes = notes(ctx, &note_terms, &known_ids, &parsed.formatted).await?;
    let mut locations = out.locs;
    locations.sort_by(|a, b| b.score.total_cmp(&a.score));
    locations.truncate(MAX_LOCATIONS);
    findings.sort_by_key(|f| (sev_rank(&f.severity), f.node_id.clone(), f.line));
    Ok(Report {
        parsed,
        locations,
        findings,
        solutions,
        notes,
    })
}

fn sev_rank(s: &str) -> u8 {
    match s {
        "error" => 0,
        "warning" => 1,
        "perf" => 2,
        _ => 3,
    }
}

async fn similar(
    ctx: &Ctx<'_>,
    sem: &CodeSemantic,
    query: &str,
    k: usize,
    out: &mut Out<'_>,
) -> anyhow::Result<()> {
    // Scope check per node, cached.
    let ids: HashSet<String> = {
        let rows: Vec<String> = sqlx::query_scalar(with_subtree!(
            "SELECT d.node_id FROM code_vec_docs d JOIN sub ON sub.id = d.node_id"
        ))
        .bind(ctx.scope)
        .fetch_all(ctx.db)
        .await?;
        rows.into_iter().collect()
    };
    let hits = match sem.search(query, k * 2, &|id| ids.contains(id)).await {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!(error = %e, "similar-code search failed");
            return Ok(());
        }
    };
    for h in hits.into_iter().filter(|h| h.score >= SIMILAR_MIN).take(k) {
        let label = match &h.symbol {
            Some(s) => format!("意思相近的程式碼：`{s}`（相似度 {:.2}）", h.score),
            None => format!("意思相近的程式碼（相似度 {:.2}）", h.score),
        };
        out.add(
            &h.node_id,
            h.line_start,
            h.line_start,
            h.line_end.min(h.line_start + 16),
            "similar",
            label,
            1.5 + h.score,
        )
        .await?;
    }
    Ok(())
}

/// Markdown notes mentioning the terms (rule ids, symbols, messages), best first.
/// Notes tagged (front matter) with `kind` = `value`, in scope.
async fn tagged(ctx: &Ctx<'_>, kind: &str, value: &str) -> anyhow::Result<Vec<String>> {
    Ok(sqlx::query_scalar(with_subtree!(
        "SELECT DISTINCT t.node_id FROM note_tags t JOIN sub ON sub.id = t.node_id
         WHERE t.kind = ?2 AND t.value = ?3 COLLATE NOCASE LIMIT 20"
    ))
    .bind(ctx.scope)
    .bind(kind)
    .bind(value)
    .fetch_all(ctx.db)
    .await?)
}

/// Excerpt of a tagged note: its first section after the front matter.
async fn note_intro(ctx: &Ctx<'_>, id: &str) -> anyhow::Result<(u32, Option<String>, String)> {
    let body: String = sqlx::query_scalar("SELECT body FROM code_text WHERE node_id = ?")
        .bind(id)
        .fetch_optional(ctx.db)
        .await?
        .unwrap_or_default();
    let start = super::notes::body_start(&body);
    let first = body
        .lines()
        .enumerate()
        .skip(start as usize - 1)
        .find(|(_, l)| !l.trim().is_empty())
        .map(|(i, _)| i as u32 + 1)
        .unwrap_or(start);
    // A leading title (# …) is the heading; show what follows it.
    let line = if body
        .lines()
        .nth(first as usize - 1)
        .is_some_and(|l| l.trim_start().starts_with('#'))
    {
        first + 1
    } else {
        first
    };
    let (heading, excerpt) = section(&body, line);
    Ok((line, heading, excerpt))
}

/// Notes for the diagnose mode, best first:
/// 1. front-matter tags (`rules`, `known`, `symbols`, and `errors` found in the input),
/// 2. full-text mentions of rule ids, symbols and messages.
async fn notes(
    ctx: &Ctx<'_>,
    terms: &[(String, f32)],
    known: &[String],
    input: &str,
) -> anyhow::Result<Vec<Note>> {
    let mut by_node: HashMap<String, Note> = HashMap::new();
    let mut hits: Vec<(String, f32, String)> = Vec::new();
    for (term, w) in terms {
        let kind = if *w >= 3.0 { "rule" } else { "symbol" };
        for id in tagged(ctx, kind, term).await? {
            hits.push((
                id,
                if kind == "rule" { 6.0 } else { 4.0 },
                format!("標註：{term}"),
            ));
        }
    }
    for k in known {
        for id in tagged(ctx, "known", k).await? {
            hits.push((id, 6.0, format!("標註：{k}")));
        }
    }
    let lower = input.to_lowercase();
    let errs: Vec<(String, String)> = sqlx::query_as(with_subtree!(
        "SELECT t.node_id, t.value FROM note_tags t JOIN sub ON sub.id = t.node_id WHERE t.kind = 'error'"
    ))
    .bind(ctx.scope)
    .fetch_all(ctx.db)
    .await?;
    for (id, v) in errs {
        if v.chars().count() >= 3 && lower.contains(&v.to_lowercase()) {
            hits.push((id, 7.0, format!("標註的錯誤：{v}")));
        }
    }
    for (id, w, label) in hits {
        if !by_node.contains_key(&id) {
            let (line, heading, excerpt) = note_intro(ctx, &id).await?;
            by_node.insert(
                id.clone(),
                Note {
                    node_id: id.clone(),
                    line,
                    heading,
                    excerpt,
                    matched: Vec::new(),
                    score: 0.0,
                },
            );
        }
        let n = by_node.get_mut(&id).unwrap();
        if !n.matched.contains(&label) {
            n.score += w;
            n.matched.push(label);
        }
    }
    let mut seen_terms = HashSet::new();
    for (term, w) in terms.iter().take(20) {
        if !seen_terms.insert(term.to_lowercase()) {
            continue;
        }
        for (id, body) in fts(ctx, term, true, 20).await? {
            let Some(line) = find_line(&body, term) else {
                continue;
            };
            let n = by_node.entry(id.clone()).or_insert_with(|| {
                let (heading, excerpt) = section(&body, line);
                Note {
                    node_id: id.clone(),
                    line,
                    heading,
                    excerpt,
                    matched: Vec::new(),
                    score: 0.0,
                }
            });
            n.score += w;
            n.matched.push(term.clone());
        }
    }
    let mut v: Vec<Note> = by_node.into_values().collect();
    v.sort_by(|a, b| b.score.total_cmp(&a.score));
    v.truncate(MAX_NOTES);
    Ok(v)
}

/// The Markdown section around `line`: nearest heading above it, up to the next
/// heading of the same or a higher level (at most ~18 lines).
pub fn section(body: &str, line: u32) -> (Option<String>, String) {
    let lines: Vec<&str> = body.lines().collect();
    let i = (line as usize)
        .saturating_sub(1)
        .min(lines.len().saturating_sub(1));
    let level = |l: &str| {
        let t = l.trim_start();
        let n = t.chars().take_while(|c| *c == '#').count();
        (n > 0 && n <= 6 && t[n..].starts_with(' ')).then_some(n)
    };
    let h = (0..=i).rev().find(|&k| level(lines[k]).is_some());
    let (start, heading, lvl) = match h {
        Some(k) => (
            k,
            Some(lines[k].trim_start_matches('#').trim().to_string()),
            level(lines[k]).unwrap(),
        ),
        None => (i.saturating_sub(3), None, 7),
    };
    let mut end = start + 1;
    while end < lines.len() && end < start + 18 {
        if level(lines[end]).is_some_and(|l| l <= lvl) {
            break;
        }
        end += 1;
    }
    // Make sure the matched line is inside the excerpt.
    let (start, end) = if i >= end {
        (i.saturating_sub(4), (i + 6).min(lines.len()))
    } else {
        (start, end)
    };
    let body_start = if h == Some(start) { start + 1 } else { start };
    let text: String = lines[body_start..end]
        .join("\n")
        .trim()
        .chars()
        .take(1500)
        .collect();
    (heading, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_suffixes_and_sections() {
        assert_eq!(
            suffix_score("/home/u/fw/src/led.c", "/projects/fw/src/led.c"),
            3
        );
        assert_eq!(suffix_score("led.c", "/a/led.c"), 1);
        assert_eq!(suffix_score("src\\main.c", "/x/src/main.c"), 2);
        assert_eq!(suffix_score("a/main.c", "/b/other.c"), 0);

        let md = "# Notes\nintro\n## DMA\nUse `dma_claim_unused_channel`.\nMore DMA text.\n## PIO\nside-set\n";
        let (h, t) = section(md, 4);
        assert_eq!(h.as_deref(), Some("DMA"));
        assert_eq!(t, "Use `dma_claim_unused_channel`.\nMore DMA text.");
        let (h, _) = section("no headings\njust text\nomp-race here\n", 3);
        assert_eq!(h, None);
        assert_eq!(
            find_line("a\nFailed To Open device x\n", "failed to open"),
            Some(2)
        );
    }
}
