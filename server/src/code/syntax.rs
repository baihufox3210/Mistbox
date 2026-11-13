//! tree-sitter parsing, plus symbols (definitions) and call sites.

use tree_sitter::{Language, Node, Parser, Tree};

use super::{Lang, Ref, Symbol};

pub fn language(lang: Lang) -> Option<Language> {
    Some(match lang {
        Lang::C => tree_sitter_c::LANGUAGE.into(),
        Lang::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        Lang::Python => tree_sitter_python::LANGUAGE.into(),
        _ => return None,
    })
}

pub fn parse(lang: Lang, src: &str) -> Option<Tree> {
    let mut p = Parser::new();
    p.set_language(&language(lang)?).ok()?;
    p.parse(src, None)
}

pub fn text<'a>(n: Node, src: &'a str) -> &'a str {
    n.utf8_text(src.as_bytes()).unwrap_or("")
}

/// 1-based line of a node's start.
pub fn line(n: Node) -> u32 {
    n.start_position().row as u32 + 1
}

pub fn line_end(n: Node) -> u32 {
    n.end_position().row as u32 + 1
}

/// 1-based column in characters.
pub fn col(n: Node, src: &str) -> u32 {
    let start = n.start_byte();
    let line_start = src[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    src[line_start..start].chars().count() as u32 + 1
}

pub fn named_children<'t>(n: Node<'t>) -> Vec<Node<'t>> {
    let mut c = n.walk();
    n.named_children(&mut c).collect()
}

pub fn children<'t>(n: Node<'t>) -> Vec<Node<'t>> {
    let mut c = n.walk();
    n.children(&mut c).collect()
}

/// Visit every node depth-first; `f` gets the node and its ancestors
/// (outermost first). Return `false` to skip a node's children.
pub fn walk<'t>(root: Node<'t>, f: &mut dyn FnMut(Node<'t>, &[Node<'t>]) -> bool) {
    fn go<'t>(
        n: Node<'t>,
        stack: &mut Vec<Node<'t>>,
        f: &mut dyn FnMut(Node<'t>, &[Node<'t>]) -> bool,
    ) {
        if !f(n, stack) {
            return;
        }
        stack.push(n);
        let mut c = n.walk();
        for k in n.children(&mut c) {
            go(k, stack, f);
        }
        stack.pop();
    }
    go(root, &mut Vec::new(), f);
}

/// Last identifier of a (possibly qualified / member) callee:
/// `foo`, `obj.foo`, `p->foo`, `ns::foo`, `obj.foo<T>` → "foo".
pub fn callee_name(call: Node, src: &str) -> Option<String> {
    let f = call.child_by_field_name("function")?;
    last_ident(f, src)
}

fn last_ident(n: Node, src: &str) -> Option<String> {
    match n.kind() {
        "identifier" | "field_identifier" | "type_identifier" | "destructor_name"
        | "operator_name" => Some(text(n, src).to_string()),
        "field_expression" => n
            .child_by_field_name("field")
            .and_then(|x| last_ident(x, src)),
        "attribute" => n
            .child_by_field_name("attribute")
            .map(|x| text(x, src).to_string()),
        "qualified_identifier" => n
            .child_by_field_name("name")
            .and_then(|x| last_ident(x, src)),
        "template_function" | "template_method" => n
            .child_by_field_name("name")
            .and_then(|x| last_ident(x, src)),
        "parenthesized_expression" | "pointer_expression" => {
            n.named_child(0).and_then(|x| last_ident(x, src))
        }
        _ => None,
    }
}

/// Name declared by a C / C++ declarator (descends pointer / function / array declarators).
pub fn declarator_name(d: Node, src: &str) -> Option<String> {
    match d.kind() {
        "identifier" | "field_identifier" | "type_identifier" | "destructor_name"
        | "operator_name" => Some(text(d, src).to_string()),
        "qualified_identifier" => Some(text(d, src).to_string()),
        _ => d
            .child_by_field_name("declarator")
            .and_then(|x| declarator_name(x, src))
            .or_else(|| {
                named_children(d)
                    .into_iter()
                    .find(|k| {
                        matches!(
                            k.kind(),
                            "identifier" | "field_identifier" | "qualified_identifier"
                        )
                    })
                    .map(|k| text(k, src).to_string())
            }),
    }
}

/// Name of a function definition (C / C++ / Python).
pub fn function_name(f: Node, src: &str) -> Option<String> {
    match f.kind() {
        "function_definition" => f
            .child_by_field_name("name")
            .map(|n| text(n, src).to_string())
            .or_else(|| {
                f.child_by_field_name("declarator")
                    .and_then(|d| declarator_name(d, src))
            }),
        _ => None,
    }
}

/// Innermost enclosing function's name (with its class for methods).
pub fn enclosing_function(ancestors: &[Node], src: &str) -> Option<String> {
    let i = ancestors
        .iter()
        .rposition(|a| a.kind() == "function_definition")?;
    let name = function_name(ancestors[i], src)?;
    let class = ancestors[..i].iter().rev().find(|a| {
        matches!(
            a.kind(),
            "class_specifier" | "struct_specifier" | "class_definition"
        )
    });
    let sep = if class.is_some_and(|c| c.kind() == "class_definition") {
        "."
    } else {
        "::"
    };
    let class = class
        .and_then(|c| c.child_by_field_name("name"))
        .map(|n| text(n, src));
    Some(match class {
        Some(c) if !name.contains("::") => format!("{c}{sep}{name}"),
        _ => name,
    })
}

pub fn symbols_and_refs(lang: Lang, tree: &Tree, src: &str) -> (Vec<Symbol>, Vec<Ref>) {
    let mut syms = Vec::new();
    let mut refs = Vec::new();
    let sep = if lang == Lang::Python { "." } else { "::" };
    walk(tree.root_node(), &mut |n, anc| {
        let class = || {
            anc.iter()
                .rev()
                .find(|a| {
                    matches!(
                        a.kind(),
                        "class_specifier" | "struct_specifier" | "class_definition"
                    )
                })
                .and_then(|c| c.child_by_field_name("name"))
                .map(|x| text(x, src).to_string())
        };
        let mut push = |name: String, kind: &'static str| {
            syms.push(Symbol {
                name,
                kind,
                line_start: line(n),
                line_end: line_end(n),
            })
        };
        match n.kind() {
            "function_definition" => {
                if let Some(name) = function_name(n, src) {
                    match class() {
                        Some(c) if !name.contains("::") => {
                            push(format!("{c}{sep}{name}"), "method")
                        }
                        _ if name.contains("::") => push(name, "method"),
                        _ => push(name, "function"),
                    }
                }
            }
            "class_definition" | "class_specifier" | "struct_specifier" | "union_specifier"
            | "enum_specifier" => {
                if n.child_by_field_name("body").is_some()
                    && let Some(name) = n.child_by_field_name("name")
                {
                    let kind = match n.kind() {
                        "struct_specifier" => "struct",
                        "union_specifier" => "union",
                        "enum_specifier" => "enum",
                        _ => "class",
                    };
                    push(text(name, src).to_string(), kind);
                }
            }
            "type_definition" => {
                if let Some(name) = n
                    .child_by_field_name("declarator")
                    .and_then(|d| declarator_name(d, src))
                {
                    push(name, "typedef");
                }
            }
            "preproc_def" | "preproc_function_def" => {
                if let Some(name) = n.child_by_field_name("name") {
                    push(text(name, src).to_string(), "macro");
                }
            }
            "call_expression" | "call" => {
                if let Some(name) = callee_name(n, src) {
                    refs.push(Ref {
                        name,
                        line: line(n),
                        in_symbol: enclosing_function(anc, src),
                    });
                }
            }
            _ => {}
        }
        true
    });
    (syms, refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_symbols_and_calls() {
        let src = "#define LED 25\ntypedef struct { int a; } pair_t;\nstruct reg { unsigned x : 1; };\nstatic void blink(int n) {\n  gpio_put(LED, 1);\n  sleep_ms(n);\n}\n";
        let t = parse(Lang::C, src).unwrap();
        let (s, r) = symbols_and_refs(Lang::C, &t, src);
        let names: Vec<_> = s.iter().map(|x| (x.name.as_str(), x.kind)).collect();
        assert!(names.contains(&("LED", "macro")));
        assert!(names.contains(&("pair_t", "typedef")));
        assert!(names.contains(&("reg", "struct")));
        assert!(names.contains(&("blink", "function")));
        let blink = s.iter().find(|x| x.name == "blink").unwrap();
        assert_eq!((blink.line_start, blink.line_end), (4, 7));
        let calls: Vec<_> = r
            .iter()
            .map(|x| (x.name.as_str(), x.in_symbol.as_deref()))
            .collect();
        assert_eq!(
            calls,
            [("gpio_put", Some("blink")), ("sleep_ms", Some("blink"))]
        );
    }

    #[test]
    fn cpp_and_python_methods() {
        let src = "class Node : public rclcpp::Node {\n public:\n  void on_msg(const Msg &m) { pub_->publish(m); }\n};\nvoid Node::tick() { rclcpp::spin_some(x); }\n";
        let t = parse(Lang::Cpp, src).unwrap();
        let (s, r) = symbols_and_refs(Lang::Cpp, &t, src);
        let names: Vec<_> = s.iter().map(|x| (x.name.as_str(), x.kind)).collect();
        assert!(names.contains(&("Node", "class")), "{names:?}");
        assert!(names.contains(&("Node::on_msg", "method")), "{names:?}");
        assert!(names.contains(&("Node::tick", "method")), "{names:?}");
        assert!(
            r.iter()
                .any(|x| x.name == "publish" && x.in_symbol.as_deref() == Some("Node::on_msg"))
        );
        assert!(
            r.iter()
                .any(|x| x.name == "spin_some" && x.in_symbol.as_deref() == Some("Node::tick"))
        );

        let py = "class Talker(Node):\n    def cb(self):\n        self.pub.publish(m)\n\ndef main():\n    rclpy.init()\n";
        let t = parse(Lang::Python, py).unwrap();
        let (s, r) = symbols_and_refs(Lang::Python, &t, py);
        let names: Vec<_> = s.iter().map(|x| (x.name.as_str(), x.kind)).collect();
        assert_eq!(
            names,
            [
                ("Talker", "class"),
                ("Talker.cb", "method"),
                ("main", "function")
            ]
        );
        assert!(
            r.iter()
                .any(|x| x.name == "publish" && x.in_symbol.as_deref() == Some("Talker.cb"))
        );
    }
}
