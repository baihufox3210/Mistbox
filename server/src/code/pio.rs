//! RP2040 / RP2350 PIO assembly (`.pio`): a small line parser plus rules for
//! the 32-instruction limit, delay / side-set bits, `set` range, labels and
//! autopull / autopush opportunities.

use std::collections::{HashMap, HashSet};

use super::{Finding, Ref, Symbol, rule};

#[derive(Debug, Clone)]
pub struct Instr {
    pub line: u32,
    pub col: u32,
    pub op: String,
    pub args: String,
    pub side: Option<String>,
    pub delay: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Program {
    pub name: String,
    pub line: u32,
    pub line_end: u32,
    /// (bits, opt, line)
    pub side_set: Option<(u32, bool, u32)>,
    pub instrs: Vec<Instr>,
    pub labels: HashMap<String, u32>,
    pub defines: HashSet<String>,
}

#[derive(Debug, Default)]
pub struct Pio {
    pub programs: Vec<Program>,
    pub defines: HashSet<String>,
    /// Lines that look like instructions but are not (line, col, word).
    pub unknown: Vec<(u32, u32, String, String)>,
}

const OPS: &[&str] = &[
    "jmp", "wait", "in", "out", "push", "pull", "mov", "irq", "set", "nop",
];

fn num(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        i64::from_str_radix(h, 16).ok()
    } else if let Some(b) = s.strip_prefix("0b").or_else(|| s.strip_prefix("0B")) {
        i64::from_str_radix(b, 2).ok()
    } else {
        s.parse().ok()
    }
}

fn is_ident(s: &str) -> bool {
    let mut c = s.chars();
    c.next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && c.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn parse(src: &str) -> Pio {
    let mut p = Pio::default();
    let mut in_code_block = false;
    let mut in_comment = false;
    let total = src.lines().count() as u32;
    for (i, raw) in src.lines().enumerate() {
        let ln = i as u32 + 1;
        let trimmed = raw.trim_start();
        if in_code_block {
            if trimmed.starts_with("%}") {
                in_code_block = false;
            }
            continue;
        }
        if trimmed.starts_with('%') {
            in_code_block = !trimmed.contains("%}");
            continue;
        }
        // Strip comments: /* … */ (multi-line), // and ;
        let mut line = String::new();
        let mut rest = raw;
        loop {
            if in_comment {
                match rest.find("*/") {
                    Some(e) => {
                        rest = &rest[e + 2..];
                        in_comment = false;
                    }
                    None => break,
                }
            } else {
                match rest.find("/*") {
                    Some(s) => {
                        line.push_str(&rest[..s]);
                        line.push_str(&" ".repeat(2));
                        rest = &rest[s + 2..];
                        in_comment = true;
                    }
                    None => {
                        line.push_str(rest);
                        break;
                    }
                }
            }
        }
        let cut = [line.find("//"), line.find(';')]
            .into_iter()
            .flatten()
            .min();
        if let Some(c) = cut {
            line.truncate(c);
        }
        let indent = line.len() - line.trim_start().len();
        let col = line[..indent].chars().count() as u32 + 1;
        let mut s = line.trim();
        if s.is_empty() {
            continue;
        }
        if let Some(d) = s.strip_prefix('.') {
            let mut w = d.split_whitespace();
            match w.next().unwrap_or("") {
                "program" => {
                    if let Some(last) = p.programs.last_mut() {
                        last.line_end = ln - 1;
                    }
                    p.programs.push(Program {
                        name: w.next().unwrap_or("?").to_string(),
                        line: ln,
                        line_end: total,
                        side_set: None,
                        instrs: Vec::new(),
                        labels: HashMap::new(),
                        defines: HashSet::new(),
                    });
                }
                "side_set" => {
                    let bits = w.next().and_then(num).unwrap_or(0) as u32;
                    let opt = d.split_whitespace().any(|x| x == "opt");
                    if let Some(pr) = p.programs.last_mut() {
                        pr.side_set = Some((bits, opt, ln));
                    }
                }
                "define" => {
                    let name = w
                        .find(|x| !x.eq_ignore_ascii_case("public"))
                        .unwrap_or("")
                        .to_string();
                    match p.programs.last_mut() {
                        Some(pr) => pr.defines.insert(name),
                        None => p.defines.insert(name),
                    };
                }
                _ => {}
            }
            continue;
        }
        // Labels: `name:` / `public name:` (an instruction may follow).
        if let Some((head, tail)) = s.split_once(':') {
            let head = head.trim();
            let name = match head.split_once(char::is_whitespace) {
                Some((kw, rest)) if kw.eq_ignore_ascii_case("public") => rest.trim(),
                Some(_) => "",
                None => head,
            };
            if is_ident(name) {
                if let Some(pr) = p.programs.last_mut() {
                    pr.labels.insert(name.to_string(), ln);
                }
                s = tail.trim();
                if s.is_empty() {
                    continue;
                }
            }
        }
        let (word, args) = s.split_once(char::is_whitespace).unwrap_or((s, ""));
        let op = word.to_ascii_lowercase();
        if !OPS.contains(&op.as_str()) {
            p.unknown.push((
                ln,
                col,
                word.to_string(),
                p.programs
                    .last()
                    .map(|x| x.name.clone())
                    .unwrap_or_default(),
            ));
            continue;
        }
        let mut args = args.trim().to_string();
        let mut delay = None;
        if let (Some(a), true) = (args.rfind('['), args.ends_with(']')) {
            delay = Some(args[a + 1..args.len() - 1].trim().to_string());
            args.truncate(a);
        }
        let mut side = None;
        let lower = args.to_ascii_lowercase();
        if let Some(i) = lower
            .find(" side ")
            .map(|i| i + 1)
            .or_else(|| lower.starts_with("side ").then_some(0))
        {
            side = Some(args[i + 5..].trim().to_string());
            args.truncate(i);
        }
        let instr = Instr {
            line: ln,
            col,
            op,
            args: args.trim().to_string(),
            side,
            delay,
        };
        match p.programs.last_mut() {
            Some(pr) => pr.instrs.push(instr),
            None => p.unknown.push((ln, col, word.to_string(), String::new())),
        }
    }
    p
}

impl Pio {
    pub fn symbols(&self) -> Vec<Symbol> {
        let mut out = Vec::new();
        for pr in &self.programs {
            out.push(Symbol {
                name: pr.name.clone(),
                kind: "pio_program",
                line_start: pr.line,
                line_end: pr.line_end,
            });
            let mut labels: Vec<_> = pr.labels.iter().collect();
            labels.sort_by_key(|(_, l)| **l);
            for (name, l) in labels {
                out.push(Symbol {
                    name: format!("{}.{name}", pr.name),
                    kind: "label",
                    line_start: *l,
                    line_end: *l,
                });
            }
        }
        out
    }

    pub fn refs(&self) -> Vec<Ref> {
        let mut out = Vec::new();
        for pr in &self.programs {
            for i in pr.instrs.iter().filter(|i| i.op == "jmp") {
                let target = i.args.rsplit([',', ' ']).next().unwrap_or("").trim();
                if is_ident(target) {
                    out.push(Ref {
                        name: format!("{}.{target}", pr.name),
                        line: i.line,
                        in_symbol: Some(pr.name.clone()),
                    });
                }
            }
        }
        out
    }
}

fn finding(id: &'static str, line: u32, col: u32, message: String, program: &str) -> Finding {
    let r = rule(id).unwrap_or_else(|| panic!("unknown rule {id}"));
    Finding {
        rule: r.id,
        severity: r.severity,
        line,
        col,
        line_end: line,
        message,
        symbol: (!program.is_empty()).then(|| program.to_string()),
    }
}

pub fn check(p: &Pio) -> Vec<Finding> {
    let mut out = Vec::new();
    for (l, c, w, prog) in &p.unknown {
        out.push(finding(
            "pio-unknown-instruction",
            *l,
            *c,
            format!("`{w}` 不是 PIO 指令"),
            prog,
        ));
    }
    let total: usize = p.programs.iter().map(|x| x.instrs.len()).sum();
    if p.programs.len() > 1 && total > 32 {
        let first = &p.programs[0];
        out.push(finding(
            "pio-shared-memory",
            first.line,
            1,
            format!(
                "本檔 {} 個程式共 {total} 條指令，同一個 PIO 區塊最多 32 條",
                p.programs.len()
            ),
            &first.name,
        ));
    }
    for pr in &p.programs {
        let name = pr.name.as_str();
        if pr.instrs.len() > 32 {
            out.push(finding(
                "pio-too-long",
                pr.line,
                1,
                format!("`.program {name}` 有 {} 條指令（上限 32）", pr.instrs.len()),
                name,
            ));
        }
        let (bits, opt) = pr.side_set.map(|(b, o, _)| (b, o)).unwrap_or((0, false));
        let used = bits + u32::from(opt && bits > 0);
        let max_delay = if used >= 5 {
            0
        } else {
            (1i64 << (5 - used)) - 1
        };
        for (k, i) in pr.instrs.iter().enumerate() {
            if let Some(d) = i.delay.as_deref().and_then(num)
                && d > max_delay
            {
                let why = if used > 0 {
                    format!("（side-set 用了 {used} 位元）")
                } else {
                    String::new()
                };
                out.push(finding(
                    "pio-delay-overflow",
                    i.line,
                    i.col,
                    format!("延遲 [{d}] 超過上限 {max_delay}{why}"),
                    name,
                ));
            }
            if bits > 0 && !opt && i.side.is_none() {
                out.push(finding(
                    "pio-side-missing",
                    i.line,
                    i.col,
                    format!("`{} {}` 沒有 side 值", i.op, i.args),
                    name,
                ));
            }
            if let Some(v) = i.side.as_deref().and_then(num)
                && bits > 0
                && v >= 1 << bits
            {
                out.push(finding(
                    "pio-side-range",
                    i.line,
                    i.col,
                    format!("side {v} 超出 {bits} 位元（最大 {}）", (1 << bits) - 1),
                    name,
                ));
            }
            if i.op == "set"
                && let Some(v) = i.args.rsplit(',').next().and_then(num)
                && !(0..=31).contains(&v)
            {
                out.push(finding(
                    "pio-set-range",
                    i.line,
                    i.col,
                    format!("`set {}`：{v} 超出 0–31", i.args),
                    name,
                ));
            }
            if i.op == "jmp" {
                let target = i.args.rsplit([',', ' ']).next().unwrap_or("").trim();
                if is_ident(target)
                    && !pr.labels.contains_key(target)
                    && !pr.defines.contains(target)
                    && !p.defines.contains(target)
                {
                    out.push(finding(
                        "pio-undefined-label",
                        i.line,
                        i.col,
                        format!("找不到標籤 `{target}`"),
                        name,
                    ));
                }
            }
            let next = pr.instrs.get(k + 1);
            let a = i.args.to_ascii_lowercase();
            if i.op == "pull"
                && !a.contains("noblock")
                && !a.contains("ifempty")
                && next.is_some_and(|n| n.op == "out")
            {
                out.push(finding(
                    "pio-autopull",
                    i.line,
                    i.col,
                    "`pull` 後緊接 `out`".into(),
                    name,
                ));
            }
            if i.op == "push" && !a.contains("iffull") && k > 0 && pr.instrs[k - 1].op == "in" {
                out.push(finding(
                    "pio-autopush",
                    i.line,
                    i.col,
                    "`in` 後緊接 `push`".into(),
                    name,
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::{Lang, analyze};

    const WS2812: &str = r#"
.program ws2812
.side_set 1

.define public T1 2
.define public T2 5

.wrap_target
bitloop:
    out x, 1       side 0 [T1 - 1] ; side-set still takes place when instruction stalls
    jmp !x do_zero side 1 [T2 - 1] ; Branch on the bit we shifted out
do_one:
    jmp  bitloop   side 1 [4]
do_zero:
    nop            side 0 [16]
    set pins, 40   side 0
    jmp nowhere    side 0
    pull block     side 0
    out pins, 8    side 0
.wrap

% c-sdk {
static inline void ws2812_program_init(PIO pio, uint sm) {
    pio_sm_set_enabled(pio, sm, true);
}
%}

.program noside
    in pins, 8
    push
    mov x, y [31]
    jmp label_ok
label_ok:
    bogus x
"#;

    #[test]
    fn pio_rules_and_symbols() {
        let a = analyze(Lang::Pio, WS2812);
        let r: Vec<_> = a.findings.iter().map(|f| (f.rule, f.line)).collect();
        assert!(r.contains(&("pio-delay-overflow", 15)), "{r:?}"); // max 15 with 1 side-set bit
        assert!(
            !r.iter().any(|x| x.0 == "pio-delay-overflow" && x.1 == 13),
            "{r:?}"
        );
        assert!(r.contains(&("pio-set-range", 16)), "{r:?}");
        assert!(r.contains(&("pio-undefined-label", 17)), "{r:?}");
        assert!(r.contains(&("pio-autopull", 18)), "{r:?}");
        assert!(r.contains(&("pio-autopush", 30)), "{r:?}");
        assert!(r.contains(&("pio-unknown-instruction", 34)), "{r:?}");
        assert!(!r.iter().any(|x| x.0 == "pio-side-missing"), "{r:?}");
        assert!(
            !r.iter().any(|x| x.1 >= 22 && x.1 <= 26),
            "C block ignored: {r:?}"
        );
        let names: Vec<_> = a
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();
        assert!(names.contains(&("ws2812", "pio_program")));
        assert!(names.contains(&("ws2812.bitloop", "label")));
        assert!(names.contains(&("noside", "pio_program")));
        assert!(
            a.refs
                .iter()
                .any(|r| r.name == "ws2812.do_zero" && r.line == 11)
        );

        let missing = ".program p\n.side_set 2\n    set pins, 1 side 1\n    nop\n    nop side 4\n";
        let r: Vec<_> = analyze(Lang::Pio, missing)
            .findings
            .iter()
            .map(|f| (f.rule, f.line))
            .collect();
        assert!(r.contains(&("pio-side-missing", 4)), "{r:?}");
        assert!(r.contains(&("pio-side-range", 5)), "{r:?}");

        let long: String = std::iter::once(".program big\n".to_string())
            .chain((0..33).map(|_| "    nop\n".to_string()))
            .collect();
        let r: Vec<_> = analyze(Lang::Pio, &long)
            .findings
            .iter()
            .map(|f| f.rule)
            .collect();
        assert!(r.contains(&"pio-too-long"));
    }
}
