//! Built-in knowledge for common error messages (no model): a regex per
//! known error, with an explanation and concrete fixes. Focused on C / C++
//! toolchains, the Pico SDK (RP2040 / RP2350), PIO, ROS 2, OpenMP / CUDA and
//! Python. Captured names are substituted into the text (`{1}`), and SDK
//! functions are mapped to the library / header that provides them.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Solution {
    /// "known" (built-in error knowledge) or "rule" (static-analysis rule).
    pub source: &'static str,
    pub id: String,
    pub title: String,
    /// Explanation and fix steps (Markdown inline code allowed).
    pub steps: Vec<String>,
    /// Related static-analysis rule, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<&'static str>,
}

struct Known {
    id: &'static str,
    re: &'static str,
    title: &'static str,
    steps: &'static [&'static str],
    rule: Option<&'static str>,
    /// Add the library / header hint for the captured symbol.
    lib_hint: bool,
}

const KNOWN: &[Known] = &[
    Known {
        id: "undefined-reference",
        re: r"undefined reference to [`'‘\x22]?([A-Za-z_][\w:]*)",
        title: "連結錯誤：找不到 `{1}` 的實作",
        steps: &[
            "宣告找得到（標頭有 include），但連結時沒有任何目標檔提供實作。",
            "實作所在的 `.c` / `.cpp` 有沒有加進 `add_executable(...)` 或 `target_sources(...)`？",
            "需要的函式庫有沒有加進 `target_link_libraries(...)`？",
            "C++ 呼叫 C 函式時，標頭要用 `extern \"C\" { ... }` 包起來。",
            "`static` 函式只在自己的檔案內可見，別的檔案用不到。",
        ],
        rule: None,
        lib_hint: true,
    },
    Known {
        id: "implicit-declaration",
        re: r"implicit declaration of function [‘'`\x22]?(\w+)",
        title: "`{1}` 沒有宣告就被呼叫",
        steps: &[
            "缺少對應的 `#include`，或函式名稱拼錯。C 編譯器會假設它回傳 `int`，參數不會檢查，常在執行期出錯。",
            "建議把這個警告當錯誤：`target_compile_options(<target> PRIVATE -Werror=implicit-function-declaration)`。",
        ],
        rule: None,
        lib_hint: true,
    },
    Known {
        id: "multiple-definition",
        re: r"multiple definition of [`'‘\x22]?([\w:]+)",
        title: "`{1}` 被定義了不只一次",
        steps: &[
            "多半是在標頭檔裡「定義」了變數或函式，被好幾個 `.c` include。",
            "變數：標頭寫 `extern int x;`，只在一個 `.c` 寫 `int x = 0;`。",
            "函式：標頭內的實作加上 `static inline`，或移到 `.c`。",
            "GCC 10 起預設 `-fno-common`，舊程式碼的未初始化全域變數也會出現這個錯誤。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "no-such-file",
        re: r"(?:fatal error: )?([\w./\-]+\.h(?:pp)?): No such file or directory",
        title: "找不到標頭檔 `{1}`",
        steps: &[
            "Pico SDK 的 `hardware/xxx.h` 由對應的函式庫提供：把 `hardware_xxx` 加進 `target_link_libraries(...)`，include 路徑會一併加入。",
            "ROS 2：`find_package(<pkg> REQUIRED)` 並 `ament_target_dependencies(<target> <pkg>)`；`package.xml` 也要加 `<depend>`。",
            "自己的標頭：`target_include_directories(<target> PRIVATE include)`。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "not-declared",
        re: r"[‘'`](\w+)['’] (?:was not declared in this scope|undeclared|is not a member of)",
        title: "`{1}` 未宣告",
        steps: &[
            "缺少 `#include`、命名空間（如 `std::`、`rclcpp::`）或名稱拼錯。",
            "若是成員函式中使用成員變數，確認宣告在類別裡。",
        ],
        rule: None,
        lib_hint: true,
    },
    Known {
        id: "parentheses-bitwise",
        re: r"suggest parentheses around comparison in operand of [‘'`]?[&|^]",
        title: "位元運算與比較的優先順序",
        steps: &["`x & MASK == 0` 等於 `x & (MASK == 0)`，請寫成 `(x & MASK) == 0`。"],
        rule: Some("bit-precedence"),
        lib_hint: false,
    },
    Known {
        id: "shift-count",
        re: r"(?:left |right )?shift count (?:>= width of type|is negative)|shift-count-overflow",
        title: "位移量超出型別寬度",
        steps: &["`1 << 32`、`1u << 40` 是未定義行為；64 位元請用 `1ull << n`，並確認 n 的範圍。"],
        rule: Some("shift-overflow"),
        lib_hint: false,
    },
    Known {
        id: "discards-volatile",
        re: r"discards [‘'`]volatile['’] qualifier",
        title: "傳遞時遺失 volatile",
        steps: &[
            "把 `volatile` 緩衝區傳給不接受 volatile 的函式（如 `memcpy`），編譯器可能把存取最佳化掉。",
            "DMA 或中斷會改的緩衝區：函式參數也宣告成 `volatile`，或在存取前後用 `__dmb()` / `__compiler_memory_barrier()`。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "segfault",
        re: r"(?i)segmentation fault|sigsegv|core dumped",
        title: "記憶體存取錯誤（segmentation fault）",
        steps: &[
            "常見原因：空指標、陣列越界、使用已釋放的記憶體、堆疊溢位（遞迴或大型區域陣列）。",
            "用 `gdb` 執行後輸入 `bt` 看呼叫堆疊；或以 `-fsanitize=address -g` 編譯，會直接指出出錯的行。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "hardfault",
        re: r"(?i)hard ?fault|isr_hardfault",
        title: "RP2040／RP2350 HardFault",
        steps: &[
            "常見原因：空指標或越界存取、未對齊存取、堆疊溢位（尤其 core1：`multicore_launch_core1_with_stack` 或調大 `PICO_STACK_SIZE` / `PICO_CORE1_STACK_SIZE`）、在擦寫 flash 時執行 flash 上的程式碼。",
            "用 debugprobe + `gdb`：停在 `isr_hardfault` 後執行 `bt`，或查看 stacked PC。",
            "中斷處理函式會在 flash 擦寫期間執行時，標上 `__not_in_flash_func(...)`。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "pico-panic",
        re: r"\*\*\* PANIC \*\*\*|PANIC:",
        title: "Pico SDK panic",
        steps: &[
            "panic 訊息之後就是原因；常見的有 DMA 通道或 PIO 狀態機都被占用（`*_claim_unused_*(true)` 找不到空的）、PIO 程式空間不足、記憶體配置失敗。",
            "用 `dma_claim_unused_channel(false)` 等「不 panic」的版本並自行處理 -1，可以印出更清楚的錯誤。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "pio-no-space",
        re: r"(?i)no program space|program too (?:large|long)|too many instructions",
        title: "PIO 指令記憶體不足",
        steps: &[
            "每個 PIO 區塊只有 32 個指令槽，由 4 個狀態機共用。",
            "精簡程式：用 side-set 取代 `set pins`、啟用 autopull／autopush、善用 `.wrap` 省掉迴圈的 `jmp`。",
            "改載入到其他 PIO 區塊（RP2350 有 pio0、pio1、pio2），或用 `pio_claim_free_sm_and_add_program_for_gpio_range` 自動挑選。",
        ],
        rule: Some("pio-too-long"),
        lib_hint: false,
    },
    Known {
        id: "pio-delay",
        re: r"(?i)delay (?:too large|out of range)|side[- ]set (?:value )?(?:too large|out of range)",
        title: "PIO 延遲或 side-set 超出範圍",
        steps: &[
            "延遲與 side-set 共用 5 個位元：`.side_set N`（`opt` 再加 1）後，延遲最大 2^(5 - N) - 1。較長的延遲改用迴圈或降低 `clkdiv`。",
        ],
        rule: Some("pio-delay-overflow"),
        lib_hint: false,
    },
    Known {
        id: "pico-sdk-path",
        re: r"PICO_SDK_PATH|SDK location was not specified",
        title: "找不到 Pico SDK",
        steps: &[
            "設定環境變數 `export PICO_SDK_PATH=/path/to/pico-sdk`，或在 CMake 加 `-DPICO_SDK_PATH=...`。",
            "專案要有 `pico_sdk_import.cmake`，並在 `project()` 之前 `include(pico_sdk_import.cmake)`。",
            "RP2350：`-DPICO_BOARD=pico2`（或 `pico2_w`）。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "stack-smashing",
        re: r"(?i)stack smashing detected|stack overflow",
        title: "堆疊溢位／堆疊破壞",
        steps: &[
            "區域陣列越界寫入，或遞迴太深、區域變數太大。大型緩衝區改成 `static` 或動態配置。",
            "嵌入式：調大堆疊（`PICO_STACK_SIZE`），並用 `-fstack-usage` 檢查每個函式的用量。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "ros-package-config",
        re: r#"Could not find a package configuration file provided by "?([\w-]+)"?"#,
        title: "CMake 找不到套件 `{1}`",
        steps: &[
            "先 `source /opt/ros/<distro>/setup.bash`（以及自己工作區的 `install/setup.bash`）再 colcon build。",
            "缺少的套件：`rosdep install --from-paths src -y --ignore-src`，或 `sudo apt install ros-<distro>-<pkg>`（底線換成 `-`）。",
            "`package.xml` 加上 `<depend>{1}</depend>`。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "python-module",
        re: r"ModuleNotFoundError: No module named '([\w.]+)'",
        title: "Python 找不到模組 `{1}`",
        steps: &[
            "ROS 2 的 `rclpy`、訊息套件：執行前要 `source /opt/ros/<distro>/setup.bash` 和工作區的 `install/setup.bash`。",
            "自己的套件：確認 `setup.py` 的 `packages`、`entry_points`，且重新 `colcon build`。",
            "一般套件：在正確的虛擬環境 `pip install`；ROS 2 用的 Python 版本要和系統一致。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "ros-qos",
        re: r"(?i)incompatible qos|offering incompatible|requesting incompatible",
        title: "ROS 2 QoS 不相容，訊息收不到",
        steps: &[
            "publisher 與 subscription 的 reliability／durability 不相容（例如 publisher 是 best_effort，subscription 要求 reliable）。",
            "感測器資料兩端都用 `rclcpp::SensorDataQoS()` / `qos_profile_sensor_data`；用 `ros2 topic info -v <topic>` 檢查兩端設定。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "ros-executor",
        re: r"(?i)already been added to an? (?:executor|wait set)|spin_until_future_complete.*(?:already|deadlock)",
        title: "ROS 2 executor 重入",
        steps: &[
            "在 callback 中再 spin 同一個 node／executor 會出錯或死結。",
            "改用非同步：`client->async_send_request(req, callback)`；需要同時處理多個 callback 時用 `MultiThreadedExecutor` 加 `ReentrantCallbackGroup` / `MutuallyExclusiveCallbackGroup`。",
        ],
        rule: Some("ros2-spin-in-callback"),
        lib_hint: false,
    },
    Known {
        id: "ros-tf",
        re: r"(?i)lookup would require extrapolation|does not exist\. canTransform|passed to lookupTransform",
        title: "TF 查詢失敗",
        steps: &[
            "時間戳記超出 TF 緩衝範圍，或座標系還沒發布。",
            "查詢最新變換用 `tf2::TimePointZero`（rclcpp）／`rclpy.time.Time()`，或給 `lookupTransform` 逾時；確認 `static_transform_publisher` 已啟動。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "omp-not-enabled",
        re: r"(?i)ignoring ['‘]?#pragma omp|unknown-pragmas.*omp|undefined reference to [`'‘]omp_",
        title: "OpenMP 沒有啟用",
        steps: &[
            "CMake：`find_package(OpenMP REQUIRED)` 並 `target_link_libraries(<target> PRIVATE OpenMP::OpenMP_CXX)`（C 用 `OpenMP::OpenMP_C`）。",
            "直接編譯：加 `-fopenmp`。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "tsan-race",
        re: r"ThreadSanitizer: data race",
        title: "ThreadSanitizer 偵測到資料競爭",
        steps: &[
            "兩個執行緒同時存取同一個變數且至少一方寫入，沒有同步。",
            "改用 `std::atomic` / `_Atomic`、`std::mutex`，OpenMP 用 `reduction` 或 `atomic`。",
        ],
        rule: Some("omp-race"),
        lib_hint: false,
    },
    Known {
        id: "cuda-illegal-address",
        re: r"(?i)an illegal memory access was encountered|misaligned address|cudaErrorIllegalAddress",
        title: "CUDA 非法記憶體存取",
        steps: &[
            "多半是索引越界：kernel 開頭加 `if (i >= n) return;`，並確認 grid × block ≥ n 且沒有多算。",
            "用 `compute-sanitizer ./app` 找出出錯的 kernel 與行號；host 端每次呼叫後檢查 `cudaGetLastError()`。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "cuda-resources",
        re: r"(?i)too many resources requested for launch|out of memory|cudaErrorMemoryAllocation",
        title: "CUDA 資源不足",
        steps: &[
            "每個 block 的執行緒太多或每個執行緒用的暫存器太多：減少 block 大小，或用 `__launch_bounds__(maxThreads)` 限制暫存器。",
            "記憶體不足：分批處理、釋放不用的配置、避免在迴圈中重複 `cudaMalloc`。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "mp-bootstrap",
        re: r"(?s)attempt has been made to start a new process before.*bootstrapping",
        title: "multiprocessing 缺少 __main__ 保護",
        steps: &["把建立 `Pool` / `Process` 的程式碼放進 `if __name__ == \"__main__\":`。"],
        rule: Some("mp-main-guard"),
        lib_hint: false,
    },
    // ---- Pico SDK build / flashing
    Known {
        id: "pio-header-missing",
        re: r"([\w\-]+)\.pio\.h: No such file or directory",
        title: "找不到 `{1}.pio.h`（PIO 標頭沒有產生）",
        steps: &[
            "`.pio.h` 是建置時由 pioasm 產生的：在 CMakeLists.txt 加上 `pico_generate_pio_header(<target> ${CMAKE_CURRENT_LIST_DIR}/{1}.pio)`。",
            "`.pio` 檔要在 `pico_generate_pio_header` 指定的路徑；改完重新 `cmake` 一次。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "ram-overflow",
        re: r"region [`'‘]?(RAM|SCRATCH_[XY]|FLASH)['’]? overflowed by (\d+) bytes|will not fit in region [`'‘]?(\w+)",
        title: "記憶體區段 `{1}` 放不下",
        steps: &[
            "RAM：RP2350 有 520 KB SRAM（RP2040 264 KB）。大型緩衝區改小、改成 `const`（放在 flash），或用 `static` 讓連結器看得到實際用量；查看 `.map` 檔找出大戶。",
            "FLASH：程式超過 flash 大小，檢查 `PICO_FLASH_SIZE_BYTES` 是否符合板子，移除除錯用的 `printf` 與大型表格。",
            "SCRATCH_X / SCRATCH_Y：核心堆疊放不下，調整 `PICO_STACK_SIZE` / `PICO_CORE1_STACK_SIZE`。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "picotool-no-device",
        re: r"(?i)no accessible rp-series devices|no accessible rp2040 devices|unable to connect to.*bootsel",
        title: "picotool 找不到板子",
        steps: &[
            "按住 BOOTSEL 再接上 USB（或按 RUN 時按住 BOOTSEL），進入 USB 隨身碟模式。",
            "Linux 權限：安裝 picotool 的 udev 規則（`99-picotool.rules`），或暫時用 `sudo`。",
            "程式正在執行且有 USB stdio 時，可用 `picotool load -f` 讓它自動重開進 BOOTSEL。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "pico-platform",
        re: r"(?i)unknown board|PICO_PLATFORM|rp2350-(?:arm-s|riscv)|not supported on (?:RP2040|RP2350)",
        title: "Pico 板子／平台設定不符",
        steps: &[
            "RP2350（Pico 2）：`cmake -DPICO_BOARD=pico2 ..`（Pico 2 W 用 `pico2_w`），平台會自動設為 `rp2350-arm-s`；要用 RISC-V 核心則 `-DPICO_PLATFORM=rp2350-riscv`。",
            "換板子後要刪掉 build 資料夾重新 cmake，舊的快取會保留原本的平台。",
            "RP2350 專用的 API（如 pio2、新的 PIO 指令）在 RP2040 不能用。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "led-pin-undeclared",
        re: r"PICO_DEFAULT_LED_PIN['’]? undeclared|PICO_DEFAULT_LED_PIN.*not declared",
        title: "沒有 `PICO_DEFAULT_LED_PIN`",
        steps: &[
            "Pico W / Pico 2 W 的 LED 接在無線晶片上，要用 `cyw43_arch_gpio_put(CYW43_WL_GPIO_LED_PIN, 1)`，並連結 `pico_cyw43_arch_none`。",
            "或確認 `PICO_BOARD` 設定正確（未設定時預設為 pico）。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "dma-claimed",
        re: r"(?i)dma channel \d+ (?:is )?already claimed|no dma channels? (?:are )?available|all dma channels",
        title: "DMA 通道已被占用",
        steps: &[
            "寫死的通道編號和其他程式碼或函式庫衝突。改用 `dma_claim_unused_channel(true)` 取得通道，用完 `dma_channel_unclaim`。",
            "RP2350 有 16 個 DMA 通道；確認沒有在迴圈中重複 claim 卻沒有釋放。",
        ],
        rule: Some("dma-hardcoded-channel"),
        lib_hint: false,
    },
    Known {
        id: "sm-claimed",
        re: r"(?i)state machine \d+ (?:is )?already claimed|no (?:free|unused) state machines?|pio_sm_claim",
        title: "PIO 狀態機已被占用",
        steps: &[
            "改用 `pio_claim_unused_sm(pio, true)`，或 `pio_claim_free_sm_and_add_program_for_gpio_range(...)` 自動挑選 PIO 區塊與狀態機。",
            "每個 PIO 區塊 4 個狀態機；RP2350 有 3 個 PIO 區塊。",
        ],
        rule: Some("pio-hardcoded-sm"),
        lib_hint: false,
    },
    // ---- bit operations / integer types
    Known {
        id: "ubsan-shift",
        re: r"(?i)shift exponent -?\d+ is (?:too large|negative)|left shift of (?:negative value|-?\d+ by \d+ places cannot be represented)",
        title: "位移造成未定義行為（UBSan）",
        steps: &[
            "有號整數左移進符號位元、位移負數，或位移量 ≥ 型別寬度都是未定義行為。",
            "遮罩用無號常數：`1u << n`、`1ull << n`；位移前確認 `n < 32`（或 64）。",
        ],
        rule: Some("shift-overflow"),
        lib_hint: false,
    },
    Known {
        id: "signed-overflow",
        re: r"(?i)signed integer overflow|integer overflow in expression",
        title: "有號整數溢位",
        steps: &[
            "有號整數溢位是未定義行為，編譯器最佳化後結果可能很奇怪。",
            "計數器、位元運算與時間戳記改用無號或更寬的型別（`uint32_t`、`int64_t`）；比較時間用相減（`(int32_t)(a - b) > 0`）。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "truncated-constant",
        re: r"(?i)overflow in (?:conversion|implicit constant conversion)|large integer implicitly truncated|unsigned conversion from .* changes value|-Woverflow",
        title: "常數被截斷",
        steps: &[
            "數值超出目標型別的範圍（例如把 `0x1FF` 存進 `uint8_t`）。",
            "確認暫存器欄位寬度，用遮罩明確取出需要的位元：`(uint8_t)(x & 0xFFu)`。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "always-true-range",
        re: r"(?i)comparison is always (?:true|false) due to limited range of data type|comparison of unsigned expression .* is always (?:true|false)",
        title: "比較結果永遠固定（型別範圍）",
        steps: &[
            "常見於 `unsigned x >= 0`（永遠成立）或 `uint8_t` 和 256 比較；迴圈 `for (unsigned i = n; i >= 0; i--)` 會無窮迴圈。",
            "`char` 在 ARM 上預設是無號的，存 -1 判斷 EOF 會失敗；用 `int` 或 `signed char`。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "bitfield-width",
        re: r"(?i)width of [‘'`]?\w+['’]? exceeds its type|is narrower than values of its type|bit-field .* (?:too large|has negative width)",
        title: "位元欄位寬度錯誤",
        steps: &[
            "位元欄位寬度不能超過其型別的位元數（例如 `uint8_t x : 9`）。",
            "列舉型別的位元欄位要夠寬，能放下所有列舉值。對應硬體暫存器時建議改用遮罩與位移常數。",
        ],
        rule: Some("bitfield-plain-int"),
        lib_hint: false,
    },
    Known {
        id: "strict-aliasing",
        re: r"dereferencing type-punned pointer will break strict-aliasing rules|-Wstrict-aliasing",
        title: "型別雙關違反 strict aliasing",
        steps: &[
            "用不同型別的指標讀同一塊記憶體（例如把 `float*` 轉成 `uint32_t*`）在最佳化後可能讀到錯的值。",
            "改用 `memcpy(&u, &f, sizeof u)`（編譯器會最佳化成單一指令）或 `union`；C++20 可用 `std::bit_cast`。",
        ],
        rule: None,
        lib_hint: false,
    },
    // ---- memory errors
    Known {
        id: "heap-corruption",
        re: r"(?i)double free|free\(\): invalid|malloc\(\): corrupted|heap corruption|corrupted size vs\. prev_size",
        title: "堆積記憶體損壞",
        steps: &[
            "重複 free、free 非 malloc 來的指標，或寫超過配置的範圍。",
            "以 `-fsanitize=address -g` 編譯執行，會指出第一個出錯的存取與配置位置；free 後把指標設為 `NULL`。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "asan-overflow",
        re: r"AddressSanitizer: (heap-buffer-overflow|stack-buffer-overflow|global-buffer-overflow|heap-use-after-free|stack-use-after-return)",
        title: "AddressSanitizer：{1}",
        steps: &[
            "報告的第一個堆疊是出錯的存取，下面的「allocated by / freed by」指出記憶體的來源。",
            "檢查迴圈邊界（`<` 與 `<=`）、`sizeof` 用在指標而不是陣列、字串少算結尾的 `\\0`。",
        ],
        rule: None,
        lib_hint: false,
    },
    // ---- ROS 2
    Known {
        id: "ros-no-executable",
        re: r"(?i)no executable found|failed to load entry point|package ['\x22]?([\w-]+)['\x22]? not found",
        title: "ros2 run 找不到套件或執行檔",
        steps: &[
            "建置後要 `source install/setup.bash`（每個新終端機都要）。",
            "Python 套件：`setup.py` 的 `entry_points={'console_scripts': ['name = pkg.module:main']}`；C++：`install(TARGETS <exe> DESTINATION lib/${PROJECT_NAME})`。",
            "修改 `setup.py` 或 `CMakeLists.txt` 後重新 `colcon build`（Python 可加 `--symlink-install`）。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "ros-context",
        re: r"(?i)rclcpp::exceptions::RCLError|context is (?:not valid|invalid)|rcl_init.*already|must call rclcpp::init|rclpy\.init\(\) has not been called",
        title: "ROS 2 context 未初始化或已關閉",
        steps: &[
            "建立任何 node 之前先呼叫 `rclcpp::init(argc, argv)` / `rclpy.init()`，結束時最後才 `shutdown()`。",
            "不要在 `shutdown()` 之後還使用 node、publisher 或 timer；全域或 static 的 node 物件會在 shutdown 後才解構。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "ros-duplicate-node",
        re: r"(?i)nodes? (?:in the graph )?(?:that )?share an exact name|found remote node with the same name|publisher already registered for provided node name",
        title: "ROS 2 節點名稱重複",
        steps: &[
            "同一個名稱啟動了兩個節點，會造成參數、服務與 tf 混亂。",
            "用 launch 的 `name=` 或 `--ros-args -r __node:=新名稱`，或加 namespace（`-r __ns:=/robot1`）。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "ros-interfaces",
        re: r"(?i)rosidl_generate_interfaces|Could not find ROS interfaces|rosidl.*not found|member_of_group",
        title: "自訂訊息（msg / srv）產生失敗",
        steps: &[
            "`package.xml` 需要 `<buildtool_depend>rosidl_default_generators</buildtool_depend>`、`<exec_depend>rosidl_default_runtime</exec_depend>` 與 `<member_of_group>rosidl_interface_packages</member_of_group>`。",
            "`CMakeLists.txt`：`rosidl_generate_interfaces(${PROJECT_NAME} \"msg/X.msg\" DEPENDENCIES std_msgs)`；同套件的 C++ 要用 `rosidl_get_typesupport_target` 連結。",
            "訊息套件最好獨立成一個 ament_cmake 套件。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "dds-participant",
        re: r"(?i)failed to find a free participant index|fastrtps.*shm|RTPS_TRANSPORT_SHM|failed to create (?:participant|domain participant)",
        title: "DDS 參與者建立失敗",
        steps: &[
            "同一個 `ROS_DOMAIN_ID` 上的參與者太多，或殘留的 Fast DDS 共享記憶體檔：刪除 `/dev/shm/fastrtps_*`（確認沒有 ROS 程式在跑）。",
            "不同專案用不同的 `ROS_DOMAIN_ID`（0–101）；Docker 容器間通訊需要 `--network host` 與 `--ipc host`。",
        ],
        rule: None,
        lib_hint: false,
    },
    // ---- parallel computing
    Known {
        id: "thread-create",
        re: r"(?i)pthread_create.*(?:failed|resource temporarily unavailable)|libgomp: Thread creation failed|std::system_error.*Resource temporarily unavailable",
        title: "無法建立執行緒",
        steps: &[
            "執行緒數量超過系統或容器限制（`ulimit -u`、cgroup 的 pids 限制），或每個執行緒的堆疊太大。",
            "用執行緒池取代為每個工作開新執行緒；OpenMP 用 `OMP_NUM_THREADS` 限制數量，避免巢狀平行（`OMP_MAX_ACTIVE_LEVELS=1`）。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "deadlock",
        re: r"(?i)resource deadlock (?:avoided|would occur)|EDEADLK|lock-order-inversion|potential deadlock",
        title: "死結",
        steps: &[
            "同一個執行緒重複鎖同一個非遞迴 mutex，或兩個執行緒以相反順序取得兩把鎖。",
            "固定取鎖順序，或用 `std::scoped_lock(a, b)` 一次取得多把鎖；ThreadSanitizer 的 lock-order-inversion 報告會列出兩條路徑。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "cuda-no-kernel-image",
        re: r"(?i)no kernel image is available for execution on the device|cudaErrorNoKernelImageForDevice|unsupported gpu architecture",
        title: "CUDA 編譯的架構不符合這張 GPU",
        steps: &[
            "用 `nvidia-smi --query-gpu=compute_cap --format=csv` 查 GPU 的 compute capability。",
            "CMake：`set(CMAKE_CUDA_ARCHITECTURES 89)`（依你的 GPU）或 `native`；nvcc：`-arch=sm_89`。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "cuda-launch-config",
        re: r"(?i)invalid configuration argument|cudaErrorInvalidConfiguration|unspecified launch failure|cudaErrorLaunchFailure",
        title: "CUDA kernel 啟動失敗",
        steps: &[
            "invalid configuration：每個 block 最多 1024 個執行緒、grid 的 y／z 維度最多 65535，或動態 shared memory 超過上限。",
            "unspecified launch failure：kernel 內越界存取或除以零，用 `compute-sanitizer` 找出位置。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "mp-pickle",
        re: r"(?i)can't pickle|cannot pickle|PicklingError|AttributeError: Can't get attribute",
        title: "multiprocessing 無法序列化",
        steps: &[
            "傳給 Pool／Process 的工作函式應可由子行程匯入，優先使用模組頂層函式；lambda、巢狀函式及不可序列化的物件狀態常會失敗。",
            "檔案、socket、鎖等物件無法傳遞，改在子行程中自行建立。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "tsan-data-race",
        re: r#"(?i)ThreadSanitizer: data race"#,
        title: "執行緒同時讀寫共享資料",
        steps: &[
            "原因：ThreadSanitizer 偵測到未受同步保護的並行存取。",
            "修正：比對報告的兩個存取堆疊；以同一把鎖保護共享資料，或依需求使用 atomic／OpenMP reduction。",
            "驗證：用 -fsanitize=thread -g 重跑相同負載；不要只把變數改成 volatile。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "tsan-unlock",
        re: r#"(?i)ThreadSanitizer: unlock of an unlocked mutex|ThreadSanitizer: double lock"#,
        title: "mutex 的上鎖與解鎖不成對",
        steps: &[
            "原因：mutex 被重複上鎖、未上鎖就解鎖，或由錯誤的執行緒解鎖。",
            "修正：使用 lock_guard／unique_lock 管理生命週期，檢查例外與提前 return 的路徑。",
            "驗證：用 ThreadSanitizer 重跑，確認每次鎖定只由持有者解鎖。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "cuda-illegal-address",
        re: r#"(?i)cudaErrorIllegalAddress|CUDA error: an illegal memory access|compute-sanitizer.*Invalid __global__"#,
        title: "CUDA 存取了無效記憶體",
        steps: &[
            "原因：可能是索引越界、無效裝置指標，或非同步工作仍在使用已釋放的緩衝區。",
            "修正：檢查 kernel 索引邊界及配置／釋放順序；用 compute-sanitizer --tool memcheck 定位第一個錯誤。",
            "驗證：CUDA 錯誤可能延後回報；除錯時在可疑 kernel 後暫時同步定位，正式版保留必要的資料依賴同步。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "cuda-out-of-memory",
        re: r#"(?i)cudaErrorMemoryAllocation|CUDA out of memory|CUDA error: out of memory"#,
        title: "GPU 記憶體不足",
        steps: &[
            "原因：配置量或同時存活的緩衝區超過可用顯存。",
            "修正：減少批次大小、重用緩衝區並檢查洩漏；確認其他行程是否佔用 GPU。",
            "驗證：記錄尖峰顯存及多輪執行後的使用量；只降低單次配置可能掩蓋洩漏。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "cuda-invalid-device",
        re: r#"(?i)cudaErrorInvalidDevice|CUDA error: invalid device ordinal"#,
        title: "指定的 CUDA 裝置不存在",
        steps: &[
            "原因：裝置編號超出目前行程可見範圍。",
            "修正：檢查 cudaGetDeviceCount 與 CUDA_VISIBLE_DEVICES；遮罩後的裝置編號會重新排列。",
            "驗證：在相同容器與環境列出可見 GPU，再用有效編號測試。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "mpi-truncate",
        re: r#"\bMPI_ERR_TRUNCATE\b|(?i:MPI.*message truncated)"#,
        title: "MPI 接收緩衝區太小",
        steps: &[
            "原因：接收端的 count／datatype 容量不足以容納訊息。",
            "修正：核對傳送與接收的 tag、communicator、datatype；變動長度訊息可先 Probe／Get_count 再配置。",
            "驗證：測試最大訊息及不同 rank 的長度，不要單純忽略錯誤或任意加大固定緩衝區。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "mpi-rank",
        re: r#"\bMPI_ERR_RANK\b"#,
        title: "MPI 指定了無效 rank",
        steps: &[
            "原因：目的或來源 rank 不屬於目前 communicator。",
            "修正：以 MPI_Comm_size 取得大小，確認 rank 在 0 到 size-1；分割 communicator 後要用新的編號。",
            "驗證：測試單一與多個 rank，檢查邊界與鄰居交換的編號。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "mpi-count",
        re: r#"\bMPI_ERR_COUNT\b"#,
        title: "MPI 訊息數量參數無效",
        steps: &[
            "原因：count 可能為負值、溢位，或不符合使用的 MPI 介面限制。",
            "修正：檢查元素數與位元組數是否混用；大訊息可分塊，或在環境支援時使用 large-count 介面。",
            "驗證：記錄每個 rank 的 count 與 datatype，測試空訊息及最大資料量。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "mp-broken-pool",
        re: r#"\bBrokenProcessPool\b|A process in the process pool was terminated abruptly"#,
        title: "工作行程異常結束",
        steps: &[
            "原因：worker 可能被系統終止、原生函式崩潰，或初始化失敗。",
            "修正：先讀取 worker 的第一個錯誤與系統 OOM 記錄；縮小輸入並直接執行同一工作以定位。",
            "驗證：限制同時執行工作數及記憶體；主行程要收集 Future 例外並重建已損壞的工作池。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "mp-spawn-bootstrap",
        re: r#"An attempt has been made to start a new process before the current process has finished its bootstrapping phase"#,
        title: "子行程啟動時重複載入主程式",
        steps: &[
            "原因：spawn 載入主模組時再次建立子行程。",
            "修正：將啟動 Process／Pool 的程式放進 if __name__ == \"__main__\":，worker 函式保留在模組頂層。",
            "驗證：以 spawn 啟動方式測試；確認 import 模組不會自動建立工作池。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "async-loop-running",
        re: r#"asyncio\.run\(\) cannot be called from a running event loop|This event loop is already running"#,
        title: "在已啟動的事件迴圈內再次啟動迴圈",
        steps: &[
            "原因：目前執行緒已有事件迴圈，無法再巢狀執行 asyncio.run／run_until_complete。",
            "修正：在 async 函式內直接 await；需要並行時使用 create_task 並追蹤完成與例外。",
            "驗證：確認工作在預期的事件迴圈執行，結束前取消或等待尚未完成的 task。",
        ],
        rule: None,
        lib_hint: false,
    },
    Known {
        id: "async-never-awaited",
        re: r#"RuntimeWarning: coroutine .+ was never awaited"#,
        title: "協程建立後沒有被等待",
        steps: &[
            "原因：呼叫 async 函式只建立協程物件，並不會自動執行。",
            "修正：在 async 上下文使用 await，或以 create_task 排程並保存工作參照。",
            "驗證：確認工作確實完成，並收集例外；不要只關閉警告。",
        ],
        rule: None,
        lib_hint: false,
    },
];

static COMPILED: LazyLock<Vec<Regex>> =
    LazyLock::new(|| KNOWN.iter().map(|k| Regex::new(k.re).unwrap()).collect());

/// (library for `target_link_libraries`, header) for a Pico SDK / common symbol.
pub fn library_for(sym: &str) -> Option<(&'static str, Option<&'static str>)> {
    const MAP: &[(&str, &str, Option<&str>)] = &[
        ("dma_", "hardware_dma", Some("hardware/dma.h")),
        ("channel_config_", "hardware_dma", Some("hardware/dma.h")),
        ("pio_", "hardware_pio", Some("hardware/pio.h")),
        ("sm_config_", "hardware_pio", Some("hardware/pio.h")),
        ("gpio_", "hardware_gpio", Some("hardware/gpio.h")),
        ("irq_", "hardware_irq", Some("hardware/irq.h")),
        ("spi_", "hardware_spi", Some("hardware/spi.h")),
        ("i2c_", "hardware_i2c", Some("hardware/i2c.h")),
        ("uart_", "hardware_uart", Some("hardware/uart.h")),
        ("pwm_", "hardware_pwm", Some("hardware/pwm.h")),
        ("adc_", "hardware_adc", Some("hardware/adc.h")),
        ("clock_", "hardware_clocks", Some("hardware/clocks.h")),
        ("clocks_", "hardware_clocks", Some("hardware/clocks.h")),
        ("flash_range_", "hardware_flash", Some("hardware/flash.h")),
        (
            "watchdog_",
            "hardware_watchdog",
            Some("hardware/watchdog.h"),
        ),
        ("interp_", "hardware_interp", Some("hardware/interp.h")),
        ("hw_claim", "hardware_claim", Some("hardware/claim.h")),
        ("multicore_", "pico_multicore", Some("pico/multicore.h")),
        ("queue_", "pico_util", Some("pico/util/queue.h")),
        ("sleep_", "pico_stdlib", Some("pico/stdlib.h")),
        ("busy_wait_", "pico_stdlib", Some("pico/stdlib.h")),
        ("add_alarm", "pico_stdlib", Some("pico/time.h")),
        ("add_repeating_timer", "pico_stdlib", Some("pico/time.h")),
        ("stdio_", "pico_stdlib", Some("pico/stdlib.h")),
        ("cyw43_", "pico_cyw43_arch_none", Some("pico/cyw43_arch.h")),
        ("tud_", "tinyusb_device", Some("tusb.h")),
        ("omp_", "OpenMP::OpenMP_C", Some("omp.h")),
        ("pthread_", "Threads::Threads", Some("pthread.h")),
        ("cuda", "CUDA::cudart", Some("cuda_runtime.h")),
    ];
    MAP.iter()
        .find(|(p, _, _)| sym.starts_with(p))
        .map(|(_, l, h)| (*l, *h))
}

/// Known errors found in `text` (each at most once).
pub fn known(text: &str) -> Vec<Solution> {
    let mut out: Vec<Solution> = Vec::new();
    for (k, re) in KNOWN.iter().zip(COMPILED.iter()) {
        let Some(c) = re.captures(text) else { continue };
        let cap = c.get(1).map(|m| m.as_str()).unwrap_or("");
        let fill = |s: &str| s.replace("{1}", cap);
        let mut steps: Vec<String> = Vec::new();
        if k.lib_hint
            && let Some((lib, header)) = library_for(cap)
        {
            let mut s = format!(
                "`{cap}` 來自 `{lib}`：在 CMakeLists.txt 加上 `target_link_libraries(<target> {lib})`"
            );
            if let Some(h) = header {
                s.push_str(&format!("，並 `#include \"{h}\"`"));
            }
            s.push('。');
            steps.push(s);
        }
        steps.extend(k.steps.iter().map(|s| fill(s)));
        let id = k.id.to_string();
        if out.iter().any(|x| x.id == id) {
            continue;
        }
        out.push(Solution {
            source: "known",
            id,
            title: fill(k.title),
            steps,
            rule: k.rule,
        });
    }
    // A specific explanation replaces the generic one it overlaps with.
    const MORE_SPECIFIC: &[(&str, &str)] = &[
        ("pio-header-missing", "no-such-file"),
        ("omp-not-enabled", "undefined-reference"),
    ];
    for (specific, generic) in MORE_SPECIFIC {
        if out.iter().any(|x| x.id == *specific) {
            out.retain(|x| x.id != *generic);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_errors_are_specific_and_have_actionable_steps() {
        for (message, expected) in [
            ("WARNING: ThreadSanitizer: data race", "tsan-data-race"),
            (
                "ThreadSanitizer: unlock of an unlocked mutex",
                "tsan-unlock",
            ),
            (
                "CUDA error: an illegal memory access",
                "cuda-illegal-address",
            ),
            ("CUDA out of memory", "cuda-out-of-memory"),
            ("cudaErrorInvalidDevice", "cuda-invalid-device"),
            ("MPI_ERR_TRUNCATE", "mpi-truncate"),
            ("MPI_ERR_RANK", "mpi-rank"),
            ("MPI_ERR_COUNT", "mpi-count"),
            (
                "concurrent.futures.process.BrokenProcessPool",
                "mp-broken-pool",
            ),
            (
                "An attempt has been made to start a new process before the current process has finished its bootstrapping phase",
                "mp-spawn-bootstrap",
            ),
            (
                "asyncio.run() cannot be called from a running event loop",
                "async-loop-running",
            ),
            (
                "RuntimeWarning: coroutine 'main' was never awaited",
                "async-never-awaited",
            ),
        ] {
            let solutions = known(message);
            let solution = solutions
                .iter()
                .find(|s| s.id == expected)
                .unwrap_or_else(|| panic!("{message}: {solutions:?}"));
            assert_eq!(solution.steps.len(), 3);
            assert!(solution.steps[0].starts_with("原因："));
            assert!(solution.steps[1].starts_with("修正："));
            assert!(solution.steps[2].starts_with("驗證："));
        }
        assert!(
            !known("socket: Resource temporarily unavailable")
                .iter()
                .any(|s| s.id == "thread-create")
        );
        assert!(
            known("libgomp: Thread creation failed: Resource temporarily unavailable")
                .iter()
                .any(|s| s.id == "thread-create")
        );
        assert!(known("ordinary text about memory and threads").is_empty());
    }

    #[test]
    fn known_errors_with_library_hints() {
        let s = known(
            "led.c:(.text.main+0x1c): undefined reference to `dma_channel_configure'\ncollect2: error: ld returned 1 exit status",
        );
        assert_eq!(s.len(), 1);
        assert_eq!(
            s[0].title,
            "連結錯誤：找不到 `dma_channel_configure` 的實作"
        );
        assert!(
            s[0].steps[0].contains("hardware_dma") && s[0].steps[0].contains("hardware/dma.h"),
            "{:?}",
            s[0].steps
        );

        let s = known(
            "main.c:5:5: warning: implicit declaration of function ‘pio_sm_put_blocking’ [-Wimplicit-function-declaration]",
        );
        assert_eq!(s[0].id, "implicit-declaration");
        assert!(s[0].steps[0].contains("hardware_pio"));

        let s = known(
            "led.c:24:30: warning: suggest parentheses around comparison in operand of ‘&’ [-Wparentheses]",
        );
        assert_eq!(
            (s[0].id.as_str(), s[0].rule),
            ("parentheses-bitwise", Some("bit-precedence"))
        );

        let ids = |t: &str| known(t).into_iter().map(|s| s.id).collect::<Vec<_>>();
        assert_eq!(
            ids("ModuleNotFoundError: No module named 'rclpy'"),
            ["python-module"]
        );
        assert_eq!(
            ids(
                "[WARN] [1.2] [listener]: New publisher discovered on topic '/scan', offering incompatible QoS."
            ),
            ["ros-qos"]
        );
        assert_eq!(
            ids("ws2812.pio:11.27-30: error: delay too large (max 15)"),
            ["pio-delay"]
        );
        assert_eq!(
            ids("CUDA error: an illegal memory access was encountered"),
            ["cuda-illegal-address"]
        );
        assert_eq!(
            ids("Program received signal SIGSEGV, Segmentation fault."),
            ["segfault"]
        );
        assert!(ids("all good").is_empty());
        assert_eq!(
            ids("fatal error: ws2812.pio.h: No such file or directory"),
            ["pio-header-missing"]
        );
        assert_eq!(
            known("ld: region `RAM' overflowed by 4096 bytes")[0].title,
            "記憶體區段 `RAM` 放不下"
        );
        assert_eq!(
            ids("runtime error: left shift of 1 by 31 places cannot be represented in type 'int'"),
            ["ubsan-shift"]
        );
        assert_eq!(
            ids("[ERROR] [rmw_fastrtps_cpp]: Failed to find a free participant index for domain 0"),
            ["dds-participant"]
        );
        assert_eq!(
            ids("CUDA error: no kernel image is available for execution on the device"),
            ["cuda-no-kernel-image"]
        );
        assert_eq!(ids("No executable found"), ["ros-no-executable"]);
        assert_eq!(ids("width of 'mode' exceeds its type"), ["bitfield-width"]);
        assert_eq!(
            ids("==1==ERROR: AddressSanitizer: heap-buffer-overflow on address"),
            ["asan-overflow"]
        );
        assert_eq!(
            library_for("multicore_launch_core1"),
            Some(("pico_multicore", Some("pico/multicore.h")))
        );
        assert_eq!(library_for("my_func"), None);
    }
}
