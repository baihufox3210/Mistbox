# Mistbox — 私人雲端硬碟

單一管理者使用的自架雲端硬碟。可原樣保存任何檔案，提供 IDE 風格的瀏覽與預覽：Markdown、程式碼、真正編譯的 LaTeX PDF、圖片、PDF、影音。另有可限定範圍的 API Key。

- 後端：Rust（Axum + Tokio），SQLite 保存帳號、目錄、檔案中繼資料與 API Key
- 前端：React + TypeScript（CodeMirror 6、react-markdown + KaTeX、pdf.js），兩種版面：雲端硬碟／IDE
- 語意搜尋：本地 ONNX 向量模型（multilingual-e5-small int8 量化版），內建於 app 映像；閒置時自動卸載
- 部署：Docker 多階段建置 + Docker Compose，共兩個容器
  - `app`：API 與前端靜態檔
  - `latex`：隔離的沙箱 worker（TeX Live 編譯 LaTeX、poppler 擷取 PDF 文字），沒有資料卷，只連 internal 網路

```
.
├── server/            Rust 後端（src/、migrations/、tests/）＋ latex-worker（src/bin/）
├── frontend/          React 前端（src/、vitest 測試）
├── samples/           可重現的預覽測試樣本（Markdown、程式碼、編碼、LaTeX、二進位、XSS）
├── scripts/           e2e.sh（端對端驗收）、make-samples.py（重新產生二進位樣本）
├── docs/API.md        API 文件、錯誤格式與 curl 範例
├── theme/             自訂 CSS（custom.css）、範例（custom.demo.css）、主題說明（README.md）、背景範例
├── docker/            LaTeX 建置期暖機文件
├── Dockerfile         web → rust → app / latex 多階段建置
├── compose.yaml
└── .env.example
```

程式碼、來源資料與運行時資料彼此分開：

- 資料庫與檔案本體存放在 Docker named volume `mistbox_mistbox-data`，不在專案資料夾內。
- 資料目錄內有 `.mistbox-data` 標記，匯入工具遇到含此標記的目錄一律略過，不會把自己匯入造成遞迴。

---

## 1. 啟動

需要 Docker Engine 24+ 與 Docker Compose v2。

```bash
cp .env.example .env            # 可選；不需要任何秘密
docker compose up -d --build    # 第一次建置會下載 TeX Live（約 1.5 GB 映像）
docker compose ps               # app 應顯示 healthy
```

第一次建置會下載 TeX Live、ONNX Runtime 與向量模型，需要數分鐘。

> 主機的 DNS 只有主機本身能連到時（例如 Tailscale MagicDNS 的 `100.100.100.100`），建置期間的 `apt`、`cargo` 會解析失敗。請在 `.env` 設定 `MISTBOX_BUILD_NETWORK=host` 後再建置。這只影響建置，不影響執行。

服務只綁定 `127.0.0.1:8725`。本機可直接開啟 <http://localhost:8725>；遠端存取請使用第 6 節的 HTTPS 反向代理。

## 2. 初始化管理者

沒有預設帳號或密碼。第一次啟動後執行：

```bash
docker compose exec app mistbox admin create --username admin
# 互動輸入兩次密碼（至少 10 字元）；或以 --password-stdin 從標準輸入讀取
```

- 密碼以 Argon2id 雜湊保存。只能建立一位管理者，之後再執行 `create` 會被拒絕。
- 忘記密碼：`docker compose exec app mistbox admin reset-password --username admin`（會登出所有 session）。
- 強制登出所有裝置：`docker compose exec app mistbox admin revoke-sessions`。
- 登入後可在右上角選單變更密碼與管理 API Key。

## 3. 使用介面

右上角可切換兩種版面（也可在設定或指令面板中切換）。主題可選淺色、深色或跟隨系統，主色為天藍色系。

### 雲端硬碟版面（檔案管理風格）

- 左側：「＋ 新增」（新檔案、新資料夾、上傳檔案、上傳資料夾）、資料夾樹、總容量與語意索引狀態。
- 主區域：資料夾與檔案**分區**顯示。
  - **格狀**：資料夾卡片，以及有大圖示的檔案卡片；圖片會直接顯示縮圖。
  - **清單**：顯示名稱、修改時間、大小、類型，可排序。
- 雙擊資料夾進入；雙擊檔案開啟全螢幕預覽：
  - 用 ← → 切換同資料夾內的檔案。
  - 文字檔可按「編輯」直接修改。
  - 圖片可「設為背景」，也可「在 IDE 開啟」。
- 上傳進度為右下角的浮動面板，可取消、失敗時可重試。
- 遇到同名檔案會詢問「保留兩者」或「略過」，不會覆寫。

### IDE 版面

- 左側檔案總管同時列出**資料夾與檔案**（依類型顯示圖示）。
  - 拖曳項目到資料夾即可移動，拖入外部檔案即可上傳到該資料夾。
- 檔案總管**底部**有操作列（新檔案、新資料夾、上傳、上傳資料夾、重新整理），上傳進度也顯示在這裡。
- 右側整塊是分頁式的預覽與編輯區，最下方是狀態列（路徑、未儲存提示、語言、語意索引狀態）。
- **空白處雙擊**會直接新增檔案（建立在目前選取的資料夾，或根目錄）。
- **分割編輯器**：像 VS Code 一樣左右、上下並排多個編輯器群組。
  - 分頁列右側的按鈕，或 `Ctrl+\`（向右）、`Ctrl+Alt+\`（向下）。
  - 把分頁或檔案總管裡的檔案拖到群組中央即加入該群組，拖到邊緣就在那一側分割。
  - `Ctrl+1`～`9` 切換群組；拖曳分隔線調整大小。
- **問題面板**（`Ctrl+Shift+M`，或點狀態列的 ⊗ ⚠ ⚡ 計數）：靜態分析與重複程式碼，見〈3c〉。

### 兩種版面共通

**右鍵選單**

- 檔案或資料夾上按右鍵：開啟、新增檔案或資料夾、上傳到此、下載、重新命名、移動、複製路徑、設為背景、刪除。
- 空白處按右鍵：新增檔案或資料夾、上傳、重新整理。

**編輯**

- UTF-8 文字檔、程式碼、Markdown、LaTeX 原始碼可直接編輯，按 `Ctrl+S` 儲存。
- 存檔時會帶上目前版本的 SHA-256（`If-Match`）。若檔案在其他地方被改過，儲存會被拒絕，不會覆蓋別人的版本。
- 關閉有未儲存變更的分頁，或離開頁面時，會先提示。
- 不能編輯的情況：
  - 非 UTF-8 檔案（避免改變編碼）。
  - 超過預覽上限、只載入部分內容的檔案。
- 編輯時會保留 CRLF 換行。

**指令面板**（取代原本的搜尋框）

| 快捷鍵 | 功能 |
|---|---|
| `Ctrl+Shift+P`、`F1`、`Ctrl+K`，或點頂部中央的按鈕 | 指令與檔名搜尋（新增或上傳檔案、切換版面與主題、開啟設定、重新載入自訂 CSS、下載或重新命名目前檔案、API Keys、登出…） |
| `Ctrl+P` | 依檔名開啟 |
| `Ctrl+Shift+F`，或輸入 `?` 開頭 | 語意搜尋，結果附片段與行號，點選後跳到該行 |
| 輸入 `>` 開頭 | 只顯示指令 |

Firefox 會攔截 `Ctrl+Shift+P`（開啟隱私視窗），請改用 `F1` 或 `Ctrl+K`。

**其他快捷鍵**

- `F2`：重新命名。
- `Delete`：移到垃圾桶（會先確認並顯示檔案數與容量；可勾選直接永久刪除）。
- `Enter`：開啟。
- `Backspace`：上一層。
- 方向鍵：移動選取。
- `Ctrl+A`：全選。

手機版會把側欄改成抽屜選單。

### 垃圾桶

- 刪除預設會移到**垃圾桶**。提示訊息上有「復原」按鈕，可立即還原。
- 垃圾桶的入口：
  - 雲端硬碟版面：側欄的「垃圾桶」。
  - IDE 版面：檔案總管標題列的垃圾桶圖示。
  - 指令面板：「開啟垃圾桶」。
- 垃圾桶中可以：
  - **還原**：回到原位置；原位置已有同名項目時會加上編號，原資料夾不存在時還原到根目錄。
  - **永久刪除**、**清空垃圾桶**。
- 超過 `MISTBOX_TRASH_DAYS`（預設 30 天，0 = 不自動清除）的項目，會由背景程序永久刪除。
- 垃圾桶中的項目：
  - 不會出現在清單、搜尋、語意搜尋中，也無法以路徑存取。
  - **API Key 永遠看不到**。
  - 檔案內容在永久刪除前都會保留，也會包含在備份中。
- API Key 只能把項目移到垃圾桶，不能永久刪除。

### 分享連結

- **建立**：在檔案或資料夾上按右鍵 →「分享連結…」（預覽畫面上方也有「分享」按鈕），可以設定：
  - **權限**：
    - 檢視：瀏覽、預覽、下載。
    - 可編輯：資料夾可以另外上傳、新增、改名、移動、移到垃圾桶、修改文字檔；單一檔案則可以修改內容。
  - **有效期限**：1 小時、1 天、7 天、30 天、永不過期，或自訂到期時間。
  - **備註**：只有你看得到，用來記得連結給了誰。
- **連結格式**：`https://你的網域/s/<id>#<密鑰>`。
  - 密鑰放在 `#` 之後，瀏覽器不會把這一段送到伺服器，所以不會出現在伺服器、反向代理的日誌或 Referer 裡。
  - 伺服器只保存密鑰的雜湊值，**完整連結只在建立時顯示一次**（會自動複製到剪貼簿）。要給其他人時，請另外建立一個連結。
- **對方看到的畫面**：不需要登入，是精簡版的雲端硬碟介面，可以預覽 Markdown、程式碼、LaTeX、PDF、圖片與影音。
  - 只看得到被分享的項目（資料夾含所有子項目），**永遠看不到敏感檔與垃圾桶**，也沒有語意搜尋、設定或任何管理功能。
  - 可編輯的連結也不能改名或刪除被分享的那個項目本身，也不能永久刪除任何東西。對方刪除的項目會進到你的垃圾桶，由你決定是否還原。
- **管理**：側欄的「分享連結」或指令面板的「管理分享連結」，可以看到每個連結的項目、權限、到期時間、開啟次數與最近使用時間。
  - 可以隨時把權限改成檢視或可編輯，或修改到期時間，立即生效。
  - **刪除**後連結立即失效。被分享的項目移到垃圾桶時，連結也會暫停，還原後恢復。
- 被分享的項目移動到其他位置時，連結會跟著項目走。
- 標記為敏感的項目不能分享。

### 預覽

「可保存」與「可預覽」分開處理：任何檔案都原樣保存、可下載。沒有預覽器的格式只顯示檔案資訊（大小、類型、時間、SHA-256）與下載按鈕。

**Markdown**

- 支援 GFM 表格、巢狀清單、待辦清單、引用、程式碼區塊（依語言高亮）、行內 `$…$` 與區塊 `$$…$$` 數學公式（KaTeX）。程式碼內的 `$` 不會被當成公式。
- 可切換「渲染」「原始碼」「分割」三種檢視；分割檢視下編輯時，渲染結果會即時更新。
- 相對圖片與文件連結以該 Markdown 所在的雲端目錄為基準解析，並經由 `/relative` 端點重新驗證權限。
- 外部圖片預設不載入。
- 原始 HTML 不渲染，危險網址會被移除。

**程式碼與文字**

- 使用 CodeMirror 6：語法高亮、行號、`Ctrl+F` 搜尋、複製、自動換行、編輯。
- 支援 C/C++、Java、Python、Rust、JS/TS/TSX、JSON、YAML、TOML、Shell、SQL、Markdown、LaTeX。未知格式以純文字顯示。
- 非 UTF-8 檔案會偵測編碼並顯示警告，可手動切換編碼。
- 超過預覽上限時可只載入開頭部分，或下載完整檔案。

**LaTeX**

- 原始碼（可編輯）與真正編譯的 PDF 分開呈現，並可並排顯示；存檔後會自動重新編譯。

**其他格式**

- 圖片、PDF（pdf.js）、瀏覽器可播放的影音都有預覽。
- HTML 只會以原始碼顯示，下載時一律為附件。

## 3a. 語意搜尋（RAG 檢索，本地模型）

- **模型**：
  - 預設為 `multilingual-e5-small-q8`：`intfloat/multilingual-e5-small` 的 int8 量化版（ONNX 檔約 118 MB，384 維，支援中英文等 100 多種語言）。與原始 fp32 版相比，向量的 cosine 相似度約 0.997，排序結果幾乎相同。
  - 在 app 容器內用 CPU 執行（ONNX Runtime）。分詞器是自行實作的輕量 Unigram 編碼器，輸出與 Hugging Face tokenizers 逐一比對完全一致，但常駐記憶體從約 300 MB 降到約 20 MB。
  - 模型在**建置映像時下載**：固定版本，並驗證 SHA-256，之後放入映像。執行時設定 `HF_HUB_OFFLINE=1`，**不連網、不把任何內容送到外部**。
- **索引**：
  - **文字類檔案**（程式碼、Markdown、LaTeX、純文字等，預設上限 1 MiB）：解碼後依行切成約 500 字的片段，向量化後存進 SQLite（`rag_chunks`）。
  - **PDF**（預設上限 32 MiB）：交給隔離的 `latex` worker 容器，以 poppler 的 `pdftotext` 擷取文字，再按頁切成片段，搜尋結果會標示頁碼。
    - 不受信任的 PDF 不會在 app 容器內解析，並受到 worker 的時間、CPU 與記憶體限制。
    - 加密、損壞或掃描型（沒有文字層）的 PDF 會略過，並在設定頁的「略過」中計數。目前沒有 OCR。
  - 依 SHA-256 判斷是否需要重新索引。上傳、編輯、匯入（包括 CLI 匯入）後，約 30 秒內自動更新；刪除的檔案會一併移除。
  - 圖片、壓縮檔、Office 文件與其他二進位檔會略過。
- **權限**：
  - 敏感檔（`.env`、私鑰、憑證）**永不索引**。
  - 每筆搜尋結果都依呼叫者重新驗證權限：API Key 只會看到自己範圍內的結果，而且需要 `read` 權限。
- **使用**：
  - 網頁：`Ctrl+Shift+F`，或在指令面板輸入 `? 你的問題`。點選 PDF 的結果會直接跳到該頁。
  - API：`/api/v1/rag/search`（片段、相似度，以及行號或頁碼，見 `unit`），以及 `/api/v1/rag/context`（產生帶 `[n] 路徑:行號` 出處的段落，可直接貼給本地或外部的 LLM 當作 RAG 上下文）。詳見 [docs/API.md](docs/API.md)。
- **管理**：
  - 設定頁可看到模型、是否已載入、索引進度，並可「重建索引」。
  - CLI：`docker compose exec app mistbox rag status`、`mistbox rag reindex`、`mistbox rag models`。
- **換模型**：可選 `multilingual-e5-small`（原始 fp32，約 470 MB）、`multilingual-e5-base`（較準、較大）、`bge-small-zh`（中文為主）、`paraphrase-multilingual-minilm`（最小）。
  - 在 `.env` 設定 `MISTBOX_RAG_MODEL=<名稱>` 後執行 `docker compose up -d --build`，索引會自動重建。
  - Docker 使用 Microsoft 官方 ONNX Runtime CPU 共享函式庫（固定版本與 SHA-256），避免 Rust `ort` 預編譯套件的 AVX2 需求；沒有 AVX2 的 x86 CPU 也可保留語意搜尋。更新 Dockerfile 後執行 `docker compose up -d --build` 重新建置。
  - 設為 `off` 則完全停用，映像不會包含模型，伺服器也不會連結 ONNX Runtime。
- **記憶體**（預設模型，實測）：

  | 狀態 | app 容器 |
  |---|---|
  | 模型閒置已卸載（預設 5 分鐘無使用） | 約 50 MB |
  | 模型已載入／索引中 | 約 250–300 MB |
  | 上一版（fp32 + HF tokenizer，常駐） | 約 1.5 GB |

  - 為了壓低記憶體，ONNX Runtime 的記憶體池（arena）已關閉，每段文字也逐一推論。
  - 模型卸載後，下次搜尋或索引時會自動重新載入，約需 1 秒。閒置時間用 `MISTBOX_RAG_IDLE_UNLOAD_SECS` 調整，設為 0 代表常駐。
  - CPU 執行緒數由 `MISTBOX_RAG_THREADS` 控制（預設 2）。
  - 搜尋是對記憶體中的向量做暴力比對，個人規模（數萬個片段）只需數毫秒。

這裡只做「檢索」（找出相關段落）。沒有內建 LLM 生成回答；要做問答，請把 `/rag/context` 的輸出交給你自己的模型。

## 3b. 主題、背景圖片與自訂 CSS

- **主題**：淺色、深色、跟隨系統，每台裝置各自記住。
- **背景圖片**：在「設定 → 背景圖片」輸入網址。
  - 接受 `https://…`，或站內路徑（例如 `/theme/bg-demo.svg`）。
  - 也可在雲端中的圖片上按右鍵 →「設為背景圖片」。
  - 可調整遮罩深度與模糊程度。
  - 背景屬於伺服器設定，所有裝置共用。
  - 只有該網域會被加入 CSP 的 `img-src`；網址中不能有引號、括號或空白，因此無法注入 CSS。
- **自訂 CSS**：`theme/` 資料夾以唯讀方式掛載到容器的 `/app/theme`，內容如下。
  - `theme/custom.css`：你的樣式，前端最後載入，可以覆蓋一切。修改後在設定或指令面板中「重新載入自訂 CSS」即可，不需重建。
  - `theme/custom.demo.css`：範例集（換主色、字型、玻璃面板、卡片大小、語法配色、Markdown 樣式…）。
  - `theme/README.md`：**給你或其他 AI 的修改指南**，列出穩定的 CSS 變數（`--accent-*`、`--syn-*`、`--mb-bg-*`…）、區塊選擇器（`[data-region="explorer"]`、`.layout-ide`、`html[data-theme="dark"]`…）與 CSP 限制。
  - 字型與圖片請放在 `theme/`，並用 `/theme/檔名` 引用。外部 CSS 與字型會被 CSP 擋下。
  - `theme/` 不需登入即可讀取，請不要放私人檔案。

## 3c. 程式碼分析（靜態分析、重複程式碼）

不使用 LLM，也不執行任何上傳的程式碼：以 tree-sitter 解析語法樹，再用規則比對。
上傳或編輯後幾秒內自動在背景分析；敏感檔與垃圾桶不分析。

- **支援語言**：
  - C、C++（含 CUDA `.cu`、Arduino `.ino`；含 C++ 內容的 `.h` 會自動以 C++ 解析）、Python：符號、呼叫關係、規則檢查。
  - RP2040／RP2350 PIO 組合語言（`.pio`）：自寫的解析器與規則。
  - 其他常見語言（Rust、Go、JS/TS、Java…）：只做重複程式碼偵測。
- **規則**（完整清單與修正建議見 `GET /api/v1/code/rules`，或問題面板中點開一筆）：
  - **位元運算／位元欄位**：`x & MASK == 0` 優先順序、`1 << n - 1`、有號左移溢位（`1 << 31`）、位移超過型別寬度、遮罩結果和 1 比較、`&&` 搭配十六進位遮罩、未指定正負的 `int` 位元欄位、以 volatile 位元欄位對應硬體暫存器。
  - **RP2350／Pico SDK**：暫存器非原子讀-改-寫（建議 `hw_set_bits` / `hw_clear_bits`）、寫死 DMA 通道或 PIO 狀態機、忙碌等待 DMA／FIFO、CPU 逐字搬 PIO 資料（建議 DMA + DREQ）、中斷中修改非 volatile 全域變數、中斷中呼叫阻塞函式、在單精度 FPU 上使用 double。
  - **PIO**：超過 32 條指令、延遲超出（與 side-set 共用 5 位元）、缺少或超出範圍的 side-set、`set` 超過 31、未定義的 jmp 標籤、可改用 autopull／autopush。
  - **ROS 2**：callback 中 sleep、spin、同步等待服務回應、建立 publisher／subscription、以傳值接收訊息（rclcpp 與 rclpy）。
  - **平行計算**：OpenMP 迴圈中共享變數的資料競爭、偽共享、迴圈內 critical、條件分支中的 `__syncthreads()`、迴圈中反覆 `cudaMemcpy`、以 volatile 做執行緒同步、multiprocessing 缺少 `__main__` 保護、迴圈反覆建立 OpenMP 團隊或 Python 工作池、GPU 記憶體配置與裝置同步、提交工作後立即等待，以及 async 函式中的阻塞 sleep。
  - **一般效能**：迴圈條件中的 `strlen`、以 `pow` 算整數次方。
- **嚴重度**：⊗ 錯誤（幾乎確定是 bug）、⚠ 警告、⚡ 可優化、ⓘ 建議。
- **重複程式碼**：以 winnowing 指紋比對（與抄襲偵測工具 MOSS 同原理），改過變數名稱、常數或註解也找得到；約 50 個語彙單元（5–8 行）以上才列出。問題面板中按「並排」會左右開啟兩處比較。
- **在哪裡看**：IDE 版面的編輯器左側有標記，滑鼠移上去看說明；問題面板可切換「目前檔案／整個雲端」。
- **API**：`/api/v1/code/findings`、`/code/duplicates`、`/code/symbols`，見 [docs/API.md](docs/API.md)。

### 診斷／優化模式

按標題列的脈搏圖示，或 `Ctrl+Shift+D`（指令面板：「診斷／優化模式」），會在 IDE 開啟「診斷／優化」臨時分頁；重複開啟會聚焦同一個分頁。左側使用與一般程式碼相同的編輯器，貼上錯誤訊息或一段程式碼，`Ctrl+Enter` 分析，右側顯示結果。可切換「輸入／結果／分割」檢視，也能像一般分頁拖曳到其他編輯器群組。

語言選單可使用自動判斷或手動指定；分析後會顯示辨識結果並更新語法高亮。輸入或選項改變後，舊結果會提示重新分析。分頁不建立雲端檔案，也不提供儲存、下載或重新命名。
支援自動修正的程式碼會顯示「預覽修正」：確認修改前後內容後按「套用修正」，只更新左側暫存輸入，不會直接修改雲端檔案；可按「復原修正」撤回，或重新分析確認結果。目前支援簡單位元比較缺少括號，以及可確認迴圈本體沒有函式呼叫或透過指標／陣列寫入的 `strlen` 長度快取（`size_t` 單一索引從 0 遞增）。其他問題仍提供建議。

不保存任何紀錄：結果只留在這個分頁的記憶體裡，重新整理就清除；伺服器也不寫 log。

- **自動判斷**輸入是錯誤訊息還是程式碼（也可手動指定），並整理格式：去掉終端機顏色碼、共同縮排、貼上時帶進來的行號。
- **左半邊：對應的程式碼**（每處附前後幾行，按「開啟」跳到該行）
  - 錯誤位置：GCC／Clang、連結器、pioasm、CMake、Python traceback、GDB backtrace、Rust panic 的 `檔案:行號`，依路徑結尾比對雲端裡的檔案（本機路徑與雲端路徑不同也找得到）。
  - 定義：錯誤訊息裡提到的函式、變數（例如 `undefined reference to 'x'`）在哪裡定義。
  - 訊息出處：執行期訊息（ROS 2 log、例外訊息）在原始碼中印出或拋出的位置，數字與路徑等變動部分會先去掉。
  - 程式碼輸入：這段程式碼在雲端的哪裡（改過名稱也找得到）、意思相近的實作、它呼叫的函式與誰呼叫它，以及這段程式碼本身的靜態分析結果。
- **右半邊：解決方式與筆記**
  - 內建 62 種常見錯誤的說明與修法（不使用 LLM），偏重：
    - C／C++ 工具鏈：連結、標頭、型別範圍、位元欄位、strict aliasing、UBSan／ASan、堆積損壞。
    - Pico SDK：`undefined reference` 會直接指出要加哪個 `hardware_xxx` 函式庫與標頭；`.pio.h` 沒產生、RAM／flash 不足、picotool、板子平台、DMA／狀態機被占用、HardFault。
    - PIO、ROS 2（QoS、executor、TF、自訂訊息、DDS、套件與執行檔、節點重名）、OpenMP、執行緒與死結、CUDA、Python。
  - 你自己遇到的錯誤可以用筆記補上，見下方〈筆記標註〉。
  - 相關靜態分析規則的修正建議。新增的平行計算建議分為原因、建議與驗證，錯誤診斷則列出原因、修正與驗證。
  - 相似實作每個檔案只列最佳片段；已有直接匹配的檔案不再重複列成語意相似。相關筆記同一檔案的多個向量片段只加權一次，並合併重複標籤。相似度只供參考，不代表功能相同或已驗證可修正錯誤。
  - 新規則版本會自動重新分析既有程式檔。檢查會辨識明確 API／import、略過無法可靠解析的新規則輸入，並處理 OpenMP private／reduction 的空白及 atomic 前的註解，以降低誤判。
  - **相關筆記**：Markdown 筆記中提到規則代號（例如 `omp-race`、`bit-precedence`）、函式名稱、錯誤訊息或例外名稱就會被找到，顯示該段落（依標題切出）。啟用語意搜尋時，也會列出意思相近的筆記。
- **筆記標註**：在 Markdown 筆記開頭加上設定區塊，把筆記連到規則、錯誤或函式。預覽時這段會顯示成一排標籤。

  ```markdown
  ---
  rules: [omp-race, bit-precedence]      # 靜態分析規則代號（問題面板／規則目錄裡看得到）
  known: [undefined-reference]           # 內建常見錯誤的代號
  symbols: [dma_channel_configure]       # 函式、型別名稱
  errors: ["Failed to find a free participant index"]  # 錯誤訊息中的一段文字（不分大小寫）
  tags: [rp2350, dma]                    # 自由標籤
  ---
  # 筆記標題
  ```

  - 有標註的筆記在診斷模式中排在最前面，顯示標題下的第一段。
  - `errors` 是補充知識庫的方法：之後貼上的錯誤訊息只要包含這段文字，這篇筆記就會出現。
  - 鍵名也可以用中文：`規則`、`常見錯誤`、`函式`、`錯誤訊息`、`標籤`。
  - **一鍵新增**：問題面板展開一筆結果，或診斷模式的解決方式卡片上，都有「新增筆記」按鈕。它會在 `/筆記/規則/<代號>.md`（或 `/筆記/錯誤/`）建立已含標註與說明的範本並開啟；已有標註的筆記時，按鈕會變成「開啟筆記」。
  - API：`GET /api/v1/code/notes?rule=omp-race`（或 `known=`、`symbol=`）。
- **設定頁**的「程式碼分析與相似程式碼搜尋」顯示分析進度、各語言檔案數、筆記數、問題統計，以及程式碼模型是否已載入、向量數量與錯誤訊息，並可「重新分析」。
- **相似程式碼搜尋**使用程式碼專用的嵌入模型 `jina-embeddings-v2-base-code`（int8，約 160 MB，768 維）：
  - 每個函式／方法／PIO 程式一個片段，函式外的程式碼以 40 行為單位，向量以 int8 存在 SQLite 並載入記憶體比對。
  - 與文字模型一樣在建置映像時下載（固定版本、驗證 SHA-256），執行時不連網；第一次使用時載入，閒置後卸載（`MISTBOX_RAG_IDLE_UNLOAD_SECS`）。
  - 實測載入約 0.5 秒、記憶體約 225 MB；一段程式碼推論約 30 ms。
  - 使用同一套可攜式 ONNX Runtime，**不需要 AVX2**（已用模擬的 Nehalem CPU 驗證）。用 `docker compose exec app mistbox rag selftest` 可以在你的機器上確認兩個模型都能執行。
  - 不需要時在 `.env` 設 `MISTBOX_CODE_MODEL=off` 再 `docker compose up -d --build`，診斷模式其他功能照常。

## 4. 來源資料夾匯入

把主機上的資料夾（例如目前這個專案資料夾）匯入雲端。匯入期間**來源只以唯讀方式掛載**，並以 `O_RDONLY|O_NOFOLLOW` 開啟，不會移動、修改或刪除任何來源檔案。

```bash
SRC=/path/to/folder

# 1) 先預演：列出數量、總容量、新檔／未變更／衝突、敏感檔、符號連結、略過與排除項目
docker compose run --rm --no-deps --user "$(id -u):10001" \
  -v "$SRC":/import:ro app import /import --dest /匯入 --dry-run

# 2) 確認無誤後正式匯入（相同參數去掉 --dry-run）
docker compose run --rm --no-deps --user "$(id -u):10001" \
  -v "$SRC":/import:ro app import /import --dest /匯入 --exclude node_modules --exclude target
```

`--user "$(id -u):10001"` 讓匯入程序以你的主機帳號讀取來源（包括權限為 600 的檔案），同時屬於 app 的群組，能寫入共用的資料卷（資料目錄設有 setgid，檔案皆可由群組寫入）。

| 選項 | 說明 |
|---|---|
| `--dry-run` | 只分析，不寫入任何內容 |
| `--dest /路徑` | 雲端中的目的資料夾（不存在時自動建立） |
| `--include-sensitive` | 納入敏感檔；**預設略過並逐一列出**（見下方說明） |
| `--symlinks skip`（預設） | 不跟隨任何符號連結，逐一列出後略過 |
| `--symlinks follow-inside` | 只跟隨「解析後仍在來源資料夾內」的**檔案**連結，並保存目標內容的複本。資料夾連結、指向來源外、懸空的連結一律略過並列出 |
| `--on-conflict skip`（預設） | 同路徑已有**不同內容**的檔案時，保留雲端版本並略過來源檔 |
| `--on-conflict rename` | 另存為 `名稱 (1).副檔名` |
| `--on-conflict fail` | 只要有任何衝突，就在寫入前中止 |
| `--exclude GLOB` | 排除符合的相對路徑或名稱（可重複，支援 `*` `?`） |
| `--json` / `--show N` | 以 JSON 輸出完整報告 / 每類最多列出 N 項（0 = 全部） |

**敏感檔**：

- 判定條件：
  - 檔名：`.env`、`.env.*`（不含 `.example`、`.sample`、`.template`）、`*.pem`、`*.key`、`*.p12`、`*.pfx`、`*.crt` 等，以及 `id_rsa`、`id_ed25519`、`.netrc`、`.npmrc`、`credentials`、`*.kdbx` 等。
  - 內容：開頭含 PEM `PRIVATE KEY`。
- 預設**不匯入**，但一定會出現在報告中。
- 加上 `--include-sensitive` 才會匯入，並標記為「敏感」。
- 敏感檔不會公開（系統本來就沒有公開分享功能），也**不會被 API Key 看到**，除非該 Key 建立時勾選「允許敏感檔」。

**重複匯入與同名衝突**：

- 每個檔案都依「路徑＋SHA-256」比對。完全相同的視為「未變更」並略過，所以重複執行是安全且冪等的。
- 內容不同時依 `--on-conflict` 處理。**沒有任何會覆寫雲端檔案的模式**。
- 型別衝突（雲端是資料夾、來源是檔案，或反過來）會略過並列出。

**一律略過並列出的項目**：

- FIFO、socket、裝置檔
- 非 UTF-8 檔名
- 含 `/` `\` 或控制字元的檔名
- 含 `.mistbox-data` 標記的 Mistbox 資料目錄

讀取失敗（例如權限不足）會列在「錯誤」中，並以結束碼 2 結束。

匯入目前專案資料夾的範例：

```bash
docker compose run --rm --no-deps --user "$(id -u):10001" -v "$PWD":/import:ro app \
  import /import --dest /Mistbox --exclude node_modules --exclude target --exclude dist --dry-run
```

## 5. LaTeX

| 項目 | 內容 |
|---|---|
| 引擎 | **XeLaTeX**（預設）或 **pdfLaTeX**（在檔案開頭 30 行內加 `% !TEX program = pdflatex`），來自 Debian trixie 的 TeX Live |
| 參考文獻 | 自動執行 BibTeX（偵測 `\bibdata`）；交叉引用與目錄最多編譯 4 次 |
| 已安裝套件 | `texlive-latex-base/recommended/extra`、`texlive-fonts-recommended`、`texlive-lang-chinese`、`texlive-lang-cjk`、`texlive-science`、`texlive-pictures`、`texlive-plain-generic`、`lmodern` |
| 涵蓋範例 | ctex、xeCJK、amsmath、amssymb、amsthm、mathtools、siunitx、graphicx、tikz/pgf、booktabs、hyperref、geometry、listings、algorithm2e 等 |
| 中文字型 | Noto CJK（`fonts-noto-cjk`）＋ ctex 內建 Fandol 字型 |

**不支援**：

- biber/biblatex 的 biber 後端、minted（需要 shell escape）、LuaLaTeX、外部程式。
- 在套件清單以外的套件會顯示「缺少檔案或套件」錯誤。
- 引用的檔案只能位於主 `.tex` 所在資料夾或其子資料夾（不可使用 `..` 或絕對路徑）。

**送入編譯的檔案**：

- 只有主檔，以及它實際引用、存在且呼叫者有 read 權限的檔案：`\input`、`\include`、`\includegraphics`（含 `\graphicspath`）、`\bibliography`、`\addbibresource`、本地 `.sty`/`.cls`、`\lstinputlisting`。
- 找不到或未授權的檔案會列在 `missing`，並一起回報。

**隔離與資源限制**：

- 獨立 `latex` 容器：
  - 唯讀根檔案系統、`/tmp` 為 tmpfs、非 root、`cap_drop: ALL`、`no-new-privileges`。
  - **不掛載資料卷**，只連 `internal: true` 網路（無對外路由）。
  - `mem_limit 2g`、`cpus 2`、`pids_limit 128`。
- 每次編譯：
  - 使用全新暫存目錄，完成後刪除。
  - 加上 `-no-shell-escape`、`shell_escape=f`、`openin_any=p`、`openout_any=p`，並清空環境變數。
  - 設定 rlimit：CPU 45 秒、位址空間 1.5 GiB、輸出檔 50 MiB、檔案描述元、無 core dump。
  - 整體逾時 60 秒，逾時會終止整個程序群組。
- 所有套件與字型都在建置階段安裝，編譯時不會下載任何東西。
- 這些限制可在 `.env` 調整（`LATEX_*`）。
- 成功的 PDF 依「引擎＋所有輸入的 SHA-256」快取，內容未變更時不會重新編譯。

**失敗時**：介面會顯示錯誤分類與第一個錯誤行，附上編譯記錄末段，並把原始碼並排顯示。錯誤分類包括：語法錯誤、缺少檔案或套件、逾時、超過資源限制、輸出過大、服務不可用。

## 6. HTTPS 反向代理

Compose 只綁定 localhost。對外請在主機上使用反向代理處理 TLS。

**Caddy**（自動申請憑證）：

```caddyfile
drive.example.com {
    encode zstd gzip
    request_body {
        max_size 10GB
    }
    reverse_proxy 127.0.0.1:8725 {
        flush_interval -1
        transport http {
            read_timeout 1h
            write_timeout 1h
        }
    }
    header Strict-Transport-Security "max-age=31536000; includeSubDomains"
}
```

**nginx**：

```nginx
server {
    listen 443 ssl http2;
    server_name drive.example.com;
    ssl_certificate     /etc/letsencrypt/live/drive.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/drive.example.com/privkey.pem;
    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;

    client_max_body_size 10g;          # 與 MISTBOX_MAX_UPLOAD_BYTES 一致
    proxy_request_buffering off;       # 串流上傳，不先暫存到 nginx
    proxy_buffering off;
    proxy_read_timeout 3600s;
    proxy_send_timeout 3600s;

    location / {
        proxy_pass http://127.0.0.1:8725;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $remote_addr;   # 只傳遞直接連線的 IP
        proxy_set_header X-Forwarded-Proto https;
    }
}
```

使用反向代理時，在 `.env` 設定 `MISTBOX_TRUST_PROXY=true`，讓登入與 API 的限流以真實用戶端 IP 計算。只有在 8725 埠不對外開放時才可以這樣設定，否則他人可偽造 IP。

`MISTBOX_COOKIE_SECURE` 請保持 `true`。Session cookie 會以 `__Host-` 前綴、`Secure; HttpOnly; SameSite=Strict` 送出，`http://localhost` 也可正常使用。

## 7. 備份、還原、檢查與更新

**一致性線上備份**：

- 備份會取得資料庫的 `VACUUM INTO` 快照，加上快照所引用的全部檔案，輸出成 tar。
- 備份期間會暫停伺服器的檔案回收，因此快照引用的每個檔案在打包時都一定存在。服務不需停止。

```bash
mkdir -p backups
docker compose exec -T app mistbox backup - > "backups/mistbox-$(date +%F).tar"
tar tf backups/mistbox-*.tar | head       # mistbox-backup/manifest.json、mistbox.db、blobs/…
```

請把 tar 檔另外保存到其他磁碟或異地。備份檔**未加密**，含所有檔案內容。需要加密時請自行用 `age`、`gpg` 等成熟工具處理，例如 `| age -r <recipient> > backup.tar.age`。

**還原**（需先停止 app）：

```bash
docker compose stop app
docker compose run --rm -T --no-deps app restore --force - < backups/mistbox-2026-11-12.tar
docker compose run --rm --no-deps app verify    # 重新計算每個檔案的 SHA-256
docker compose start app
```

- 沒有 `--force` 時，若目標已有資料就拒絕還原。
- 加上 `--force` 時，既有資料會移到 volume 內的 `pre-restore-<時間>/`，不會刪除。確認還原無誤後可手動清理。
- 還原到全新 volume 時不需要 `--force`。

**完整性檢查與清理**：

```bash
docker compose exec app mistbox verify   # 遺失 / SHA-256 不符的檔案與未被引用的 blob
docker compose exec app mistbox gc       # 清除未被引用的 blob（例如當機時寫到一半的上傳）
```

**更新版本**：

```bash
docker compose exec -T app mistbox backup - > backups/before-update.tar   # 先備份
git pull
docker compose up -d --build              # 啟動時自動執行資料庫 migration
docker compose exec app mistbox verify
```

volume 位置：`docker volume inspect mistbox_mistbox-data`。

**不要**用 `docker compose down -v`，它會刪除資料卷。

## 8. API 與 API Key

完整說明見 [docs/API.md](docs/API.md)。

在網頁「API Keys」建立 Key 時可設定：

- 名稱與到期日
- `list` / `read` / `write` 權限
- 範圍：一個檔案，或一個資料夾（涵蓋其下所有層級，依請求當下的位置判定）
- 是否允許存取敏感檔

完整 Key 只顯示一次，伺服器只保存 SHA-256 雜湊與公開 ID。撤銷立即生效。介面會顯示建立、到期與最近使用時間。

```bash
H="Authorization: Bearer $MISTBOX_KEY"
curl -sS -H "$H" "https://drive.example.com/api/v1/fs/list?path=/"
curl -sS -H "$H" -T report.pdf "https://drive.example.com/api/v1/fs/upload?path=/report.pdf"
curl -sS -H "$H" -OJ "https://drive.example.com/api/v1/fs/download?path=/report.pdf"
```

## 9. 安全設計摘要

**儲存**

- 邏輯目錄存在 SQLite 中。檔案本體存放於 `blobs/<xx>/<伺服器產生的隨機 ID>`，使用者提供的名稱與路徑永遠不會拼接成主機路徑。
- 檔名禁止空白名稱、`.`、`..`、`/`、`\` 與控制字元，長度上限 255 bytes。
- 上傳採串流寫入暫存檔，同時計算 SHA-256，接著 fsync 並原子 rename，最後才寫入資料庫。失敗不會留下可見檔案，啟動時也會清空暫存區。
- 上傳遇到同名檔案回 409，不會覆寫。只有編輯器明確存檔時才會取代檔案內容，而且必須附上目前版本的 SHA-256（`If-Match`），舊版本會在同一個交易中排入回收。

**權限**

- 每個端點都重新驗證權限，包括列出、搜尋、下載、文字預覽、相對資源、LaTeX 資源。
- 範圍外的項目回 404，無法藉由猜測 UUID 或移動檔案越權。
- 分享連結與 API Key 使用同一套範圍與權限檢查，並且一律看不到敏感檔與垃圾桶，也不能使用任何管理功能。
  - 連結的密鑰放在網址的 fragment，只透過 `X-Share-Token` 標頭送出；資料庫只保存雜湊值。
  - 圖片、影音與下載這類瀏覽器自己發出的 GET 請求，使用每個分享各自的 HttpOnly、SameSite=Strict cookie，而且只有網址帶 `?share=<id>` 時才會採用；寫入操作一律需要標頭，因此不受 CSRF 影響。

**認證**

- 密碼使用 Argon2id；查無帳號時也執行等量計算，避免從回應時間推測帳號是否存在。
- Session token 只存放在 HttpOnly cookie，資料庫只保存 token 的雜湊。
- CSRF token 只存在記憶體，所有非 GET 請求都必須附上。
- 不使用 localStorage 保存任何憑證（只保存側欄寬度等介面偏好）。

**標頭**

- 前端套用嚴格 CSP（`script-src 'self'`，不允許外部資源），並設定 `X-Frame-Options: DENY`、`nosniff`、`Referrer-Policy: no-referrer`。
- 檔案內容回應附 `Content-Security-Policy: sandbox`。非白名單類型一律以 `application/octet-stream` 附件下載。

**限流與大小限制**

- 登入：每個 IP 每 15 分鐘 10 次。
- API：每個 IP 每分鐘 1200 次。
- JSON 請求 1 MiB；單檔上傳上限可設定（預設 10 GiB）。

**日誌**

- 只記錄方法、路徑、狀態碼與耗時。
- 不記錄 query string、標頭（含 Authorization）、密碼、Key 或檔案內容。

**容器**

- 以非 root（app：10001，latex：10002）執行，`read_only` 根檔案系統、`cap_drop: ALL`、`no-new-privileges`。
- 不掛載 Docker socket，也不常駐掛載任何主機目錄。
- 映像、Git 與前端 bundle 內都沒有秘密；`.env.example` 也不需要任何秘密。

**加密**

- 沒有自創加密演算法，**也沒有端對端加密**。
- 傳輸加密由 HTTPS 反向代理負責；靜態資料加密請使用磁碟或檔案系統層級的加密（例如 LUKS）。

## 10. 開發與測試

```bash
# 後端
cd server && cargo test                      # 單元＋整合測試（API、權限、匯入、備份、PDF 索引）
MISTBOX_DATA_DIR=/tmp/mb MISTBOX_STATIC_DIR=../frontend/dist cargo run -- serve

# 本機使用真正的向量模型（預設測試用的是詞彙雜湊）
cargo build --release --features embed
MISTBOX_RAG_MODEL_DIR=./models target/release/mistbox rag download-model
# 輕量分詞器與 Hugging Face tokenizers 的逐一比對（可指定任意文字資料夾當語料）
MISTBOX_TEST_TOKENIZER=$(find models -name tokenizer.json | head -1) MISTBOX_TEST_CORPUS=.. \
  cargo test --release --features embed --lib unigram

# 前端
cd frontend && npm ci && npm test && npm run build
npm run dev                                   # Vite，/api 代理到 127.0.0.1:8080

# 端對端驗收（實際啟動 Compose，使用獨立的 project 與 volume，結束後自動清除）
scripts/e2e.sh                                # 主機 DNS 特殊時：MISTBOX_BUILD_NETWORK=host scripts/e2e.sh
```

若主機使用 Tailscale exit node 等會攔截所有路由的設定，主機無法連到 Docker bridge 網段，`127.0.0.1:8725` 的轉發也會無回應。這時可以：

- 加上 `E2E_VIA_CONTAINER=1`：讓 HTTP 請求改由一個共用 app 網路命名空間的輔助容器發出，其餘步驟仍在主機上執行。
- 日常使用時，在 Tailscale 設定中允許 LAN 存取（`--exit-node-allow-lan-access`），或為 `172.16.0.0/12` 加上走 main table 的路由規則。

`samples/` 是可重現的預覽樣本：

- 中文 Markdown（表格、清單、數學、相對圖片）
- 各語言程式碼，以及 Big5、GBK、UTF-16 編碼
- 可成功編譯的 LaTeX 專案與各種錯誤案例（語法錯誤、缺套件、無窮迴圈、shell escape、讀取 `/etc/passwd`、管線輸入）
- 二進位檔、隱藏檔、無副檔名檔案、XSS 測試檔

二進位樣本可用 `python3 scripts/make-samples.py` 重新產生。
