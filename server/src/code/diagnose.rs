//! Diagnose / optimise mode, input side: tell an error log from a code
//! snippet, normalise it, and pull out what can be looked up — stack frames
//! (`file:line`), symbol names, literal message fragments and search terms.
//! Pure functions; the lookups live in `resolve`.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use super::Lang;

/// Largest input accepted (pasted logs / snippets).
pub const MAX_INPUT: usize = 64 * 1024;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Error,
    Code,
}

/// A location mentioned in the input (compiler diagnostic, stack frame…).
#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    /// 1-based line of the (formatted) input this frame comes from.
    pub input_line: u32,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub col: Option<u32>,
    pub func: Option<String>,
    /// error | warning | note | exception | frame
    pub kind: &'static str,
    pub message: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct Parsed {
    pub kind: Kind,
    /// Source language (code input, or guessed from the files in an error log).
    pub lang: Option<&'static str>,
    pub formatted: String,
    /// Code input: line numbers stripped from a pasted listing start here (else 1).
    pub first_line: u32,
    pub frames: Vec<Frame>,
    /// Identifiers worth looking up (functions, types, variables in messages).
    pub symbols: Vec<String>,
    /// Literal message fragments that may appear verbatim in the source.
    pub fragments: Vec<String>,
}

static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap());
static LISTING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(\d+)\s*[|│:]\s?").unwrap());

/// Compiler / tool diagnostics and stack frames. Order matters: first match wins per line.
static FRAME_RES: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    [
        // GCC / Clang: file:line:col: error: message
        ("cc", r"^(?P<file>[^\s:][^:]*?):(?P<line>\d+):(?:(?P<col>\d+):)?\s*(?P<kind>fatal error|error|warning|note):\s*(?P<msg>.*)$"),
        // pioasm: file.pio:12.5-10: error: message
        ("pioasm", r"^(?P<file>[^\s:]+\.pio):(?P<line>\d+)\.(?P<col>\d+)(?:-\d+)?:\s*(?P<kind>error|warning):\s*(?P<msg>.*)$"),
        // MSVC: file(line,col): error C1234: message
        ("msvc", r"^(?P<file>.+?)\((?P<line>\d+)(?:,(?P<col>\d+))?\)\s*:\s*(?P<kind>fatal error|error|warning)\s+\w+:\s*(?P<msg>.*)$"),
        // Linker: file.c:(.text+0x10): undefined reference to `x'  /  file.c:42: undefined reference
        ("ld", r"^(?P<file>[^\s:]+\.(?:c|cc|cpp|cxx|o|obj)):(?:\(\.\w+[^)]*\)|(?P<line>\d+)):\s*(?P<msg>.*)$"),
        // Python traceback
        ("py", r#"^\s*File "(?P<file>[^"]+)", line (?P<line>\d+)(?:, in (?P<func>\S+))?"#),
        // Rust
        ("rust", r"panicked at (?P<file>[^:\s]+):(?P<line>\d+):(?P<col>\d+)"),
        ("rustc", r"^\s*--> (?P<file>[^:\s]+):(?P<line>\d+):(?P<col>\d+)"),
        // GDB backtrace: #1  0x1000 in func (args) at file.c:42
        ("gdb", r"^#\d+\s+(?:0x[0-9a-fA-F]+ in )?(?P<func>[\w:~<>]+) \(.*\) at (?P<file>[^:\s]+):(?P<line>\d+)"),
        // JS / Node: at func (file:line:col)
        ("js", r"^\s*at (?:(?P<func>[^\s(]+) \()?(?P<file>[^\s():]+):(?P<line>\d+):(?P<col>\d+)\)?"),
        // CMake
        ("cmake", r"^CMake (?P<kind>Error|Warning)(?: \(dev\))? at (?P<file>[^:\s]+):(?P<line>\d+)"),
        // Assertion: assertion "x" failed: file "f.c", line 12
        ("assert", r#"[Aa]ssert(?:ion)?.*?file "?(?P<file>[^",\s]+)"?,? line (?P<line>\d+)"#),
        // Anything else that looks like path/file.ext:line
        ("generic", r"(?P<file>(?:[\w.\-]+[/\\])*[\w.\-]+\.(?:c|h|cc|cpp|cxx|hpp|hh|cu|cuh|ino|py|pio|rs|go|js|ts|java)):(?P<line>\d+)(?::(?P<col>\d+))?"),
    ]
    .into_iter()
    .map(|(k, r)| (k, Regex::new(r).unwrap()))
    .collect()
});

static IN_FUNCTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?P<file>[^:]+): In (?:member )?function ['‘`](?P<func>[^'’`]+)['’`]:").unwrap());
static PY_EXC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<exc>[A-Za-z_][\w.]*(?:Error|Exception|Warning|Interrupt|Exit|Fault)):\s*(?P<msg>.*)$").unwrap()
});
/// ROS 2 / spdlog style: [ERROR] [1700000000.123] [node]: message
static ROS_LOG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[(?P<lvl>FATAL|ERROR|WARN|INFO|DEBUG)\](?:\s*\[[^\]]*\])*:\s*(?P<msg>.+)$").unwrap());
static QUOTED_IDENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[‘'`\x22](?P<id>[A-Za-z_][\w:]*)(?:\([^)]*\))?['’\x22]").unwrap());
static PY_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:name|attribute|module named) '(?P<id>[\w.]+)'|'(?P<obj>\w+)' object has no attribute '(?P<attr>\w+)'").unwrap()
});
static ERROR_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(error|warning|exception|traceback|panic(?:ked)?|undefined reference|segmentation fault|hardfault|hard fault|assert(?:ion)? .*failed|fatal|abort(?:ed)?|core dumped|sigsegv|sigabrt|backtrace)\b|\[(?:ERROR|FATAL|WARN)\]").unwrap()
});
/// Variable parts of a log message (numbers, hex, paths, quoted values).
static VOLATILE_PART: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"0x[0-9a-fA-F]+|\b\d+(?:\.\d+)*\b|(?:/[\w.\-]+)+|"[^"]*"|'[^']*'|‘[^’]*’|\[[^\]]*\]"#).unwrap()
});

const NOT_SYMBOLS: &[&str] = &[
    "int", "char", "void", "float", "double", "long", "short", "unsigned", "signed", "bool", "const", "struct", "class",
    "return", "static", "auto", "size_t", "uint8_t", "uint16_t", "uint32_t", "uint64_t", "int8_t", "int16_t",
    "int32_t", "int64_t", "std", "None", "True", "False", "self", "this", "main", "error", "warning", "note", "in",
    "and", "or", "not", "for", "while", "if", "else", "Werror", "Wall", "Wextra",
];

fn clean(text: &str) -> String {
    let t = ANSI.replace_all(text, "");
    let t = t.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<&str> = t.lines().map(|l| l.trim_end()).collect();
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }
    lines.join("\n")
}

/// Remove the indentation shared by all non-empty lines.
fn dedent(text: &str) -> String {
    let common = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start_matches([' ', '\t']).len())
        .min()
        .unwrap_or(0);
    text.lines().map(|l| if l.len() >= common { &l[common..] } else { l.trim_start() }).collect::<Vec<_>>().join("\n")
}

/// `12 | code` / `12: code` listings (rustc, gcc caret output, editors): strip
/// the numbers when nearly every line has one; returns the first number.
fn strip_listing(text: &str) -> Option<(String, u32)> {
    let lines: Vec<&str> = text.lines().collect();
    let nonempty = lines.iter().filter(|l| !l.trim().is_empty()).count();
    let caps: Vec<_> = lines.iter().filter_map(|l| LISTING.captures(l)).collect();
    if nonempty < 2 || caps.len() * 10 < nonempty * 8 {
        return None;
    }
    let first: u32 = caps[0][1].parse().ok()?;
    let out = lines.iter().map(|l| LISTING.replace(l, "").into_owned()).collect::<Vec<_>>().join("\n");
    Some((out, first))
}

pub fn guess_lang(code: &str) -> Lang {
    let has = |s: &str| code.contains(s);
    let lines = || code.lines().map(str::trim_start);
    if lines().any(|l| l.starts_with(".program") || l.starts_with(".side_set") || l.starts_with(".wrap"))
        || lines().filter(|l| l.contains(" side ") || l.ends_with(']')).count() >= 2
            && lines().any(|l| ["out ", "in ", "jmp ", "set ", "pull", "push", "wait "].iter().any(|i| l.starts_with(i)))
    {
        return Lang::Pio;
    }
    if lines().any(|l| l.starts_with("def ") || l.starts_with("import ") || l.starts_with("from ") || l.starts_with("class ") && l.ends_with(':'))
        || has("self.") && !has(";")
    {
        return Lang::Python;
    }
    if has("std::") || has("rclcpp") || has("template<") || has("template <") || has("nullptr") || has("__global__")
        || lines().any(|l| l.starts_with("class ") || l.starts_with("namespace ") || l.starts_with("auto "))
    {
        return Lang::Cpp;
    }
    if has("#include") || has(";") && (has("{") || has("(")) {
        return Lang::C;
    }
    Lang::Generic
}

fn lang_of_file(f: &str) -> Option<Lang> {
    Lang::from_name(f).filter(|l| *l != Lang::Generic && *l != Lang::Markdown)
}

fn push_unique(v: &mut Vec<String>, s: &str) {
    if !v.iter().any(|x| x == s) {
        v.push(s.to_string());
    }
}

/// Parse frames out of an error log.
fn frames_of(text: &str) -> Vec<Frame> {
    let mut out: Vec<Frame> = Vec::new();
    let mut func_ctx: Option<String> = None;
    for (i, line) in text.lines().enumerate() {
        let ln = i as u32 + 1;
        if let Some(c) = IN_FUNCTION.captures(line) {
            func_ctx = Some(c["func"].to_string());
            continue;
        }
        for (k, re) in FRAME_RES.iter() {
            let Some(c) = re.captures(line) else { continue };
            let file = c.name("file").map(|m| m.as_str().trim().to_string());
            // URLs and versions are not files.
            if file.as_deref().is_some_and(|f| f.contains("://")) {
                continue;
            }
            let kind = match (*k, c.name("kind").map(|m| m.as_str().to_ascii_lowercase())) {
                (_, Some(k)) if k.contains("error") => "error",
                (_, Some(k)) if k.contains("warning") => "warning",
                (_, Some(k)) if k == "note" => "note",
                ("ld", _) => "error",
                ("rust", _) | ("assert", _) => "error",
                _ => "frame",
            };
            out.push(Frame {
                input_line: ln,
                file,
                line: c.name("line").and_then(|m| m.as_str().parse().ok()),
                col: c.name("col").and_then(|m| m.as_str().parse().ok()),
                func: c.name("func").map(|m| m.as_str().to_string()).or_else(|| func_ctx.clone()),
                kind,
                message: c.name("msg").map(|m| m.as_str().trim().to_string()).filter(|m| !m.is_empty()),
            });
            break;
        }
        if let Some(c) = PY_EXC.captures(line.trim()) {
            out.push(Frame {
                input_line: ln,
                kind: "exception",
                message: Some(format!("{}: {}", &c["exc"], &c["msg"])),
                ..Default::default()
            });
        }
    }
    out
}

pub fn parse(input: &str, force: Option<Kind>, hint: Option<Lang>) -> Parsed {
    let text = clean(input);
    let frames = frames_of(&text);
    let looks_error = !frames.is_empty() || ERROR_WORDS.is_match(&text);
    let kind = force.unwrap_or(if looks_error && hint.is_none() { Kind::Error } else { Kind::Code });

    if kind == Kind::Code {
        let (body, first_line) = match strip_listing(&text) {
            Some((b, n)) => (b, n),
            None => (text.clone(), 1),
        };
        let formatted = dedent(&body);
        let lang = hint.unwrap_or_else(|| guess_lang(&formatted));
        return Parsed {
            kind,
            lang: Some(lang.as_str()),
            formatted,
            first_line,
            frames: Vec::new(),
            symbols: Vec::new(),
            fragments: Vec::new(),
        };
    }

    // Error log: identical consecutive lines collapse (spammy logs).
    let mut lines: Vec<&str> = Vec::new();
    for l in text.lines() {
        if lines.last() != Some(&l) || l.is_empty() {
            lines.push(l);
        }
    }
    let formatted = lines.join("\n");
    let frames = frames_of(&formatted);

    let mut symbols: Vec<String> = Vec::new();
    let mut fragments: Vec<String> = Vec::new();
    for f in &frames {
        if let Some(func) = &f.func {
            let base = func.rsplit("::").next().unwrap_or(func).trim_end_matches("()");
            if base.len() >= 3 && !NOT_SYMBOLS.contains(&base) && !base.starts_with('<') {
                push_unique(&mut symbols, base);
            }
        }
    }
    for line in formatted.lines() {
        for c in QUOTED_IDENT.captures_iter(line) {
            let id = c["id"].rsplit("::").next().unwrap_or(&c["id"]);
            if id.len() >= 3 && !NOT_SYMBOLS.contains(&id) && !id.chars().all(|c| c.is_ascii_digit()) {
                push_unique(&mut symbols, id);
            }
        }
        for c in PY_NAME.captures_iter(line) {
            for k in ["id", "attr"] {
                if let Some(m) = c.name(k) {
                    let id = m.as_str().rsplit('.').next().unwrap_or(m.as_str());
                    if id.len() >= 3 {
                        push_unique(&mut symbols, id);
                    }
                }
            }
        }
    }
    // Message text that may be a string literal in the code: runtime log lines
    // and exception messages (not compiler diagnostics, which come from the tool).
    let compiler_lines: std::collections::HashSet<u32> =
        frames.iter().filter(|f| f.file.is_some()).map(|f| f.input_line).collect();
    for (i, line) in formatted.lines().enumerate() {
        if compiler_lines.contains(&(i as u32 + 1)) {
            continue;
        }
        let msg = if let Some(c) = ROS_LOG.captures(line.trim()) {
            c["msg"].to_string()
        } else if let Some(c) = PY_EXC.captures(line.trim()) {
            c["msg"].to_string()
        } else {
            continue;
        };
        for part in VOLATILE_PART.split(&msg) {
            let p = part.trim().trim_matches(|c: char| c.is_ascii_punctuation() && c != '_').trim();
            if p.chars().count() >= 8 && p.contains(' ') {
                push_unique(&mut fragments, p);
            }
        }
    }
    fragments.sort_by_key(|f| std::cmp::Reverse(f.len()));
    fragments.truncate(6);
    symbols.truncate(12);
    let lang = frames.iter().find_map(|f| f.file.as_deref().and_then(lang_of_file)).map(Lang::as_str);
    Parsed { kind, lang, formatted, first_line: 1, frames, symbols, fragments }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gcc_and_linker_output() {
        let log = "\x1b[1m/home/u/fw/src/led.c: In function ‘main’:\x1b[0m\n/home/u/fw/src/led.c:24:30: warning: suggest parentheses around comparison in operand of ‘&’ [-Wparentheses]\n   24 |     if (pio0_hw->fdebug & 0x4 == 0) {\n/usr/bin/ld: CMakeFiles/fw.dir/src/led.c.obj: in function `main':\nled.c:(.text.main+0x1c): undefined reference to `dma_channel_configure'\ncollect2: error: ld returned 1 exit status\n";
        let p = parse(log, None, None);
        assert_eq!(p.kind, Kind::Error);
        assert!(!p.formatted.contains('\x1b'));
        let f = &p.frames[0];
        assert_eq!((f.file.as_deref(), f.line, f.col, f.kind), (Some("/home/u/fw/src/led.c"), Some(24), Some(30), "warning"));
        assert_eq!(f.func.as_deref(), Some("main"));
        assert!(p.frames.iter().any(|f| f.file.as_deref() == Some("led.c") && f.kind == "error"), "{:?}", p.frames);
        assert!(p.symbols.contains(&"dma_channel_configure".to_string()), "{:?}", p.symbols);
        assert!(!p.symbols.contains(&"main".to_string()));
        assert_eq!(p.lang, Some("c"));
    }

    #[test]
    fn python_traceback_and_ros_log() {
        let log = "Traceback (most recent call last):\n  File \"/ws/src/bot/bot/driver.py\", line 42, in on_cmd\n    self.port.write(frame)\n  File \"/usr/lib/python3/dist-packages/serial/serialposix.py\", line 640, in write\n    raise SerialException('write failed')\nserial.serialutil.SerialException: write failed: [Errno 5] Input/output error\n[ERROR] [1700000000.123456789] [motor_driver]: Failed to open device /dev/ttyACM0 after 3 retries\n";
        let p = parse(log, None, None);
        assert_eq!(p.kind, Kind::Error);
        let files: Vec<_> = p.frames.iter().filter_map(|f| f.file.as_deref()).collect();
        assert_eq!(files, ["/ws/src/bot/bot/driver.py", "/usr/lib/python3/dist-packages/serial/serialposix.py"]);
        assert_eq!(p.frames[0].func.as_deref(), Some("on_cmd"));
        assert!(p.frames.iter().any(|f| f.kind == "exception"));
        assert!(p.symbols.contains(&"on_cmd".to_string()));
        assert!(p.fragments.iter().any(|f| f == "Failed to open device"), "{:?}", p.fragments);
        assert!(p.fragments.iter().any(|f| f.starts_with("write failed")), "{:?}", p.fragments);
        assert_eq!(p.lang, Some("python"));
    }

    #[test]
    fn pioasm_gdb_and_rust() {
        let p = parse("ws2812.pio:11.27-30: error: delay too large (max 15)\n", None, None);
        assert_eq!((p.frames[0].file.as_deref(), p.frames[0].line, p.frames[0].kind), (Some("ws2812.pio"), Some(11), "error"));
        let p = parse("#0  0x10000356 in isr_hardfault ()\n#1  0x100002f0 in dma_handler () at /fw/src/led.c:17\n", None, None);
        assert_eq!(p.frames.len(), 1);
        assert_eq!((p.frames[0].func.as_deref(), p.frames[0].line), (Some("dma_handler"), Some(17)));
        assert!(p.symbols.contains(&"dma_handler".to_string()));
        let p = parse("thread 'main' panicked at src/rag.rs:120:9:\nindex out of bounds\n", None, None);
        assert_eq!((p.frames[0].file.as_deref(), p.frames[0].line), (Some("src/rag.rs"), Some(120)));
    }

    #[test]
    fn code_input_is_formatted() {
        let code = "    for (int i = 0; i < n; i++) {\r\n        sum += a[i];\r\n    }\r\n\r\n";
        let p = parse(code, None, None);
        assert_eq!(p.kind, Kind::Code);
        assert_eq!(p.formatted, "for (int i = 0; i < n; i++) {\n    sum += a[i];\n}");
        assert_eq!(p.lang, Some("c"));
        let listing = "  24 |     if (x & 0x4 == 0) {\n  25 |         y = 1;\n  26 |     }\n";
        let p = parse(listing, Some(Kind::Code), None);
        assert_eq!((p.formatted.as_str(), p.first_line), ("if (x & 0x4 == 0) {\n    y = 1;\n}", 24));
        assert_eq!(guess_lang(".program ws\n.side_set 1\n  out x, 1 side 0 [2]\n"), Lang::Pio);
        assert_eq!(guess_lang("def cb(self, msg):\n    time.sleep(1)\n"), Lang::Python);
        assert_eq!(guess_lang("auto sub = create_subscription<std_msgs::msg::String>(\"t\", 10, cb);"), Lang::Cpp);
        // A code snippet that merely mentions "error" stays code when a language is given.
        let p = parse("printf(\"error: %d\\n\", e);", None, Some(Lang::C));
        assert_eq!(p.kind, Kind::Code);
    }
}
