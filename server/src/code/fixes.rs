//! Reviewed, deterministic edits for pasted C/C++ snippets. Never writes files.
use super::{
    Lang,
    syntax::{self, text, walk},
};
use serde::Serialize;
use tree_sitter::Node;

#[derive(Debug, Serialize)]
pub struct Fix {
    pub rule: &'static str,
    pub line: u32,
    pub title: &'static str,
    /// Complete replacement of the normalised snippet, for preview and apply.
    pub replacement: String,
}

fn op<'a>(n: Node, src: &'a str) -> &'a str {
    n.child_by_field_name("operator")
        .map(|n| text(n, src))
        .unwrap_or("")
}

pub fn suggest(lang: Lang, src: &str, first_line: u32) -> Vec<Fix> {
    if !matches!(lang, Lang::C | Lang::Cpp) {
        return vec![];
    }
    let Some(tree) = syntax::parse(lang, src) else {
        return vec![];
    };
    if tree.root_node().has_error() {
        return vec![];
    }
    let mut fixes = Vec::new();
    walk(tree.root_node(), &mut |n, _| {
        if fixes.len() >= 8 {
            return false;
        }
        let candidate = if n.kind() == "binary_expression" && matches!(op(n, src), "&" | "|" | "^")
        {
            // Only the unambiguous common shape `flags & MASK == 0`.
            // More complex/nested comparisons remain manual suggestions.
            n.child_by_field_name("left")
                .zip(n.child_by_field_name("right"))
                .and_then(|(left, right)| {
                    if right.kind() != "binary_expression"
                        || !matches!(op(right, src), "==" | "!=")
                        || left.kind() != "identifier"
                    {
                        return None;
                    }
                    let mask = right.child_by_field_name("left")?;
                    let zero = right.child_by_field_name("right")?;
                    if !matches!(mask.kind(), "identifier" | "number_literal")
                        || text(zero, src) != "0"
                    {
                        return None;
                    }
                    Some((
                        "bit-precedence",
                        "為位元運算加上括號",
                        n.byte_range(),
                        format!(
                            "({} {} {}) {} 0",
                            text(left, src),
                            op(n, src),
                            text(mask, src),
                            op(right, src)
                        ),
                    ))
                })
        } else if n.kind() == "for_statement"
            && !src.contains("volatile")
            && !src.contains("mistbox_length")
        {
            strlen_loop(n, src)
        } else {
            None
        };
        if let Some((rule, title, range, replacement)) = candidate {
            // Comments inside a rewritten expression must never be discarded.
            let original = &src[range.clone()];
            if original.contains("/*") || original.contains("//") {
                return true;
            }
            let mut result = src.to_string();
            result.replace_range(range, &replacement);
            if syntax::parse(lang, &result).is_some_and(|t| !t.root_node().has_error()) {
                fixes.push(Fix {
                    rule,
                    title,
                    line: syntax::line(n) + first_line.saturating_sub(1),
                    replacement: result,
                });
            }
        }
        true
    });
    fixes
}

fn strlen_loop(
    n: Node,
    src: &str,
) -> Option<(&'static str, &'static str, std::ops::Range<usize>, String)> {
    let init = n.child_by_field_name("initializer")?;
    let cond = n.child_by_field_name("condition")?;
    let body = n.child_by_field_name("body")?;
    let update = n.child_by_field_name("update")?;
    if init.kind() != "declaration"
        || text(init.child_by_field_name("type")?, src) != "size_t"
        || cond.kind() != "binary_expression"
        || op(cond, src) != "<"
    {
        return None;
    }
    let decls: Vec<_> = syntax::named_children(init)
        .into_iter()
        .filter(|n| n.kind() == "init_declarator")
        .collect();
    if decls.len() != 1 {
        return None;
    }
    let var = decls[0].child_by_field_name("declarator")?;
    if var.kind() != "identifier" || text(decls[0].child_by_field_name("value")?, src) != "0" {
        return None;
    }
    if text(cond.child_by_field_name("left")?, src) != text(var, src) {
        return None;
    }
    let call = cond.child_by_field_name("right")?;
    if call.kind() != "call_expression"
        || text(call.child_by_field_name("function")?, src) != "strlen"
    {
        return None;
    }
    let args = syntax::named_children(call.child_by_field_name("arguments")?);
    if args.len() != 1 || args[0].kind() != "identifier" {
        return None;
    }
    let string = text(args[0], src);
    if string == text(var, src)
        || update.kind() != "update_expression"
        || op(update, src) != "++"
        || text(update.child_by_field_name("argument")?, src) != text(var, src)
    {
        return None;
    }
    let mut safe = true;
    walk(body, &mut |node, _| {
        match node.kind() {
            "call_expression" | "preproc_call" | "goto_statement" | "labeled_statement"
            | "declaration" | "asm_statement" => safe = false,
            "assignment_expression" => {
                if !node
                    .child_by_field_name("left")
                    .is_some_and(|l| l.kind() == "identifier" && text(l, src) != string)
                {
                    safe = false;
                }
            }
            "update_expression" => {
                if !node
                    .child_by_field_name("argument")
                    .is_some_and(|l| l.kind() == "identifier" && text(l, src) != string)
                {
                    safe = false;
                }
            }
            _ => {}
        }
        safe
    });
    if !safe {
        return None;
    }
    let mut replacement = text(n, src).to_string();
    // Reverse source order keeps offsets stable.
    replacement.replace_range(
        call.start_byte() - n.start_byte()..call.end_byte() - n.start_byte(),
        "mistbox_length",
    );
    let end = init.end_byte() - n.start_byte();
    if !replacement[..end].ends_with(';') {
        return None;
    }
    replacement.insert_str(end - 1, &format!(", mistbox_length = strlen({string})"));
    Some((
        "strlen-in-loop",
        "在迴圈初始化時快取字串長度",
        n.byte_range(),
        replacement,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parentheses_and_unicode() {
        let src = "// 中文\nint f(unsigned x) { return x & 8 == 0; }";
        let f = suggest(Lang::C, src, 10);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].line, 11);
        assert!(f[0].replacement.contains("(x & 8) == 0"));
        assert!(suggest(Lang::C, &f[0].replacement, 1).is_empty());
        assert!(suggest(Lang::C, "int f(int x) { return x & /*keep*/ 8 == 0; }", 1).is_empty());
        assert!(suggest(Lang::Python, src, 1).is_empty());
    }
    #[test]
    fn caches_only_simple_read_only_loops() {
        let src = "#include <string.h>\nunsigned f(const char *s) { unsigned sum = 0; for (size_t i = 0; i < strlen(s); i++) { sum += (unsigned char)s[i]; } return sum; }";
        let f = suggest(Lang::C, src, 1);
        assert_eq!(f.len(), 1);
        assert!(
            f[0].replacement
                .contains("i = 0, mistbox_length = strlen(s); i < mistbox_length")
        );
        assert!(suggest(Lang::C, &f[0].replacement, 1).is_empty());
        for body in [
            "s[i] = 0;",
            "mutate(s);",
            "s++;",
            "*p = 0;",
            "char *p = s;",
            "goto out;",
        ] {
            let bad = src.replace("sum += (unsigned char)s[i];", body);
            assert!(suggest(Lang::C, &bad, 1).is_empty(), "{body}");
        }
    }
}
