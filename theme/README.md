# Mistbox 主題與自訂 CSS

這個資料夾會以**唯讀**方式掛載到容器的 `/app/theme`，並由伺服器在 `/theme/` 路徑提供：

| 檔案 | 用途 |
|---|---|
| `custom.css` | 你的樣式。前端會把它當成**最後一個**樣式表載入，因此可以覆蓋所有內建樣式 |
| `custom.demo.css` | 範例集：換主色、字型、玻璃面板、卡片大小、語法配色等，可整段複製到 `custom.css` |
| 其他檔案 | 圖片、字型等，用 `/theme/檔名` 引用，例如 `/theme/bg.jpg`、`/theme/fonts/x.woff2` |

修改後在網頁「設定 → 自訂 CSS → 重新載入 custom.css」，或在指令面板（`Ctrl+Shift+P`）執行「重新載入自訂 CSS」即可生效，不需要重建映像。

背景圖片請在「設定 → 背景圖片」輸入網址：

- 可以是 `https://…` 外部網址，或 `/theme/bg.jpg` 這類站內路徑。
- 也可以在雲端中的圖片上按右鍵 →「設為背景圖片」。
- 遮罩深度與模糊程度也在同一處調整。

## 限制（安全性）

- 頁面的 CSP 只允許站內的樣式、字型與圖片。
  - CSS 裡的 `@import url(https://…)`、外部字型、外部圖片都會被瀏覽器阻擋。
  - 請把需要的檔案放進這個資料夾。
  - 唯一的例外是「設定」中指定的背景圖片網域。
- 自訂 CSS 無法執行 JavaScript。
- `/theme/` 下的檔案**不需登入**即可讀取。請不要放私人檔案。

## 穩定的掛勾（hooks）

下列名稱視為公開介面，後續版本會盡量保持不變。

### 根元素屬性

| 選擇器 | 說明 |
|---|---|
| `html[data-theme="light"]` / `html[data-theme="dark"]` | 目前實際套用的主題（「跟隨系統」時依系統設定而定） |
| `html[data-theme-pref="system"]` | 使用者的主題偏好（`light` / `dark` / `system`） |
| `html[data-has-bg]` | 已設定背景圖片時存在 |
| `.app.layout-drive` / `.app.layout-ide` | 目前的版面 |

### 區塊（`data-region`）

| 值 | 位置 |
|---|---|
| `app` | 整個應用程式 |
| `topbar` | 頂部列（含指令中心按鈕） |
| `drive` / `sidebar` / `drive-main` / `overlay` | 雲端硬碟版面：整體、左側欄、主區域、全螢幕預覽 |
| `ide` / `explorer` / `explorer-actions` / `editor` / `statusbar` | IDE 版面：整體、檔案總管、底部操作列、編輯區、狀態列 |
| `preview` | 檔案預覽／編輯器 |
| `uploads` | 上傳進度（雲端硬碟為浮動面板 `.floating`，IDE 為檔案總管內 `.embedded`） |
| `palette` / `context-menu` | 指令面板、右鍵選單 |

### 常用 class

- 共用元件：`.btn` `.btn.primary` `.btn.ghost` `.badge` `.segmented` `.modal` `.ctx-menu` `.palette`
- 雲端硬碟版面：`.new-btn` `.folder-card` `.file-card` `.file-card .thumb` `.section-title` `.drive-crumbs` `.side-row`
- IDE 版面：`.tree-row`（`.kind-dir` / `.kind-file`、`.selected`、`.current`）、`.tabs` `.tab.on` `.welcome`
- 預覽與編輯：`.markdown-body`（Markdown 渲染結果）、`.code-view` `.cm-editor`（CodeMirror）、`.pdf-pages`、`.viewer-toolbar`
- 檔案圖示：`.ficon` `.ficon-folder` `.ficon-md` `.ficon-code` …

## 設計變數

完整清單在 `frontend/src/styles.css` 開頭。最常用的如下。

| 變數 | 說明 |
|---|---|
| `--bg` `--surface` `--surface-2` `--panel` `--hover` | 背景與面板層次 |
| `--line` `--line-strong` | 邊框 |
| `--text` `--text-2` `--text-3` | 主要／次要／輔助文字 |
| `--accent-50` … `--accent-800` | 主色色階（預設天藍） |
| `--primary-bg` `--primary-bg-hover` `--primary-fg` | 主要按鈕與狀態列 |
| `--selection` `--selection-strong` | 選取狀態 |
| `--link` `--danger` `--warn` `--ok` | 連結與狀態色 |
| `--radius` `--radius-lg` `--shadow` `--shadow-lg` | 圓角與陰影 |
| `--sans` `--mono` `--code-font-size` | 字型 |
| `--icon-folder` | 資料夾圖示的線條顏色 |
| `--icon-line` | 檔案圖示的頁面外框顏色 |
| `--icon-code` `--icon-md` `--icon-pdf` `--icon-image` … | 頁面內小符號（依檔案類型）的顏色 |
| `--hover` `--selection` `--tint` `--accent-line` | 滑過、選取的淡色底與強調細線（簡約線條風格） |
| `--code-bg` `--code-gutter-bg` `--code-active-line` `--code-selection` | 編輯器 |
| `--syn-keyword` `--syn-string` `--syn-number` `--syn-comment` `--syn-function` `--syn-type` `--syn-property` `--syn-variable` `--syn-punct` `--syn-meta` `--syn-tag` `--syn-heading` `--syn-link` | 語法高亮；CodeMirror 與 Markdown 程式碼區塊共用 |
| `--mb-bg-image` `--mb-bg-dim` `--mb-bg-blur` | 背景圖、整體不透明度、模糊（由設定寫入；透明度 100% 對應不透明度 0） |
| `--mb-bg-overlay` | 遮罩顏色（RGB 三元組） |
| `--glass` | 有背景時面板的不透明度，預設跟隨 `--mb-bg-dim`，初始為 0 |

深色模式的覆寫請寫在 `html[data-theme='dark'] { … }` 內。

Logo 配色可在設定中獨立選擇深色或淺藍色，儲存於目前裝置，並同步套用至分頁圖示。
