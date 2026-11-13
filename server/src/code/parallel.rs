//! Conservative parallel-performance checks: require recognised APIs/imports.
use super::{
    Finding, Lang, rule,
    syntax::{self, text, walk},
};
use std::collections::HashMap;
use tree_sitter::{Node, Tree};

fn scope(anc: &[Node]) -> usize {
    anc.iter()
        .rev()
        .find(|n| n.kind() == "function_definition")
        .map_or(0, |n| n.id())
}
fn in_loop(anc: &[Node]) -> bool {
    for (i, n) in anc.iter().enumerate().rev() {
        if matches!(
            n.kind(),
            "function_definition" | "lambda" | "lambda_expression"
        ) {
            return false;
        }
        if matches!(
            n.kind(),
            "for_statement"
                | "while_statement"
                | "do_statement"
                | "for_in_clause"
                | "for_range_loop"
        ) {
            // C initialisers and Python iterables run once per loop entry.
            let once = n
                .child_by_field_name("initializer")
                .or_else(|| n.child_by_field_name("right"));
            if once.is_some_and(|once| anc.get(i + 1).is_some_and(|child| child.id() == once.id()))
            {
                continue;
            }
            return true;
        }
    }
    false
}
fn add(
    out: &mut Vec<Finding>,
    id: &'static str,
    n: Node,
    anc: &[Node],
    src: &str,
    message: String,
) {
    let r = rule(id).expect("registered parallel rule");
    out.push(Finding {
        rule: id,
        severity: r.severity,
        line: syntax::line(n),
        col: syntax::col(n, src),
        line_end: syntax::line_end(n),
        message,
        symbol: syntax::enclosing_function(anc, src),
    });
}

pub fn check(lang: Lang, tree: &Tree, src: &str) -> Vec<Finding> {
    if tree.root_node().has_error() {
        return vec![];
    }
    let mut out = Vec::new();
    if matches!(lang, Lang::C | Lang::Cpp) {
        walk(tree.root_node(), &mut |n, anc| {
            if n.kind() == "call_expression" && in_loop(anc) {
                let name = n
                    .child_by_field_name("function")
                    .map(|n| text(n, src))
                    .unwrap_or("");
                if matches!(
                    name,
                    "cudaMalloc" | "cudaFree" | "cudaMallocHost" | "cudaFreeHost"
                ) {
                    add(
                        &mut out,
                        "cuda-allocation-in-loop",
                        n,
                        anc,
                        src,
                        format!("迴圈中呼叫 `{name}`，可評估重用緩衝區以減少配置成本"),
                    );
                }
                if name == "cudaDeviceSynchronize" {
                    add(
                        &mut out,
                        "cuda-device-sync-in-loop",
                        n,
                        anc,
                        src,
                        "迴圈中等待整張 GPU；確認是否只需要等待單一 stream 或事件".into(),
                    );
                }
            }
            if n.kind() == "preproc_call" {
                let arg = n
                    .child_by_field_name("argument")
                    .map(|a| text(a, src))
                    .unwrap_or("");
                let words: Vec<_> = arg.split_whitespace().collect();
                if words.first() == Some(&"omp") && words.contains(&"parallel") && in_loop(anc) {
                    add(
                        &mut out,
                        "omp-team-in-loop",
                        n,
                        anc,
                        src,
                        "外層迴圈反覆進入 OpenMP parallel 區域，可能增加執行緒團隊管理成本".into(),
                    );
                }
            }
            true
        });
        return out;
    }
    if lang != Lang::Python {
        return out;
    }
    // Resolve only module-level explicit imports. Do not guess from method names.
    let mut imports: HashMap<String, String> = HashMap::new();
    for node in syntax::named_children(tree.root_node()) {
        let raw = text(node, src);
        if node.kind() == "import_from_statement" {
            if let Some((module, names)) = raw
                .strip_prefix("from ")
                .and_then(|s| s.split_once(" import "))
            {
                if matches!(module, "concurrent.futures" | "multiprocessing" | "time") {
                    for item in names.trim_matches(['(', ')']).split(',') {
                        let parts: Vec<_> = item.split_whitespace().collect();
                        if let Some(name) = parts.first() {
                            let alias = if parts.len() == 3 && parts[1] == "as" {
                                parts[2]
                            } else {
                                name
                            };
                            imports.insert(alias.to_string(), format!("{module}.{name}"));
                        }
                    }
                }
            }
        } else if node.kind() == "import_statement" {
            for item in raw.trim_start_matches("import ").split(',') {
                let parts: Vec<_> = item.split_whitespace().collect();
                if let Some(module) = parts.first() {
                    if matches!(*module, "concurrent.futures" | "multiprocessing" | "time") {
                        let alias = if parts.len() == 3 && parts[1] == "as" {
                            parts[2]
                        } else {
                            module
                        };
                        imports.insert(alias.to_string(), module.to_string());
                    }
                }
            }
        }
    }
    let resolve = |name: &str| -> String {
        if let Some(full) = imports.get(name) {
            return full.clone();
        }
        let (base, member) = name.rsplit_once('.').unwrap_or((name, ""));
        imports
            .get(base)
            .map(|full| format!("{full}.{member}"))
            .unwrap_or_default()
    };
    let is_pool = |name: &str| {
        matches!(
            name,
            "concurrent.futures.ThreadPoolExecutor"
                | "concurrent.futures.ProcessPoolExecutor"
                | "multiprocessing.Pool"
        )
    };
    let import_roots: std::collections::HashSet<String> = imports
        .keys()
        .map(|name| name.split('.').next().unwrap_or(name).to_string())
        .collect();
    let mut pools = HashMap::new();
    let mut shadowed = std::collections::HashSet::new();
    walk(tree.root_node(), &mut |n, anc| {
        if matches!(n.kind(), "function_definition" | "class_definition") {
            if let Some(name) = n.child_by_field_name("name") {
                if import_roots.contains(text(name, src)) {
                    shadowed.insert((scope(anc), text(name, src).to_string()));
                }
            }
            if let Some(params) = n.child_by_field_name("parameters") {
                walk(params, &mut |p, _| {
                    if p.kind() == "identifier" && import_roots.contains(text(p, src)) {
                        shadowed.insert((n.id(), text(p, src).to_string()));
                    }
                    true
                });
            }
        }
        if n.kind() == "assignment" {
            if let Some(left) = n
                .child_by_field_name("left")
                .filter(|n| n.kind() == "identifier")
            {
                let key = (scope(anc), text(left, src).to_string());
                if let Some(right) = n
                    .child_by_field_name("right")
                    .filter(|n| n.kind() == "call")
                {
                    let name = right
                        .child_by_field_name("function")
                        .map(|n| resolve(text(n, src)))
                        .unwrap_or_default();
                    if is_pool(&name) && !import_roots.contains(text(left, src)) {
                        pools.insert(key.clone(), name);
                    } else {
                        shadowed.insert(key.clone());
                    }
                } else {
                    shadowed.insert(key.clone());
                }
                if import_roots.contains(text(left, src)) {
                    shadowed.insert(key);
                }
            }
        }
        true
    });
    walk(tree.root_node(), &mut |n, anc| {
        if n.kind() != "call" {
            return true;
        }
        let func = n
            .child_by_field_name("function")
            .map(|n| text(n, src))
            .unwrap_or("");
        let name = resolve(func);
        let base = func.split('.').next().unwrap_or(func);
        let shadow = shadowed.contains(&(scope(anc), base.to_string()))
            || shadowed.contains(&(0, base.to_string()));
        if !shadow && is_pool(&name) && in_loop(anc) {
            add(
                &mut out,
                "python-pool-in-loop",
                n,
                anc,
                src,
                format!("迴圈中建立 `{func}`，可將工作池移至迴圈外重用"),
            );
        }
        let async_fn = anc
            .iter()
            .rev()
            .find(|n| matches!(n.kind(), "function_definition" | "lambda"))
            .is_some_and(|n| text(*n, src).trim_start().starts_with("async def "));
        if !shadow && name == "time.sleep" && async_fn {
            add(
                &mut out,
                "python-blocking-sleep-async",
                n,
                anc,
                src,
                "async 函式中呼叫 `time.sleep`，會阻塞所在的事件迴圈執行緒".into(),
            );
        }
        // Exact `pool.submit(...).result()` in a loop; no assumptions about arbitrary .result().
        let Some(function) = n
            .child_by_field_name("function")
            .filter(|n| n.kind() == "attribute")
        else {
            return true;
        };
        if function
            .child_by_field_name("attribute")
            .map(|n| text(n, src))
            != Some("result")
            || !in_loop(anc)
        {
            return true;
        }
        let Some(submit) = function
            .child_by_field_name("object")
            .filter(|n| n.kind() == "call")
        else {
            return true;
        };
        let Some(submit_fn) = submit
            .child_by_field_name("function")
            .filter(|n| n.kind() == "attribute")
        else {
            return true;
        };
        if submit_fn
            .child_by_field_name("attribute")
            .map(|n| text(n, src))
            != Some("submit")
        {
            return true;
        }
        let Some(receiver) = submit_fn
            .child_by_field_name("object")
            .filter(|n| n.kind() == "identifier")
        else {
            return true;
        };
        let key = (scope(anc), text(receiver, src).to_string());
        if pools
            .get(&key)
            .is_some_and(|name| name.starts_with("concurrent.futures."))
            && !shadowed.contains(&key)
        {
            add(
                &mut out,
                "python-submit-wait-in-loop",
                n,
                anc,
                src,
                "每次提交工作後立即 `.result()`，這個提交迴圈無法同時保留多個待執行工作".into(),
            );
        }
        true
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ids(lang: Lang, src: &str) -> Vec<&'static str> {
        super::super::analyze(lang, src)
            .findings
            .into_iter()
            .map(|f| f.rule)
            .collect()
    }
    #[test]
    fn cuda_loop_costs_require_real_api_calls_and_loops() {
        let yes = ids(
            Lang::Cpp,
            "void f() { for (int i=0;i<10;i++) { cudaMalloc(&p, n); cudaDeviceSynchronize(); cudaFree(p); } }",
        );
        assert!(yes.contains(&"cuda-allocation-in-loop"));
        assert!(yes.contains(&"cuda-device-sync-in-loop"));
        let no = ids(
            Lang::Cpp,
            "void f() { cudaMalloc(&p, n); cudaFree(p); for (int i=0;i<10;i++) { obj.cudaMalloc(); cudaStreamSynchronize(s); const char *x = \"cudaDeviceSynchronize()\"; } }",
        );
        assert!(!no.contains(&"cuda-allocation-in-loop"));
        assert!(!no.contains(&"cuda-device-sync-in-loop"));
        let lambda = ids(
            Lang::Cpp,
            "void f() { for (int i=0;i<10;i++) { auto later = []() { cudaDeviceSynchronize(); }; } }",
        );
        assert!(!lambda.contains(&"cuda-device-sync-in-loop"));
    }
    #[test]
    fn python_checks_imports_aliases_and_receiver_provenance() {
        let yes = ids(
            Lang::Python,
            "from concurrent.futures import ThreadPoolExecutor as Executor\nimport time as clock\npool = Executor(4)\nfor i in range(10):\n    result = pool.submit(work, i).result()\n    other = Executor(2)\nasync def tick():\n    clock.sleep(1)\n",
        );
        for id in [
            "python-submit-wait-in-loop",
            "python-pool-in-loop",
            "python-blocking-sleep-async",
        ] {
            assert!(yes.contains(&id), "{id}: {yes:?}");
        }
        for src in [
            "for i in range(10):\n    pool = ThreadPoolExecutor(2)\n    fake.submit(work).result()\n",
            "from concurrent.futures import ThreadPoolExecutor\ndef f(ThreadPoolExecutor):\n    for i in range(10):\n        pool = ThreadPoolExecutor(2)\n",
            "from concurrent.futures import ThreadPoolExecutor\npool = ThreadPoolExecutor(2)\npool = fake\nfor i in range(10):\n    pool.submit(work).result()\n",
            "import time\ndef tick():\n    time.sleep(1)\n",
            "import time\nasync def tick(time):\n    time.sleep(1)\n",
        ] {
            let no = ids(Lang::Python, src);
            assert!(
                !no.iter().any(|id| id.starts_with("python-")),
                "{src}: {no:?}"
            );
        }
    }
    #[test]
    fn openmp_private_spacing_and_commented_atomic_do_not_race() {
        for clause in ["reduction (+:sum)", "private (sum)", "firstprivate (sum)"] {
            let src = format!(
                "void f() {{ int sum=0;\n#pragma omp parallel for {clause}\nfor (int i=0;i<10;i++) {{ sum += i; }} }}"
            );
            assert!(!ids(Lang::C, &src).contains(&"omp-race"), "{clause}");
        }
        let protected = "void f() { int sum=0;\n#pragma omp parallel for\nfor (int i=0;i<10;i++) {\n#pragma omp atomic\n/* protected */\nsum += i; } }";
        assert!(!ids(Lang::C, protected).contains(&"omp-race"));
        let race = protected.replace("#pragma omp atomic", "");
        assert!(ids(Lang::C, &race).contains(&"omp-race"));
    }
    #[test]
    fn guarded_and_inverted_main_checks_differ() {
        let guarded =
            "from multiprocessing import Pool\nif __name__ == '__main__':\n    p = Pool(2)\n";
        assert!(!ids(Lang::Python, guarded).contains(&"mp-main-guard"));
        assert!(ids(Lang::Python, &guarded.replace("==", "!=")).contains(&"mp-main-guard"));
    }
    #[test]
    fn loop_initialisation_is_not_repeated_work() {
        assert!(
            !ids(
                Lang::Cpp,
                "void f() { for (int i = cudaDeviceSynchronize(); i < 10; i++) {} }"
            )
            .contains(&"cuda-device-sync-in-loop")
        );
        assert!(!ids(Lang::Python, "from concurrent.futures import ThreadPoolExecutor\nfor result in ThreadPoolExecutor(2).map(work, items):\n    consume(result)\n").contains(&"python-pool-in-loop"));
    }

    #[test]
    fn omp_team_rule_requires_parallel_inside_loop() {
        let src = "void f() { for (int j=0;j<4;j++) {\n#pragma omp parallel for\nfor (int i=0;i<10;i++) { use(i); } } }";
        assert!(ids(Lang::C, src).contains(&"omp-team-in-loop"));
        assert!(!ids(Lang::C, &src.replace("parallel for", "simd")).contains(&"omp-team-in-loop"));
    }
}
