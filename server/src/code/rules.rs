//! Static-analysis rules for C / C++ (incl. CUDA, Pico SDK, ROS 2 rclcpp,
//! OpenMP) and Python (rclpy, multiprocessing). Each rule is a syntactic
//! pattern on the tree-sitter tree; the catalogue is `super::RULES`.

use std::collections::{HashMap, HashSet};

use tree_sitter::{Node, Tree};

use super::syntax::{
    callee_name, col, declarator_name, enclosing_function, line, line_end, named_children, text,
    walk,
};
use super::{Finding, Lang, rule};

/// File-wide facts gathered before the rules run.
#[derive(Default, Debug)]
pub struct Facts {
    /// Includes Pico SDK headers (`pico/…`, `hardware/…`).
    pub pico: bool,
    /// Uses ROS 2 (rclcpp / rclpy).
    pub ros: bool,
    /// Uses threads (pthread, <thread>, OpenMP, <atomic>, <mutex>).
    pub threaded: bool,
    pub multiprocessing: bool,
    pub dma_claim: bool,
    pub sm_claim: bool,
    /// Functions registered as interrupt / timer / alarm handlers.
    pub irq_handlers: HashSet<String>,
    /// Functions passed as ROS callbacks (by name).
    pub callbacks: HashSet<String>,
    /// File-scope variables.
    pub globals: HashMap<String, Global>,
}

#[derive(Debug, Clone)]
pub struct Global {
    pub volatile: bool,
    /// `_Atomic`, `std::atomic<…>`, `atomic_*`, or `const`.
    pub safe: bool,
    pub line: u32,
}

/// Registration call → index of the handler argument.
const HANDLER_ARGS: &[(&str, usize)] = &[
    ("irq_set_exclusive_handler", 1),
    ("irq_add_shared_handler", 1),
    ("gpio_set_irq_enabled_with_callback", 3),
    ("gpio_set_irq_callback", 0),
    ("gpio_add_raw_irq_handler", 1),
    ("gpio_add_raw_irq_handler_masked", 1),
    ("add_repeating_timer_ms", 1),
    ("add_repeating_timer_us", 1),
    ("add_alarm_in_ms", 1),
    ("add_alarm_in_us", 1),
    ("add_alarm_at", 1),
];

const ROS_CB_CREATORS: &[&str] = &[
    "create_subscription",
    "create_generic_subscription",
    "create_wall_timer",
    "create_timer",
    "create_service",
];

const BLOCKING_IN_IRQ: &[&str] = &[
    "sleep_ms",
    "sleep_us",
    "sleep_until",
    "busy_wait_ms",
    "busy_wait_us",
    "busy_wait_us_32",
    "printf",
    "puts",
    "putchar",
    "malloc",
    "calloc",
    "realloc",
    "free",
    "pio_sm_put_blocking",
    "pio_sm_get_blocking",
    "dma_channel_wait_for_finish_blocking",
    "mutex_enter_blocking",
    "sem_acquire_blocking",
    "queue_add_blocking",
    "queue_remove_blocking",
    "uart_read_blocking",
    "uart_write_blocking",
    "i2c_read_blocking",
    "i2c_write_blocking",
    "spi_read_blocking",
    "spi_write_blocking",
];

const DOUBLE_MATH: &[&str] = &[
    "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "sqrt", "pow", "exp", "log", "log10",
    "fabs", "floor", "ceil", "fmod", "round", "hypot",
];

/// Per-file caps for very common, low-severity findings.
const CAPS: &[(&str, usize)] = &[("double-on-fpu32", 5), ("shift-signed-literal", 3)];

#[derive(Debug, Clone, Copy)]
struct Lit {
    value: u128,
    unsigned: bool,
    longs: u8,
}

/// Integer literal (`1`, `0x1Fu`, `1ULL`, `0b101`, `1'000`); `None` for floats / other.
fn int_lit(n: Node, src: &str) -> Option<Lit> {
    if n.kind() != "number_literal" && n.kind() != "integer" {
        return None;
    }
    let raw = text(n, src).replace('\'', "").to_ascii_lowercase();
    let body = raw.trim_end_matches(['u', 'l', 'z']);
    let suffix = &raw[body.len()..];
    let (digits, radix) = if let Some(h) = body.strip_prefix("0x") {
        (h, 16)
    } else if let Some(b) = body.strip_prefix("0b") {
        (b, 2)
    } else if body.len() > 1 && body.starts_with('0') && body.bytes().all(|b| b.is_ascii_digit()) {
        (&body[1..], 8)
    } else {
        (body, 10)
    };
    let value = u128::from_str_radix(digits, radix).ok()?;
    Some(Lit {
        value,
        unsigned: suffix.contains('u'),
        longs: suffix.matches('l').count() as u8,
    })
}

fn is_float_literal(n: Node, src: &str) -> bool {
    if n.kind() != "number_literal" {
        return false;
    }
    let t = text(n, src).to_ascii_lowercase();
    !t.starts_with("0x") && (t.contains('.') || t.contains('e')) && !t.ends_with('f')
}

fn op<'a>(n: Node, src: &'a str) -> &'a str {
    n.child_by_field_name("operator")
        .map(|o| text(o, src))
        .unwrap_or("")
}

fn args<'t>(call: Node<'t>) -> Vec<Node<'t>> {
    call.child_by_field_name("arguments")
        .map(|a| {
            named_children(a)
                .into_iter()
                .filter(|k| k.kind() != "comment")
                .collect()
        })
        .unwrap_or_default()
}

/// Leftmost identifier of `a`, `a.b`, `a->b[c].d`, `*a`.
fn root_ident<'a>(n: Node, src: &'a str) -> Option<&'a str> {
    match n.kind() {
        "identifier" => Some(text(n, src)),
        "field_expression" | "subscript_expression" => n
            .child_by_field_name("argument")
            .and_then(|a| root_ident(a, src)),
        "pointer_expression" | "parenthesized_expression" => {
            n.named_child(0).and_then(|a| root_ident(a, src))
        }
        _ => None,
    }
}

fn plain_name(qualified: &str) -> &str {
    qualified
        .rsplit("::")
        .next()
        .unwrap_or(qualified)
        .rsplit('.')
        .next()
        .unwrap_or(qualified)
}

fn strip_ref(s: &str) -> &str {
    s.trim().trim_start_matches('&').trim()
}

// ------------------------------------------------------------------ facts

pub fn facts(lang: Lang, tree: &Tree, src: &str) -> Facts {
    let mut f = Facts::default();
    walk(tree.root_node(), &mut |n, anc| {
        match n.kind() {
            "preproc_include" => {
                let p = n
                    .child_by_field_name("path")
                    .map(|p| text(p, src))
                    .unwrap_or("");
                if p.contains("pico/") || p.contains("hardware/") || p.contains("pico.h") {
                    f.pico = true;
                }
                if p.contains("rclcpp") {
                    f.ros = true;
                }
                if [
                    "pthread.h",
                    "<thread>",
                    "omp.h",
                    "<atomic>",
                    "<mutex>",
                    "stdatomic.h",
                    "threads.h",
                ]
                .iter()
                .any(|h| p.contains(h))
                {
                    f.threaded = true;
                }
            }
            "preproc_call" => {
                if text(n, src).contains("omp") {
                    f.threaded = true;
                }
            }
            "import_statement" | "import_from_statement" => {
                let t = text(n, src);
                if t.contains("rclpy") {
                    f.ros = true;
                }
                if t.contains("multiprocessing") || t.contains("ProcessPoolExecutor") {
                    f.multiprocessing = true;
                }
            }
            "call_expression" | "call" => {
                let Some(name) = callee_name(n, src) else {
                    return true;
                };
                let a = args(n);
                if let Some((_, i)) = HANDLER_ARGS.iter().find(|(h, _)| *h == name)
                    && let Some(h) = a.get(*i)
                {
                    f.irq_handlers.insert(strip_ref(text(*h, src)).to_string());
                }
                if matches!(
                    name.as_str(),
                    "dma_claim_unused_channel" | "dma_channel_claim" | "dma_claim_mask"
                ) {
                    f.dma_claim = true;
                }
                if name.starts_with("pio_claim")
                    || name == "pio_sm_claim"
                    || name == "pio_claim_unused_sm"
                {
                    f.sm_claim = true;
                }
                if ROS_CB_CREATORS.contains(&name.as_str()) {
                    for x in &a {
                        let x = if x.kind() == "keyword_argument" {
                            x.child_by_field_name("value").unwrap_or(*x)
                        } else {
                            *x
                        };
                        match x.kind() {
                            // rclpy: self.cb / cb
                            "identifier" | "attribute" => {
                                f.callbacks.insert(plain_name(text(x, src)).to_string());
                            }
                            // rclcpp: std::bind(&Node::cb, this, _1) / &Node::cb
                            _ => {
                                let t = text(x, src);
                                for part in t.split(['(', ',', ')', ' ']) {
                                    if let Some(p) = part.trim().strip_prefix('&')
                                        && p.contains("::")
                                    {
                                        f.callbacks.insert(plain_name(p).to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            "declaration"
                if lang != Lang::Python
                    && anc.last().is_some_and(|p| p.kind() == "translation_unit") =>
            {
                let quals: Vec<&str> = named_children(n)
                    .into_iter()
                    .filter(|k| k.kind() == "type_qualifier")
                    .map(|k| text(k, src))
                    .collect();
                let ty = n
                    .child_by_field_name("type")
                    .map(|t| text(t, src))
                    .unwrap_or("");
                let volatile = quals.contains(&"volatile");
                let safe = quals
                    .iter()
                    .any(|q| *q == "_Atomic" || *q == "const" || *q == "constexpr")
                    || ty.contains("atomic")
                    || ty.contains("mutex");
                let mut c = n.walk();
                for d in n.children_by_field_name("declarator", &mut c) {
                    if d.kind() == "function_declarator" {
                        continue;
                    }
                    if let Some(name) = declarator_name(d, src) {
                        f.globals.insert(
                            name,
                            Global {
                                volatile,
                                safe,
                                line: line(n),
                            },
                        );
                    }
                }
            }
            _ => {}
        }
        true
    });
    f
}

// ------------------------------------------------------------------ findings

struct Out<'s> {
    src: &'s str,
    v: Vec<Finding>,
    seen: HashSet<(&'static str, u32, u32)>,
    /// Occurrences per capped rule (including those not reported).
    counts: HashMap<&'static str, usize>,
}

impl Out<'_> {
    fn add(&mut self, id: &'static str, n: Node, anc: &[Node], message: String) {
        let r = rule(id).unwrap_or_else(|| panic!("unknown rule {id}"));
        let (l, c) = (line(n), col(n, self.src));
        if !self.seen.insert((r.id, l, c)) {
            return;
        }
        if let Some((_, cap)) = CAPS.iter().find(|(id, _)| *id == r.id) {
            let n = self.counts.entry(r.id).or_default();
            *n += 1;
            if *n > *cap {
                return;
            }
        }
        self.v.push(Finding {
            rule: r.id,
            severity: r.severity,
            line: l,
            col: c,
            line_end: line_end(n),
            message,
            symbol: enclosing_function(anc, self.src),
        });
    }
}

pub fn check(lang: Lang, tree: &Tree, src: &str, f: &Facts) -> Vec<Finding> {
    let mut o = Out {
        src,
        v: Vec::new(),
        seen: HashSet::new(),
        counts: HashMap::new(),
    };
    match lang {
        Lang::Python => check_python(tree, src, f, &mut o),
        _ => check_c(lang, tree, src, f, &mut o),
    }
    for (id, cap) in CAPS {
        let n = o.counts.get(id).copied().unwrap_or(0);
        if n > *cap
            && let Some(last) = o.v.iter_mut().rev().find(|x| x.rule == *id)
        {
            last.message
                .push_str(&format!("（本檔另有 {} 處相同情形未列出）", n - cap));
        }
    }
    o.v
}

fn in_function_named<'a>(anc: &[Node], src: &'a str) -> Option<&'a str> {
    let fd = anc
        .iter()
        .rev()
        .find(|a| a.kind() == "function_definition")?;
    let d = fd.child_by_field_name("declarator")?;
    // function_declarator → declarator (identifier)
    let mut cur = d;
    loop {
        match cur.kind() {
            "identifier" | "field_identifier" => return Some(text(cur, src)),
            "qualified_identifier" => return Some(plain_name(text(cur, src))),
            _ => cur = cur.child_by_field_name("declarator")?,
        }
    }
}

/// The ROS callback (lambda or function) containing this node, if any.
fn ros_callback<'t>(anc: &[Node<'t>], src: &str, f: &Facts) -> Option<Node<'t>> {
    for a in anc.iter().rev() {
        match a.kind() {
            "lambda_expression" => {
                let call = a
                    .parent()
                    .filter(|p| p.kind() == "argument_list")
                    .and_then(|p| p.parent());
                if call
                    .and_then(|c| callee_name(c, src))
                    .is_some_and(|n| ROS_CB_CREATORS.contains(&n.as_str()))
                {
                    return Some(*a);
                }
            }
            "function_definition" => {
                let name = in_function_named(std::slice::from_ref(a), src)?;
                let lower = name.to_ascii_lowercase();
                return (f.callbacks.contains(name)
                    || lower.ends_with("callback")
                    || lower.ends_with("_cb"))
                .then_some(*a);
            }
            _ => {}
        }
    }
    None
}

fn in_loop(anc: &[Node]) -> bool {
    for a in anc.iter().rev() {
        match a.kind() {
            "for_statement" | "while_statement" | "do_statement" | "for_range_loop" => return true,
            "function_definition" | "lambda_expression" => return false,
            _ => {}
        }
    }
    false
}

fn empty_body(b: Node) -> bool {
    match b.kind() {
        "expression_statement" => b.named_child_count() == 0,
        "compound_statement" => named_children(b).iter().all(|k| k.kind() == "comment"),
        _ => false,
    }
}

fn check_c(lang: Lang, tree: &Tree, src: &str, f: &Facts, o: &mut Out) {
    let mut irq_reported: HashSet<(String, String)> = HashSet::new();
    let mut flagged_loops: HashSet<usize> = HashSet::new();
    walk(tree.root_node(), &mut |n, anc| {
        let in_irq =
            || in_function_named(anc, src).is_some_and(|name| f.irq_handlers.contains(name));
        match n.kind() {
            "binary_expression" => binary(n, anc, src, f, o),
            "field_declaration" => {
                let has_bits = named_children(n)
                    .iter()
                    .any(|k| k.kind() == "bitfield_clause");
                if has_bits {
                    let ty = n.child_by_field_name("type");
                    if lang == Lang::C
                        && ty.is_some_and(|t| {
                            t.kind() == "primitive_type"
                                && matches!(text(t, src), "int" | "char" | "short" | "long")
                        })
                    {
                        o.add(
                            "bitfield-plain-int",
                            n,
                            anc,
                            format!(
                                "`{}` 的正負號由編譯器決定",
                                text(n, src).trim_end_matches(';')
                            ),
                        );
                    }
                    if named_children(n)
                        .iter()
                        .any(|k| k.kind() == "type_qualifier" && text(*k, src) == "volatile")
                    {
                        o.add(
                            "bitfield-volatile-register",
                            n,
                            anc,
                            "volatile 位元欄位：寫入會對整個暫存器做讀-改-寫".into(),
                        );
                    }
                }
            }
            "struct_specifier" | "union_specifier" => {
                let has_bits = n.child_by_field_name("body").is_some_and(|b| {
                    named_children(b).iter().any(|fd| {
                        named_children(*fd)
                            .iter()
                            .any(|k| k.kind() == "bitfield_clause")
                    })
                });
                let volatile_use = n.parent().is_some_and(|p| {
                    matches!(
                        p.kind(),
                        "declaration" | "type_definition" | "field_declaration"
                    ) && named_children(p)
                        .iter()
                        .any(|k| k.kind() == "type_qualifier" && text(*k, src) == "volatile")
                });
                if has_bits && volatile_use {
                    o.add(
                        "bitfield-volatile-register",
                        n,
                        anc,
                        "以 volatile 位元欄位結構對應暫存器".into(),
                    );
                }
            }
            "assignment_expression" | "update_expression" => {
                let target = if n.kind() == "assignment_expression" {
                    n.child_by_field_name("left")
                } else {
                    n.child_by_field_name("argument")
                };
                let Some(target) = target else { return true };
                if n.kind() == "assignment_expression" {
                    let op = op(n, src);
                    if matches!(op, "|=" | "&=" | "^=")
                        && target.kind() == "field_expression"
                        && let Some(base) = root_ident(target, src)
                        && (base.ends_with("_hw") || (f.pico && base == "pio"))
                    {
                        let lhs = text(target, src);
                        let rhs = n.child_by_field_name("right");
                        let rhs_t = rhs.map(|r| text(r, src)).unwrap_or("mask");
                        let fix = match op {
                            "|=" => format!("hw_set_bits(&{lhs}, {rhs_t})"),
                            "^=" => format!("hw_xor_bits(&{lhs}, {rhs_t})"),
                            _ => {
                                let m = rhs
                                    .filter(|r| {
                                        r.kind() == "unary_expression" && op_text(*r, src) == "~"
                                    })
                                    .and_then(|r| r.child_by_field_name("argument"))
                                    .map(|a| text(a, src).to_string())
                                    .unwrap_or_else(|| format!("~({rhs_t})"));
                                format!("hw_clear_bits(&{lhs}, {m})")
                            }
                        };
                        o.add(
                            "rmw-register",
                            n,
                            anc,
                            format!("`{}` 不是原子操作；建議 `{fix}`", text(n, src)),
                        );
                    }
                }
                if in_irq()
                    && let Some(var) = root_ident(target, src)
                    && let Some(g) = f.globals.get(var)
                {
                    let handler = in_function_named(anc, src).unwrap_or("").to_string();
                    if !g.volatile
                        && !g.safe
                        && irq_reported.insert((handler.clone(), var.to_string()))
                    {
                        o.add(
                                    "irq-shared-nonvolatile",
                                    n,
                                    anc,
                                    format!("中斷處理函式 `{handler}` 修改全域變數 `{var}`（第 {} 行宣告），但它不是 volatile 或原子型別", g.line),
                                );
                    }
                }
            }
            "call_expression" => call_c(n, anc, src, f, o, in_irq()),
            "while_statement" => {
                let cond = n
                    .child_by_field_name("condition")
                    .map(|c| text(c, src))
                    .unwrap_or("");
                if n.child_by_field_name("body").is_some_and(empty_body) {
                    if cond.contains("dma_channel_is_busy") {
                        o.add("busy-wait-dma", n, anc, "空迴圈輪詢 DMA 狀態".into());
                    } else if cond.contains("pio_sm_is_tx_fifo_full")
                        || cond.contains("pio_sm_is_rx_fifo_empty")
                    {
                        o.add("busy-wait-fifo", n, anc, "空迴圈輪詢 PIO FIFO".into());
                    }
                }
            }
            "preproc_call" => {
                let arg = n
                    .child_by_field_name("argument")
                    .map(|a| text(a, src).trim())
                    .unwrap_or("");
                if text(n, src).trim_start().starts_with("#pragma") && arg.starts_with("omp") {
                    omp(n, arg, anc, src, o);
                }
            }
            "number_literal" if f.pico && is_float_literal(n, src) => {
                if n.parent().is_some_and(|p| p.kind() == "binary_expression") {
                    let t = text(n, src);
                    o.add(
                        "double-on-fpu32",
                        n,
                        anc,
                        format!("`{t}` 是 double 常數，整個運算會走雙精度；改寫成 `{t}f`"),
                    );
                }
            }
            "declaration"
                if f.threaded
                    && !f.pico
                    && anc.last().is_some_and(|p| p.kind() == "translation_unit") =>
            {
                if named_children(n)
                    .iter()
                    .any(|k| k.kind() == "type_qualifier" && text(*k, src) == "volatile")
                {
                    o.add(
                        "volatile-as-sync",
                        n,
                        anc,
                        format!(
                            "`{}`：volatile 不能取代原子操作或鎖",
                            text(n, src).trim_end_matches(';')
                        ),
                    );
                }
            }
            "lambda_expression" | "function_definition" if f.ros => {
                let is_cb =
                    ros_callback(&[anc, &[n]].concat(), src, f).is_some_and(|c| c.id() == n.id());
                if is_cb {
                    let params = if n.kind() == "lambda_expression" {
                        n.child_by_field_name("declarator")
                            .and_then(|d| d.child_by_field_name("parameters"))
                    } else {
                        n.child_by_field_name("declarator")
                            .and_then(|d| d.child_by_field_name("parameters"))
                    };
                    for p in params.map(named_children).unwrap_or_default() {
                        if p.kind() != "parameter_declaration" {
                            continue;
                        }
                        let ty = p
                            .child_by_field_name("type")
                            .map(|t| text(t, src))
                            .unwrap_or("");
                        let by_value = p
                            .child_by_field_name("declarator")
                            .is_some_and(|d| d.kind() == "identifier");
                        if ty.contains("msg::")
                            && by_value
                            && !ty.contains("Ptr")
                            && !ty.contains("ptr")
                        {
                            o.add(
                                "ros2-msg-by-value",
                                p,
                                anc,
                                format!("`{}` 每則訊息都會複製一次", text(p, src)),
                            );
                        }
                    }
                }
            }
            _ => {}
        }
        if matches!(
            n.kind(),
            "for_statement" | "while_statement" | "do_statement" | "for_range_loop"
        ) && !anc.iter().any(|a| flagged_loops.contains(&a.id()))
        {
            let body = n
                .child_by_field_name("body")
                .map(|b| text(b, src))
                .unwrap_or("");
            let walks_buffer = body.contains('[') || body.contains("++)") || body.contains("*p");
            if (body.contains("pio_sm_put_blocking") || body.contains("pio_sm_get_blocking"))
                && walks_buffer
            {
                flagged_loops.insert(n.id());
                o.add(
                    "pio-cpu-loop",
                    n,
                    anc,
                    "迴圈中逐字經由 CPU 存取 PIO FIFO".into(),
                );
            }
        }
        true
    });
}

fn op_text<'a>(n: Node, src: &'a str) -> &'a str {
    n.child_by_field_name("operator")
        .map(|o| text(o, src))
        .unwrap_or("")
}

const CMP: &[&str] = &["==", "!=", "<", ">", "<=", ">="];

/// `cout << "x" << n - 1`: an overloaded stream `<<`, not a bit shift.
fn stream_shift(n: Node, src: &str) -> bool {
    let mut top = n;
    while let Some(p) = top.parent() {
        if p.kind() == "binary_expression"
            && op_text(p, src) == "<<"
            && p.child_by_field_name("left")
                .is_some_and(|l| l.id() == top.id())
        {
            top = p;
        } else {
            break;
        }
    }
    let mut cur = Some(top);
    while let Some(c) = cur {
        let operands: Vec<Node> = if c.kind() == "binary_expression" && op_text(c, src) == "<<" {
            c.child_by_field_name("right").into_iter().collect()
        } else {
            vec![c]
        };
        for x in operands {
            if matches!(
                x.kind(),
                "string_literal" | "raw_string_literal" | "char_literal" | "concatenated_string"
            ) {
                return true;
            }
            let t = text(x, src).to_ascii_lowercase();
            if [
                "cout",
                "cerr",
                "clog",
                "stream",
                "debug",
                "log",
                "std::endl",
                "endl",
            ]
            .iter()
            .any(|w| t.contains(w))
            {
                return true;
            }
        }
        cur = (c.kind() == "binary_expression" && op_text(c, src) == "<<")
            .then(|| c.child_by_field_name("left"))
            .flatten();
    }
    false
}

fn binary(n: Node, anc: &[Node], src: &str, f: &Facts, o: &mut Out) {
    let op = op(n, src);
    let l = n.child_by_field_name("left");
    let r = n.child_by_field_name("right");
    match op {
        "&" | "|" | "^" => {
            for c in [l, r].into_iter().flatten() {
                if c.kind() == "binary_expression" && CMP.contains(&op_text(c, src)) {
                    o.add(
                        "bit-precedence",
                        n,
                        anc,
                        format!("`{}` 會先算 `{}`，再做 `{op}`", text(n, src), text(c, src)),
                    );
                    break;
                }
            }
        }
        "<<" | ">>" => {
            if op == "<<" && stream_shift(n, src) {
                return;
            }
            if let Some(r) = r.filter(|r| {
                r.kind() == "binary_expression" && matches!(op_text(*r, src), "+" | "-")
            }) {
                o.add(
                    "shift-precedence",
                    n,
                    anc,
                    format!(
                        "`{}` 等於 `{} {op} ({})`",
                        text(n, src),
                        l.map(|l| text(l, src)).unwrap_or(""),
                        text(r, src)
                    ),
                );
            }
            let ll = l.and_then(|l| int_lit(l, src));
            let rl = r.and_then(|r| int_lit(r, src));
            match (ll, rl) {
                (Some(a), Some(b)) => {
                    let width: u128 = match a.longs {
                        0 => 32,
                        1 if f.pico => 32,
                        _ => 64,
                    };
                    if b.value >= width {
                        o.add(
                            "shift-overflow",
                            n,
                            anc,
                            format!(
                                "`{}`：位移量 {} ≥ 型別寬度 {width} 位元",
                                text(n, src),
                                b.value
                            ),
                        );
                    } else if op == "<<" && !a.unsigned && a.value != 0 {
                        let bits = 128 - a.value.leading_zeros() as u128;
                        if bits + b.value > width - 1 {
                            let lt = l.map(|l| text(l, src)).unwrap_or("1");
                            o.add(
                                "shift-overflow",
                                n,
                                anc,
                                format!("`{}` 會移進有號整數的符號位元（未定義行為）；請寫 `{lt}u << {}`", text(n, src), b.value),
                            );
                        }
                    }
                }
                (Some(a), None) if op == "<<" && f.pico && !a.unsigned && a.longs == 0 => {
                    o.add(
                        "shift-signed-literal",
                        n,
                        anc,
                        format!(
                            "`{}` 用的是有號常數；建議 `{}u << …`",
                            text(n, src),
                            text(l.unwrap(), src)
                        ),
                    );
                }
                _ => {}
            }
        }
        "==" | "!=" => {
            for (a, b) in [(l, r), (r, l)] {
                let (Some(a), Some(b)) = (a, b) else { continue };
                let one = int_lit(b, src).is_some_and(|x| x.value == 1);
                let inner = (a.kind() == "parenthesized_expression")
                    .then(|| a.named_child(0))
                    .flatten();
                if let Some(inner) =
                    inner.filter(|i| i.kind() == "binary_expression" && op_text(*i, src) == "&")
                {
                    // Only when the mask is known not to include bit 0:
                    // an even literal, or a single-bit shift `1 << n` (n ≠ 0).
                    let mask_misses_bit0 = |m: Node| -> bool {
                        let m = if m.kind() == "parenthesized_expression" {
                            m.named_child(0).unwrap_or(m)
                        } else {
                            m
                        };
                        if let Some(x) = int_lit(m, src) {
                            return x.value & 1 == 0;
                        }
                        m.kind() == "binary_expression"
                            && op_text(m, src) == "<<"
                            && m.child_by_field_name("left")
                                .and_then(|l| int_lit(l, src))
                                .is_some_and(|x| x.value == 1)
                            && !m
                                .child_by_field_name("right")
                                .and_then(|r| int_lit(r, src))
                                .is_some_and(|x| x.value == 0)
                    };
                    let bad = [
                        inner.child_by_field_name("left"),
                        inner.child_by_field_name("right"),
                    ]
                    .into_iter()
                    .flatten()
                    .any(mask_misses_bit0);
                    if one && bad {
                        o.add(
                            "mask-compare-one",
                            n,
                            anc,
                            format!("`{}`：遮罩後的值是 0 或遮罩本身，不會等於 1", text(n, src)),
                        );
                        break;
                    }
                }
            }
        }
        "&&" | "||" => {
            for c in [l, r].into_iter().flatten() {
                let t = text(c, src).to_ascii_lowercase();
                if t.starts_with("0x") && int_lit(c, src).is_some_and(|x| x.value > 1) {
                    let want = if op == "&&" { "&" } else { "|" };
                    o.add(
                        "logical-with-mask",
                        n,
                        anc,
                        format!("`{}`：是否想寫 `{want}`？", text(n, src)),
                    );
                    break;
                }
            }
        }
        _ => {}
    }
}

fn call_c(n: Node, anc: &[Node], src: &str, f: &Facts, o: &mut Out, in_irq: bool) {
    let Some(name) = callee_name(n, src) else {
        return;
    };
    let a = args(n);
    let lit = |i: usize| a.get(i).is_some_and(|x| x.kind() == "number_literal");
    if !f.dma_claim
        && name.starts_with("dma_channel_")
        && !matches!(
            name.as_str(),
            "dma_channel_claim" | "dma_channel_unclaim" | "dma_channel_is_claimed"
        )
        && lit(0)
    {
        o.add(
            "dma-hardcoded-channel",
            n,
            anc,
            format!("`{name}` 使用固定通道 {}", text(a[0], src)),
        );
    }
    if !f.sm_claim && name.starts_with("pio_sm_") && lit(1) {
        o.add(
            "pio-hardcoded-sm",
            n,
            anc,
            format!("`{name}` 使用固定狀態機 {}", text(a[1], src)),
        );
    }
    if in_irq && BLOCKING_IN_IRQ.contains(&name.as_str()) {
        let h = in_function_named(anc, src).unwrap_or("");
        o.add(
            "irq-blocking-call",
            n,
            anc,
            format!("中斷處理函式 `{h}` 中呼叫 `{name}`"),
        );
    }
    if f.pico && DOUBLE_MATH.contains(&name.as_str()) {
        o.add(
            "double-on-fpu32",
            n,
            anc,
            format!("`{name}` 是雙精度版本；單精度請用 `{name}f`"),
        );
    }
    let negative = |x: Node| text(x, src).trim_start().starts_with('-');
    if name == "pow"
        && a.len() == 2
        && !negative(a[1])
        && (int_lit(a[0], src).is_some_and(|x| x.value == 2)
            || int_lit(a[1], src).is_some_and(|x| x.value == 2))
    {
        o.add("pow-integer", n, anc, format!("`{}`", text(n, src)));
    }
    if name == "strlen" {
        // Inside the condition of the nearest loop?
        if let Some(i) = anc
            .iter()
            .rposition(|x| matches!(x.kind(), "for_statement" | "while_statement"))
        {
            let cond = anc[i].child_by_field_name("condition");
            if cond.is_some_and(|c| anc[i + 1..].iter().any(|x| x.id() == c.id())) {
                o.add(
                    "strlen-in-loop",
                    n,
                    anc,
                    format!("`{}` 每次迭代都重新計算", text(n, src)),
                );
            }
        }
    }
    if name.starts_with("cudaMemcpy") && !name.contains("Async") && in_loop(anc) {
        o.add(
            "cuda-memcpy-in-loop",
            n,
            anc,
            format!("迴圈中呼叫 `{name}`"),
        );
    }
    if name == "__syncthreads" {
        let fi = anc
            .iter()
            .rposition(|x| x.kind() == "function_definition")
            .unwrap_or(0);
        if let Some(cond) = anc[fi..]
            .iter()
            .rev()
            .find(|x| x.kind() == "if_statement")
            .and_then(|x| x.child_by_field_name("condition"))
        {
            let c = text(cond, src);
            if c.contains("threadIdx") || c.contains("tid") || c.contains("lane") {
                o.add(
                    "cuda-syncthreads-divergent",
                    n,
                    anc,
                    format!("`__syncthreads()` 位於依執行緒而異的條件 `{c}` 之內"),
                );
            }
        }
    }
    if f.ros
        && let Some(cb) = ros_callback(anc, src, f)
    {
        match name.as_str() {
            "sleep_for" | "sleep_until" | "usleep" | "sleep" | "nanosleep" => {
                o.add(
                    "ros2-sleep-in-callback",
                    n,
                    anc,
                    format!(
                        "callback 中呼叫 `{}`",
                        text(n.child_by_field_name("function").unwrap_or(n), src)
                    ),
                );
            }
            "spin_until_future_complete" | "spin_some" | "spin_all" | "spin" | "spin_once" => {
                o.add(
                    "ros2-spin-in-callback",
                    n,
                    anc,
                    format!("callback 中呼叫 `{name}`"),
                );
            }
            "create_publisher"
            | "create_subscription"
            | "create_client"
            | "create_service"
            | "create_wall_timer" => {
                o.add(
                    "ros2-create-in-callback",
                    n,
                    anc,
                    format!("callback 中呼叫 `{name}`"),
                );
            }
            "get" | "wait" | "wait_for" => {
                let recv = n
                    .child_by_field_name("function")
                    .filter(|x| x.kind() == "field_expression")
                    .and_then(|x| x.child_by_field_name("argument"));
                if let Some(recv) = recv {
                    let rt = text(recv, src);
                    let from_request = rt.contains("async_send_request")
                        || text(cb, src)
                            .lines()
                            .any(|l| l.contains(rt) && l.contains("async_send_request"));
                    if from_request {
                        o.add(
                            "ros2-future-get-in-callback",
                            n,
                            anc,
                            format!("callback 中對服務請求的 future 呼叫 `.{name}()`"),
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

/// `#pragma omp …` followed by a loop / block: races, critical in loop, false sharing.
fn omp(pragma: Node, arg: &str, anc: &[Node], src: &str, o: &mut Out) {
    if !(arg.contains("parallel") || arg.split_whitespace().nth(1) == Some("for")) {
        return;
    }
    let mut next = pragma.next_named_sibling();
    while next.is_some_and(|x| x.kind() == "comment") {
        next = next.and_then(|x| x.next_named_sibling());
    }
    let Some(region) = next else { return };
    let is_for = region.kind() == "for_statement";

    // Variables that are private / reductions / declared inside the region / the loop variable.
    let mut private: HashSet<String> = HashSet::new();
    for clause in [
        "reduction",
        "private",
        "firstprivate",
        "lastprivate",
        "linear",
        "copyin",
    ] {
        let mut rest = arg;
        while let Some(i) = rest.find(&format!("{clause}(")) {
            let after = &rest[i + clause.len() + 1..];
            let end = after.find(')').unwrap_or(after.len());
            let list = after[..end].rsplit(':').next().unwrap_or("");
            private.extend(
                list.split(',')
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty()),
            );
            rest = &after[end..];
        }
    }
    if is_for
        && let Some(init) = region.child_by_field_name("initializer")
        && let Some(v) = declarator_name(init, src).or_else(|| {
            init.child_by_field_name("left")
                .map(|l| text(l, src).to_string())
        })
    {
        private.insert(v);
    }
    let mut tid_vars: HashSet<String> = HashSet::new();
    let mut protected: HashSet<usize> = HashSet::new();
    let scope: Vec<Node> = anc.to_vec();
    walk(region, &mut |n, inner| {
        match n.kind() {
            "declaration" => {
                let mut c = n.walk();
                for d in n.children_by_field_name("declarator", &mut c) {
                    if let Some(name) = declarator_name(d, src) {
                        if text(d, src).contains("omp_get_thread_num") {
                            tid_vars.insert(name.clone());
                        }
                        private.insert(name);
                    }
                }
            }
            "preproc_call" => {
                let a = n
                    .child_by_field_name("argument")
                    .map(|a| text(a, src).trim())
                    .unwrap_or("");
                if a.starts_with("omp") && (a.contains("atomic") || a.contains("critical")) {
                    if let Some(s) = n.next_named_sibling() {
                        protected.insert(s.id());
                    }
                    if is_for && a.contains("critical") {
                        o.add(
                            "omp-critical-in-loop",
                            n,
                            &scope,
                            "平行迴圈的每次迭代都進入 critical".into(),
                        );
                    }
                }
            }
            _ => {}
        }
        if inner.iter().any(|a| protected.contains(&a.id())) || protected.contains(&n.id()) {
            return false;
        }
        let (target, opsym) = match n.kind() {
            "assignment_expression" => (n.child_by_field_name("left"), op(n, src)),
            "update_expression" => (n.child_by_field_name("argument"), "++"),
            _ => return true,
        };
        let Some(target) = target else { return true };
        if opsym == "=" {
            return true;
        }
        let full: Vec<Node> = scope.iter().copied().chain(inner.iter().copied()).collect();
        match target.kind() {
            "identifier" => {
                let v = text(target, src);
                if !private.contains(v) {
                    let red = match opsym {
                        "*=" => "*",
                        "|=" => "|",
                        "&=" => "&",
                        "^=" => "^",
                        _ => "+",
                    };
                    o.add(
                        "omp-race",
                        n,
                        &full,
                        format!("`{}`：`{v}` 是所有執行緒共享的變數；加上 `reduction({red}:{v})` 或 `#pragma omp atomic`", text(n, src)),
                    );
                }
            }
            "subscript_expression" => {
                let idx = target
                    .child_by_field_name("index")
                    .or_else(|| target.child_by_field_name("indices"))
                    .map(|i| text(i, src).trim_matches(['[', ']']).trim().to_string())
                    .unwrap_or_default();
                if idx.contains("omp_get_thread_num") || tid_vars.contains(&idx) {
                    o.add(
                        "omp-false-sharing",
                        n,
                        &full,
                        format!("`{}` 依執行緒編號寫入相鄰元素", text(n, src)),
                    );
                }
            }
            _ => {}
        }
        true
    });
}

// ------------------------------------------------------------------ python

fn check_python(tree: &Tree, src: &str, f: &Facts, o: &mut Out) {
    walk(tree.root_node(), &mut |n, anc| {
        if n.kind() != "call" {
            return true;
        }
        let Some(name) = callee_name(n, src) else {
            return true;
        };
        let func = n
            .child_by_field_name("function")
            .map(|x| text(x, src))
            .unwrap_or("");
        // Inside a function used as a ROS callback?
        let cb = anc
            .iter()
            .rev()
            .find(|a| a.kind() == "function_definition")
            .and_then(|fd| fd.child_by_field_name("name"))
            .map(|x| text(x, src))
            .filter(|x| f.ros && f.callbacks.contains(*x));
        if cb.is_some() {
            if name == "sleep" && (func == "sleep" || func.ends_with("time.sleep")) {
                o.add(
                    "rclpy-sleep-in-callback",
                    n,
                    anc,
                    format!("callback 中呼叫 `{func}`"),
                );
            }
            if matches!(
                name.as_str(),
                "spin_until_future_complete" | "spin" | "spin_once"
            ) {
                o.add(
                    "rclpy-spin-in-callback",
                    n,
                    anc,
                    format!("callback 中呼叫 `{func}`"),
                );
            }
            if name == "call" {
                let recv = func.trim_end_matches(".call").to_ascii_lowercase();
                if recv.contains("cli") || recv.contains("client") {
                    o.add(
                        "rclpy-sync-call-in-callback",
                        n,
                        anc,
                        format!("callback 中同步呼叫 `{func}()`"),
                    );
                }
            }
        }
        if f.multiprocessing && matches!(name.as_str(), "Pool" | "Process" | "ProcessPoolExecutor")
        {
            let guarded = anc.iter().any(|a| {
                matches!(
                    a.kind(),
                    "function_definition" | "class_definition" | "lambda"
                ) || (a.kind() == "if_statement"
                    && a.child_by_field_name("condition")
                        .is_some_and(|c| text(c, src).contains("__name__")))
            });
            if !guarded {
                o.add("mp-main-guard", n, anc, format!("模組頂層建立 `{func}`"));
            }
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::super::{Lang, analyze};

    fn rules(lang: Lang, src: &str) -> Vec<(&'static str, u32)> {
        analyze(lang, src)
            .findings
            .iter()
            .map(|f| (f.rule, f.line))
            .collect()
    }
    fn has(lang: Lang, src: &str, rule: &str) -> bool {
        rules(lang, src).iter().any(|(r, _)| *r == rule)
    }

    #[test]
    fn bit_operations() {
        let c = "int f(unsigned x, int n) {\n  if (x & 0x4 == 0) return 1;\n  int a = 1 << n - 1;\n  unsigned b = 1 << 31;\n  unsigned ok = 1u << 31;\n  unsigned long long big = 1ull << 40;\n  unsigned w = 1u << 32;\n  if ((x & (1u << 3)) == 1) return 2;\n  if ((x & 1u) == 1) return 3;\n  if (x && 0x80) return 4;\n  if ((x & 0x4) == 0) return 5;\n  return a + b;\n}\n";
        let r = rules(Lang::C, c);
        assert!(r.contains(&("bit-precedence", 2)), "{r:?}");
        assert!(r.contains(&("shift-precedence", 3)), "{r:?}");
        assert!(r.contains(&("shift-overflow", 4)), "{r:?}");
        assert!(
            !r.iter()
                .any(|x| x.1 == 5 || x.1 == 6 || x.1 == 9 || x.1 == 11),
            "{r:?}"
        );
        assert!(r.contains(&("shift-overflow", 7)), "{r:?}");
        assert!(r.contains(&("mask-compare-one", 8)), "{r:?}");
        assert!(r.contains(&("logical-with-mask", 10)), "{r:?}");
    }

    #[test]
    fn known_false_positives_stay_quiet() {
        // Masks including bit 0, C++ stream output, C++ in a .h file.
        let c = "int f(unsigned s, unsigned v) {\n  if ((s & 3) == 1) return 1;\n  if (1 == (v & 127u)) return 2;\n  if ((v & (1u << n)) == 1) return 3;\n  return 0;\n}\n";
        let r = rules(Lang::C, c);
        assert_eq!(r, [("mask-compare-one", 4)], "{r:?}");
        let cpp = "void p(int n) { std::cout << \" n=\" << n - 1 << std::endl; YYDEBUG << \" R\" << n - 1; }\n";
        assert!(rules(Lang::Cpp, cpp).is_empty());
        let h = "class Out {\npublic:\n  virtual int output(std::string d) = 0;\n};\n";
        assert!(rules(Lang::C, h).is_empty(), "{:?}", rules(Lang::C, h));
    }

    #[test]
    fn bit_fields() {
        let c = "struct flags { int mode : 2; unsigned en : 1; uint32_t x : 4; };\ntypedef volatile struct { uint32_t en : 1; uint32_t div : 7; } ctrl_reg_t;\nstruct r { volatile uint32_t f : 3; };\n";
        let r = rules(Lang::C, c);
        assert_eq!(
            r.iter().filter(|x| x.0 == "bitfield-plain-int").count(),
            1,
            "{r:?}"
        );
        assert!(r.contains(&("bitfield-volatile-register", 2)), "{r:?}");
        assert!(r.contains(&("bitfield-volatile-register", 3)), "{r:?}");
    }

    #[test]
    fn pico_sdk_dma_pio_irq() {
        let c = r#"#include "pico/stdlib.h"
#include "hardware/dma.h"
#include "hardware/pio.h"
static bool done;
volatile bool ok_flag;
static int counter;
void dma_handler(void) {
    done = true;
    ok_flag = true;
    counter++;
    printf("x");
    dma_hw->ints0 = 1u << 0;
}
int main(void) {
    irq_set_exclusive_handler(DMA_IRQ_0, dma_handler);
    dma_channel_config c = dma_channel_get_default_config(0);
    pio_sm_init(pio0, 0, offset, &cfg);
    pio0_hw->ctrl |= 1u << sm;
    pio->ctrl &= ~(1u << sm);
    while (dma_channel_is_busy(0)) ;
    while (pio_sm_is_tx_fifo_full(pio0, sm)) {}
    for (int i = 0; i < n; i++) {
        pio_sm_put_blocking(pio0, sm, buf[i]);
    }
    float y = x * 0.5;
    float z = sqrt(y);
    uint32_t m = 1 << pin;
    return 0;
}
"#;
        let r = rules(Lang::C, c);
        let n = |id: &str| r.iter().filter(|x| x.0 == id).count();
        assert_eq!(n("irq-shared-nonvolatile"), 2, "{r:?}"); // done, counter (not ok_flag)
        assert!(r.contains(&("irq-blocking-call", 11)), "{r:?}");
        assert!(n("dma-hardcoded-channel") >= 2, "{r:?}");
        assert!(r.contains(&("pio-hardcoded-sm", 17)), "{r:?}");
        assert!(r.contains(&("rmw-register", 18)), "{r:?}");
        assert!(r.contains(&("rmw-register", 19)), "{r:?}");
        let msg = analyze(Lang::C, c)
            .findings
            .into_iter()
            .find(|f| f.rule == "rmw-register" && f.line == 19)
            .unwrap()
            .message;
        assert!(
            msg.contains("hw_clear_bits(&pio->ctrl, (1u << sm))"),
            "{msg}"
        );
        assert!(r.contains(&("busy-wait-dma", 20)), "{r:?}");
        assert!(r.contains(&("busy-wait-fifo", 21)), "{r:?}");
        assert!(r.contains(&("pio-cpu-loop", 22)), "{r:?}");
        assert!(r.contains(&("double-on-fpu32", 25)), "{r:?}");
        assert!(r.contains(&("double-on-fpu32", 26)), "{r:?}");
        assert!(r.contains(&("shift-signed-literal", 27)), "{r:?}");
        // Claiming channels / state machines silences the hard-coded warnings.
        let claimed = c.replace("int main(void) {", "int main(void) {\n    int ch = dma_claim_unused_channel(true);\n    uint sm = pio_claim_unused_sm(pio0, true);");
        assert!(!has(Lang::C, &claimed, "dma-hardcoded-channel"));
        assert!(!has(Lang::C, &claimed, "pio-hardcoded-sm"));
    }

    #[test]
    fn ros2_cpp() {
        let c = r#"#include "rclcpp/rclcpp.hpp"
#include "std_msgs/msg/string.hpp"
class Talker : public rclcpp::Node {
public:
  Talker() : Node("t") {
    sub_ = create_subscription<std_msgs::msg::String>("in", 10, [this](std_msgs::msg::String msg) {
      std::this_thread::sleep_for(100ms);
      auto fut = client_->async_send_request(req);
      fut.get();
      auto p = create_publisher<std_msgs::msg::String>("x", 10);
    });
    timer_ = create_wall_timer(1s, std::bind(&Talker::on_timer, this));
  }
  void on_timer() { rclcpp::spin_some(shared_from_this()); }
  void fine(std_msgs::msg::String msg) { std::this_thread::sleep_for(1s); }
};
"#;
        let r = rules(Lang::Cpp, c);
        assert!(r.contains(&("ros2-msg-by-value", 6)), "{r:?}");
        assert!(r.contains(&("ros2-sleep-in-callback", 7)), "{r:?}");
        assert!(r.contains(&("ros2-future-get-in-callback", 9)), "{r:?}");
        assert!(r.contains(&("ros2-create-in-callback", 10)), "{r:?}");
        assert!(r.contains(&("ros2-spin-in-callback", 14)), "{r:?}");
        assert!(!r.iter().any(|x| x.1 == 15), "{r:?}");
    }

    #[test]
    fn rclpy_and_multiprocessing() {
        let py = "import time\nimport rclpy\nfrom multiprocessing import Pool\n\nclass N(Node):\n    def __init__(self):\n        self.create_subscription(String, 'x', self.on_msg, 10)\n        self.create_timer(1.0, self.tick)\n    def on_msg(self, m):\n        time.sleep(1)\n        self.cli.call(req)\n    def tick(self):\n        rclpy.spin_until_future_complete(self, f)\n    def other(self):\n        time.sleep(1)\n\npool = Pool(4)\nif __name__ == '__main__':\n    p = Pool(2)\n";
        let r = rules(Lang::Python, py);
        assert!(r.contains(&("rclpy-sleep-in-callback", 10)), "{r:?}");
        assert!(r.contains(&("rclpy-sync-call-in-callback", 11)), "{r:?}");
        assert!(r.contains(&("rclpy-spin-in-callback", 13)), "{r:?}");
        assert!(!r.iter().any(|x| x.1 == 15), "{r:?}");
        assert!(r.contains(&("mp-main-guard", 17)), "{r:?}");
        assert!(!r.iter().any(|x| x.1 == 19), "{r:?}");
    }

    #[test]
    fn openmp_cuda_threads_perf() {
        let c = r#"#include <omp.h>
#include <string.h>
#include <math.h>
volatile int stop;
double sum_all(const double *a, int n, int *counts) {
    double sum = 0, ok = 0, prod = 1;
    #pragma omp parallel for
    for (int i = 0; i < n; i++) {
        sum += a[i];
        double t = a[i] * 2;
        t += 1;
        counts[omp_get_thread_num()]++;
        #pragma omp critical
        prod *= a[i];
    }
    #pragma omp parallel for reduction(+:ok)
    for (int i = 0; i < n; i++) { ok += a[i]; }
    for (size_t i = 0; i < strlen(s); i++) {}
    double p = pow(2, n);
    return sum + ok + p;
}
"#;
        let r = rules(Lang::C, c);
        assert!(r.contains(&("volatile-as-sync", 4)), "{r:?}");
        assert!(r.contains(&("omp-race", 9)), "{r:?}");
        assert!(
            !r.iter()
                .any(|x| x.0 == "omp-race" && (x.1 == 11 || x.1 == 14 || x.1 == 17)),
            "{r:?}"
        );
        assert!(r.contains(&("omp-false-sharing", 12)), "{r:?}");
        assert!(r.contains(&("omp-critical-in-loop", 13)), "{r:?}");
        assert!(r.contains(&("strlen-in-loop", 18)), "{r:?}");
        assert!(r.contains(&("pow-integer", 19)), "{r:?}");

        let cu = "__global__ void k(float *x) {\n  int tid = threadIdx.x;\n  if (threadIdx.x < 16) {\n    __syncthreads();\n  }\n}\nvoid host(float *d, float *h, int n) {\n  for (int i = 0; i < n; i++) cudaMemcpy(d + i, h + i, 4, cudaMemcpyHostToDevice);\n}\n";
        let r = rules(Lang::Cpp, cu);
        assert!(r.contains(&("cuda-syncthreads-divergent", 4)), "{r:?}");
        assert!(r.contains(&("cuda-memcpy-in-loop", 8)), "{r:?}");
    }

    #[test]
    fn findings_carry_enclosing_function() {
        let a = analyze(Lang::C, "int g(int x) {\n  return x & 1 == 0;\n}\n");
        assert_eq!(a.findings[0].symbol.as_deref(), Some("g"));
        assert_eq!((a.findings[0].line, a.findings[0].col), (2, 10));
    }
}
