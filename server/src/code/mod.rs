//! Code index: symbols, call sites, static-analysis findings and clone
//! fingerprints for source files in the drive. No model involved: parsing is
//! tree-sitter (C / C++ / Python) plus a small parser for RP2350 PIO assembly.
//!
//! - `syntax`  symbols and call sites from the syntax tree
//! - `rules`   C / C++ / CUDA / Python rules (bit operations, Pico SDK, DMA,
//!   ROS 2, OpenMP / CUDA, general performance)
//! - `pio`     PIO assembly parser and rules
//! - `clones`  token fingerprints (winnowing) for duplicate-code detection
//! - `index`   background indexer and queries over the stored results
//! - `diagnose` diagnose / optimise mode: parse pasted errors and code
//! - `knowledge` built-in explanations for common error messages
//! - `resolve` look parsed input up in the index (locations, notes)

pub mod clones;
pub mod diagnose;
pub mod embed;
pub mod index;
pub mod knowledge;
pub mod pio;
pub mod resolve;
pub mod rules;
pub mod syntax;

use serde::Serialize;

/// Bump to re-analyse every file after changing rules or the fingerprinting.
pub const ENGINE: i64 = 2;
/// Larger files are not analysed.
pub const MAX_FILE_BYTES: i64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    C,
    Cpp,
    Python,
    Pio,
    /// Other source code: fingerprints only (duplicate detection).
    Generic,
    /// Markdown notes: full-text search only (solutions / optimisation notes).
    Markdown,
}

impl Lang {
    pub fn as_str(self) -> &'static str {
        match self {
            Lang::C => "c",
            Lang::Cpp => "cpp",
            Lang::Python => "python",
            Lang::Pio => "pio",
            Lang::Generic => "generic",
            Lang::Markdown => "markdown",
        }
    }

    pub fn parse(s: &str) -> Option<Lang> {
        Some(match s {
            "c" => Lang::C,
            "cpp" | "c++" | "cuda" => Lang::Cpp,
            "python" | "py" => Lang::Python,
            "pio" => Lang::Pio,
            "generic" => Lang::Generic,
            "markdown" | "md" => Lang::Markdown,
            _ => return None,
        })
    }

    /// Language from a file name; `None` for files that are not source code.
    pub fn from_name(name: &str) -> Option<Lang> {
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
        Some(match ext.as_str() {
            "c" | "h" => Lang::C,
            "cc" | "cpp" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" | "ipp" | "tpp" | "ino"
            | "cu" | "cuh" => Lang::Cpp,
            "py" | "pyw" => Lang::Python,
            "pio" => Lang::Pio,
            "rs" | "go" | "java" | "kt" | "kts" | "scala" | "js" | "mjs" | "cjs" | "jsx" | "ts"
            | "tsx" | "cs" | "swift" | "m" | "mm" | "lua" | "zig" | "v" | "sv" | "vhd" | "vhdl"
            | "s" | "asm" | "sh" | "bash" | "pl" | "rb" | "php" | "dart" | "jl" | "f90" | "f"
            | "cmake" => Lang::Generic,
            "md" | "markdown" => Lang::Markdown,
            _ => return None,
        })
    }
}

/// `.h` files are often C++ (classes, templates, namespaces).
pub fn looks_cpp(src: &str) -> bool {
    src.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with("class ")
            || l.starts_with("namespace ")
            || l.starts_with("template")
            || l.starts_with("public:")
            || l.starts_with("private:")
            || l.contains("std::")
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Very likely a bug (undefined behaviour, deadlock, data race…).
    Error,
    /// Suspicious; often a bug or a portability problem.
    Warning,
    /// Works, but there is a faster / lighter way.
    Perf,
    /// Style / robustness suggestion.
    Info,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Perf => "perf",
            Severity::Info => "info",
        }
    }
}

/// A rule in the catalogue (shown in the UI; notes can refer to `id`).
#[derive(Serialize, Debug)]
pub struct Rule {
    pub id: &'static str,
    pub severity: Severity,
    /// bit | embedded | pio | dma | ros2 | parallel | perf
    pub category: &'static str,
    pub title: &'static str,
    /// How to fix / what to do instead.
    pub hint: &'static str,
}

macro_rules! rules {
    ($( $id:literal, $sev:ident, $cat:literal, $title:literal, $hint:literal; )*) => {
        pub static RULES: &[Rule] = &[ $( Rule { id: $id, severity: Severity::$sev, category: $cat, title: $title, hint: $hint }, )* ];
    };
}

rules! {
    // ---- bit operations / bit fields (C, C++)
    "bit-precedence", Error, "bit", "位元運算與比較的優先順序",
        "`==` / `!=` / `<` 的優先順序高於 `&` `|` `^`：`x & MASK == 0` 等於 `x & (MASK == 0)`。請寫成 `(x & MASK) == 0`。";
    "shift-precedence", Warning, "bit", "位移與加減的優先順序",
        "`+` `-` 的優先順序高於 `<<` `>>`：`1 << n - 1` 等於 `1 << (n - 1)`。若要 `(1 << n) - 1` 請加括號。";
    "shift-overflow", Error, "bit", "位移超出型別寬度或有號溢位",
        "有號 `int` 左移進符號位元（如 `1 << 31`）或位移量 ≥ 型別寬度是未定義行為。改用 `1u << 31`、`1ull << 40`。RP2350（32 位元）上 `long` 只有 32 位元。";
    "shift-signed-literal", Info, "bit", "有號常數做位元遮罩",
        "`1 << n` 在 n = 31 時是未定義行為；暫存器遮罩請用無號常數 `1u << n`（Pico SDK 的慣例）。";
    "mask-compare-one", Error, "bit", "遮罩結果和 1 比較",
        "`(x & (1u << n)) == 1` 只有 n = 0 時可能成立。測試位元請寫 `(x & mask) != 0` 或 `((x >> n) & 1u) == 1u`。";
    "logical-with-mask", Warning, "bit", "邏輯運算子搭配位元遮罩",
        "`&&` / `||` 搭配十六進位遮罩通常是想寫 `&` / `|`。";
    "bitfield-plain-int", Warning, "bit", "位元欄位使用未指定正負的 int",
        "`int x : 3` 是否有號由編譯器決定。請明確寫 `unsigned x : 3`、`uint32_t x : 3` 或 `signed x : 3`。";
    "bitfield-volatile-register", Warning, "bit", "以 volatile 位元欄位對應硬體暫存器",
        "位元欄位的排列順序、填補與存取寬度由編譯器決定，寫一個欄位會變成「讀-改-寫」整個暫存器。硬體暫存器請用遮罩與位移常數（如 SDK 的 `*_BITS` / `*_LSB`），RP2350 可用 `hw_set_bits` / `hw_clear_bits` / `hw_write_masked`。";
    // ---- RP2350 / Pico SDK
    "rmw-register", Warning, "embedded", "對硬體暫存器做非原子的讀-改-寫",
        "`hw->reg |= mask` 是讀-改-寫，可能和中斷或另一個核心互相覆蓋。RP2350 的暫存器有原子別名：`hw_set_bits(&hw->reg, mask)`、`hw_clear_bits(...)`、`hw_xor_bits(...)`；部分欄位用 `hw_write_masked(...)`。";
    "dma-hardcoded-channel", Warning, "dma", "寫死 DMA 通道編號",
        "直接用固定的通道號碼，可能和 SDK 或其他函式庫用到的通道衝突。請用 `int ch = dma_claim_unused_channel(true);`，或先 `dma_channel_claim(n)`。";
    "pio-hardcoded-sm", Warning, "pio", "寫死 PIO 狀態機編號",
        "請用 `uint sm = pio_claim_unused_sm(pio, true);`（或 `pio_claim_free_sm_and_add_program`），避免和其他程式搶同一個狀態機。";
    "busy-wait-dma", Perf, "dma", "忙碌等待 DMA 完成",
        "空迴圈輪詢 `dma_channel_is_busy` 會占住 CPU。若等待期間有別的事可做，改用 DMA 完成中斷（`dma_channel_set_irq0_enabled` + `irq_set_exclusive_handler`）並以 `__wfi()` 休眠；只想等完成時用 `dma_channel_wait_for_finish_blocking`。";
    "busy-wait-fifo", Perf, "pio", "忙碌等待 PIO FIFO",
        "輪詢 `pio_sm_is_tx_fifo_full` / `pio_sm_is_rx_fifo_empty` 後再存取，可直接用 `pio_sm_put_blocking` / `pio_sm_get_blocking`；大量資料請用 DMA 搭配 `pio_get_dreq(pio, sm, is_tx)` 讓硬體搬運。";
    "pio-cpu-loop", Perf, "pio", "以 CPU 迴圈逐字搬資料進出 PIO",
        "迴圈中逐一 `pio_sm_put_blocking` / `pio_sm_get_blocking` 會讓 CPU 全程陪跑。改用 DMA：`channel_config_set_dreq(&c, pio_get_dreq(pio, sm, true))`，寫入位址 `&pio->txf[sm]`。";
    "irq-shared-nonvolatile", Warning, "embedded", "中斷處理函式修改的全域變數不是 volatile／原子",
        "主程式可能把變數快取在暫存器裡，永遠看不到中斷裡的修改。旗標請宣告為 `volatile`；多位元組或讀-改-寫的資料請用 `critical_section`、`spin_lock` 或 C11 `_Atomic`／`<stdatomic.h>`。";
    "irq-blocking-call", Warning, "embedded", "中斷處理函式中呼叫阻塞或耗時的函式",
        "中斷裡不要 `sleep_*`、`printf`、`malloc` 或 `*_blocking` 等待：會拖住其他中斷，甚至死結。只設旗標或把資料放進佇列，在主迴圈處理。";
    "double-on-fpu32", Perf, "embedded", "在單精度 FPU 上使用 double",
        "RP2350 的 Cortex-M33 FPU 只支援單精度；`0.5` 這種常數是 double，會讓整個運算變成較慢的雙精度路徑。請寫 `0.5f`，數學函式用 `sinf` / `sqrtf` 等 `f` 版本。";
    // ---- ROS 2
    "ros2-sleep-in-callback", Warning, "ros2", "在 ROS 2 callback 中 sleep",
        "callback 中 sleep 會卡住整個 executor（單執行緒時其他訂閱、計時器都停）。改用 timer（`create_wall_timer` / `create_timer`）安排延遲工作。";
    "ros2-spin-in-callback", Error, "ros2", "在 callback 中 spin",
        "在 callback 中 `spin_until_future_complete` / `spin_some` / `spin` 會重入同一個 executor，造成死結或例外。改用非同步：`async_send_request(req, response_callback)`；必要時用 `MultiThreadedExecutor` 加 `ReentrantCallbackGroup`。";
    "ros2-future-get-in-callback", Error, "ros2", "在 callback 中同步等待服務回應",
        "在 callback 中對 `async_send_request(...)` 的 future 呼叫 `.get()` / `.wait()`，回應要由同一個 executor 處理，會永遠等不到。改傳回呼函式給 `async_send_request`。";
    "ros2-create-in-callback", Perf, "ros2", "在 callback 中建立 publisher／subscription",
        "每次 callback 都建立通訊端點非常耗時，且會讓訊息遺失。請在建構子中建立一次並存成成員。";
    "ros2-msg-by-value", Perf, "ros2", "callback 以傳值方式接收訊息",
        "每則訊息都會被複製一次。請用 `const Msg::ConstSharedPtr` / `Msg::SharedPtr`、`const Msg &`，或零複製的 `std::unique_ptr<Msg>`（intra-process）。";
    "rclpy-sleep-in-callback", Warning, "ros2", "在 rclpy callback 中 time.sleep",
        "會卡住 executor。請改用 `self.create_timer(...)`，或把耗時工作移到其他執行緒。";
    "rclpy-spin-in-callback", Error, "ros2", "在 rclpy callback 中 spin",
        "在 callback 中 `rclpy.spin_until_future_complete` / `spin_once` 會死結。改用 `client.call_async(req).add_done_callback(...)`，或 `async def` callback 搭配 `await`。";
    "rclpy-sync-call-in-callback", Error, "ros2", "在 rclpy callback 中同步呼叫服務",
        "`client.call()` 在 callback 中會死結（回應需要同一個 executor 處理）。改用 `call_async` 加 `add_done_callback`，或使用不同的 callback group 與 `MultiThreadedExecutor`。";
    // ---- parallel computing
    "omp-race", Error, "parallel", "OpenMP 平行迴圈中更新共享變數（資料競爭）",
        "多個執行緒同時 `+=` 同一個共享變數會遺失更新。請加 `reduction(+:var)`；非歸約運算用 `#pragma omp atomic`，或讓每個執行緒用區域變數最後再合併。";
    "omp-false-sharing", Perf, "parallel", "以執行緒編號索引共享陣列（偽共享）",
        "`a[omp_get_thread_num()] += ...` 讓不同執行緒寫同一條 cache line，效能會大幅下降。在迴圈內累加到區域變數最後寫一次，或把每個元素補齊到 64 位元組（`alignas(64)`）。";
    "omp-critical-in-loop", Perf, "parallel", "平行迴圈內使用 critical",
        "每次迭代都進 `critical` 等於序列化。累加請用 `reduction`，單一變數更新用 `atomic`。";
    "cuda-syncthreads-divergent", Error, "parallel", "條件分支中的 __syncthreads()",
        "若同一個 block 的執行緒走不同分支，`__syncthreads()` 會死結或行為未定義。把同步移到分支外，讓所有執行緒都會執行到。";
    "cuda-memcpy-in-loop", Perf, "parallel", "迴圈中反覆 cudaMemcpy",
        "每次 `cudaMemcpy` 都會同步並有固定開銷。請合併成一次大批傳輸，或用 `cudaMemcpyAsync` 搭配 stream 與 pinned memory 讓傳輸和運算重疊。";
    "volatile-as-sync", Warning, "parallel", "以 volatile 做執行緒同步",
        "`volatile` 不提供原子性也不保證記憶體順序。執行緒間的旗標請用 `atomic_bool` / `std::atomic<bool>`。";
    "mp-main-guard", Error, "parallel", "multiprocessing 缺少 __main__ 保護",
        "在模組頂層建立 `Pool` / `Process`，用 spawn 啟動方式（Windows、macOS 預設）時子行程重新 import 模組會無限遞迴。請放進 `if __name__ == \"__main__\":`。";
    // ---- general performance
    "strlen-in-loop", Perf, "perf", "迴圈條件中呼叫 strlen",
        "每次迭代都重新計算長度，使迴圈變成 O(n²)。先把長度存進變數，或改用 `s[i] != '\\0'` 作為條件。";
    "pow-integer", Perf, "perf", "用 pow 計算整數次方",
        "`pow` 走浮點運算。2 的次方用 `1u << n`，平方用 `x * x`。";
    // ---- PIO assembly
    "pio-too-long", Error, "pio", "PIO 程式超過 32 條指令",
        "每個 PIO 區塊只有 32 個指令槽，由所有狀態機共用。請精簡程式（善用 side-set、autopull／autopush 與 `.wrap`），或分到另一個 PIO 區塊。";
    "pio-shared-memory", Info, "pio", "同一檔案的 PIO 程式總長超過 32 條",
        "同一個 PIO 區塊放不下全部程式；載入時要分配到不同的 PIO（RP2350 有 pio0、pio1、pio2）。";
    "pio-delay-overflow", Error, "pio", "PIO 延遲超出可用位元",
        "延遲與 side-set 共用 5 個位元：side-set 用掉 N 位元（`opt` 再加 1）後，延遲最大值是 2^(5 - N) - 1。";
    "pio-side-missing", Error, "pio", "缺少 side-set 值",
        "宣告 `.side_set N`（未加 `opt`）時每條指令都必須帶 `side`。若有些指令不需要，改成 `.side_set N opt`。";
    "pio-side-range", Error, "pio", "side-set 值超出位元數",
        "`.side_set N` 只能表示 0 到 2^N - 1。";
    "pio-set-range", Error, "pio", "set 的立即值超過 31",
        "`set` 只有 5 位元立即值（0–31）。更大的值請用 `pull` 從 TX FIFO 取得，或用 `mov` 搭配位移。";
    "pio-undefined-label", Error, "pio", "jmp 到未定義的標籤",
        "檢查標籤拼字，或確認標籤位在同一個 `.program` 內。";
    "pio-unknown-instruction", Warning, "pio", "無法辨識的 PIO 指令",
        "PIO 指令只有 jmp、wait、in、out、push、pull、mov、irq、set（與 nop）。";
    "pio-autopull", Perf, "pio", "可改用 autopull",
        "`pull` 緊接 `out` 時可啟用 autopull（`sm_config_set_out_shift(&c, shift_right, true, bits)`），省下一條指令與一個週期。";
    "pio-autopush", Perf, "pio", "可改用 autopush",
        "`in` 後緊接 `push` 時可啟用 autopush（`sm_config_set_in_shift(&c, shift_right, true, bits)`），省下一條指令與一個週期。";
}

pub fn rule(id: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.id == id)
}

#[derive(Serialize, Clone, Debug)]
pub struct Finding {
    pub rule: &'static str,
    pub severity: Severity,
    /// 1-based.
    pub line: u32,
    /// 1-based, in characters.
    pub col: u32,
    pub line_end: u32,
    pub message: String,
    /// Enclosing function / program, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    /// function | method | struct | union | enum | class | typedef | macro | pio_program | label
    pub kind: &'static str,
    pub line_start: u32,
    pub line_end: u32,
}

/// A call site (for "who calls this" / "what does this call").
#[derive(Serialize, Clone, Debug)]
pub struct Ref {
    pub name: String,
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_symbol: Option<String>,
}

#[derive(Debug, Default)]
pub struct Analysis {
    pub symbols: Vec<Symbol>,
    pub refs: Vec<Ref>,
    pub findings: Vec<Finding>,
    pub fingerprints: Vec<clones::Fp>,
    pub tokens: usize,
    pub lines: u32,
}

/// Analyse one source file. Pure and synchronous (run it off the async runtime).
pub fn analyze(lang: Lang, src: &str) -> Analysis {
    let lang = if lang == Lang::C && looks_cpp(src) {
        Lang::Cpp
    } else {
        lang
    };
    let mut a = Analysis {
        lines: src.lines().count() as u32,
        ..Default::default()
    };
    match lang {
        Lang::C | Lang::Cpp | Lang::Python => {
            if let Some(tree) = syntax::parse(lang, src) {
                let facts = rules::facts(lang, &tree, src);
                (a.symbols, a.refs) = syntax::symbols_and_refs(lang, &tree, src);
                a.findings = rules::check(lang, &tree, src, &facts);
            }
        }
        Lang::Pio => {
            let p = pio::parse(src);
            a.symbols = p.symbols();
            a.refs = p.refs();
            a.findings = pio::check(&p);
        }
        Lang::Generic => {}
        Lang::Markdown => return a,
    }
    a.findings.sort_by_key(|f| (f.line, f.col, f.severity));
    let toks = clones::tokenize(lang, src);
    a.tokens = toks.len();
    a.fingerprints = clones::fingerprints(&toks);
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_ids_unique_and_languages_detected() {
        let mut ids: Vec<_> = RULES.iter().map(|r| r.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), RULES.len());
        assert_eq!(Lang::from_name("a/b/main.C"), Some(Lang::C));
        assert_eq!(Lang::from_name("node.hpp"), Some(Lang::Cpp));
        assert_eq!(Lang::from_name("k.cu"), Some(Lang::Cpp));
        assert_eq!(Lang::from_name("ws2812.pio"), Some(Lang::Pio));
        assert_eq!(Lang::from_name("talker.py"), Some(Lang::Python));
        assert_eq!(Lang::from_name("notes.md"), Some(Lang::Markdown));
        assert_eq!(Lang::from_name("a.txt"), None);
        assert_eq!(Lang::from_name("Makefile"), None);
    }
}

/// Run every rule over a real source tree and print what fires (for tuning
/// false positives): `MISTBOX_TEST_CODE_CORPUS=/path cargo test corpus -- --nocapture`.
#[cfg(test)]
mod corpus {
    use super::*;

    #[test]
    fn corpus() {
        let Ok(dir) = std::env::var("MISTBOX_TEST_CODE_CORPUS") else {
            return;
        };
        let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
        let mut samples: Vec<String> = Vec::new();
        let mut files = 0;
        let mut stack = vec![std::path::PathBuf::from(dir)];
        let t = std::time::Instant::now();
        while let Some(p) = stack.pop() {
            for e in std::fs::read_dir(&p).unwrap().flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let Some(lang) = Lang::from_name(&path.to_string_lossy()) else {
                    continue;
                };
                let Ok(src) = std::fs::read_to_string(&path) else {
                    continue;
                };
                files += 1;
                for f in analyze(lang, &src).findings {
                    *counts.entry(f.rule).or_default() += 1;
                    if counts[f.rule] <= 4 {
                        samples.push(format!(
                            "{} {}:{} {}",
                            f.rule,
                            path.display(),
                            f.line,
                            f.message
                        ));
                    }
                }
            }
        }
        println!("{files} files in {:?}", t.elapsed());
        for (r, n) in &counts {
            println!("{n:6} {r}");
        }
        for s in samples {
            println!("{s}");
        }
    }
}
