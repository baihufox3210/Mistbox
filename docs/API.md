# Mistbox API v1

Base URL：`https://<你的網域>/api/v1`（本機：`http://127.0.0.1:8080/api/v1`）

## 認證

| 方式 | 用途 | 傳遞方式 |
|---|---|---|
| **API Key** | 腳本、自動化 | `Authorization: Bearer mbk_<id>_<secret>` |
| 管理者 session | 網頁介面 | HttpOnly cookie＋`X-CSRF-Token` 標頭（所有非 GET 請求） |

- API Key **只能**放在 `Authorization` 標頭；放在網址參數一律無效（401），伺服器也不會記錄 Authorization 標頭。
- API Key 不能呼叫 `/auth/*`、`/keys*`（403）。
- 只要帶了 `Authorization` 標頭，就只會以 Key 驗證，不會退回 cookie。

### API Key 權限模型

| 權限 | 允許的操作 |
|---|---|
| `list` | 列出資料夾、搜尋、查詢檔案資訊、解析相對路徑 |
| `read` | 下載內容、文字預覽、Markdown 相對資源、LaTeX 編譯與 PDF；也可查詢資訊 |
| `write` | 上傳、建立資料夾；在範圍內重新命名、移動、刪除 |

**範圍（scope）**：每把 Key 綁定一個節點 ID。

- **資料夾範圍**：涵蓋該資料夾本身與其下**所有層級**的後代。判定方式是每次請求時，沿著項目**當下**的上層鏈檢查。項目被移出範圍後立即無法存取，移入後即可存取。
- **檔案範圍**：只涵蓋那一個檔案。
- Key 看到的路徑都以範圍為根（`/`）。範圍以外的上層名稱不會外洩。`root` 這個 ID 對 Key 而言就是它的範圍節點。
- 範圍外或被隱藏的項目一律回 **404**（不回 403），避免以猜測 ID 探測項目是否存在。範圍內但缺少權限旗標則回 **403**。
- Key 不能重新命名、移動或刪除自己的範圍根。移動時，來源與目的地都必須在範圍內。
- 標記為「敏感」的檔案（`.env`、私鑰、憑證…）只有在建立 Key 時勾選 `allow_sensitive` 才看得到。

撤銷立即生效。過期或撤銷的 Key 回 `401 invalid_api_key`。

## 錯誤格式

所有錯誤都是 JSON：

```json
{ "error": { "code": "not_found", "message": "找不到項目，或沒有存取權限" } }
```

部分錯誤附帶額外欄位（例如 `retry_after`、`size`、`limit`、`log`、`missing`）。

| HTTP | code | 說明 |
|---|---|---|
| 400 | `bad_request`, `invalid_name` | 參數錯誤；名稱為空、`.`、`..`，或含 `/` `\` 控制字元，或超過 255 bytes |
| 400 | `upload_incomplete` | 上傳中斷，沒有留下任何內容 |
| 401 | `unauthorized`, `invalid_api_key`, `invalid_credentials` | 未登入；Key 無效、過期或已撤銷 |
| 403 | `forbidden`, `csrf` | 權限不足；缺少 CSRF token |
| 404 | `not_found`, `no_route`, `latex_not_compiled` | 不存在或不在授權範圍內 |
| 409 | `conflict` | 同名項目已存在（**永不靜默覆寫**） |
| 413 | `upload_too_large`, `preview_too_large`, `latex_input_too_large` | 超過大小限制 |
| 422 | `checksum_mismatch` | `X-Content-SHA256` 與實際內容不符，檔案未儲存 |
| 422 | `latex_failed`, `latex_missing_file`, `latex_timeout`, `latex_resource_limit`, `latex_output_too_large` | LaTeX 編譯失敗（附 `log`、`missing`） |
| 429 | `rate_limited` | 請求過於頻繁（附 `retry_after` 秒數） |
| 500 | `internal`, `blob_missing` | 伺服器錯誤 |
| 412 | `version_conflict` | 覆寫內容時 `If-Match` 不是目前版本 |
| 428 | `precondition_required` | 覆寫內容時缺少 `If-Match` |
| 503 | `latex_disabled`, `latex_unavailable`, `latex_busy`, `rag_disabled`, `rag_not_ready` | LaTeX 或語意搜尋服務停用、無法連線、忙碌，或模型尚未就緒 |

## 節點物件

```json
{
  "id": "6f1c…",            // 隨機 UUID（磁碟路徑由伺服器另行產生，與此無關）
  "parent_id": "…",         // 呼叫者的根節點為 null
  "name": "報告 2026.pdf",
  "kind": "file",           // "file" | "dir"
  "size": 182044,
  "mime": "application/pdf",
  "sha256": "9b1d…",
  "sensitive": false,
  "created_at": "2026-11-12T08:00:00Z",
  "modified_at": "2026-11-12T08:00:00Z",
  "path": "/文件/報告 2026.pdf"   // 相對於呼叫者的根
}
```

## 以路徑操作（最適合 curl）

路徑採 POSIX 格式，以呼叫者的根為 `/`，不接受 `..`。

| 方法 | 端點 | 權限 | 說明 |
|---|---|---|---|
| GET | `/fs/list?path=/dir` | list | 列出資料夾：`{node, breadcrumbs, children}` |
| GET | `/fs/info?path=/a.txt[&stats=1]` | list 或 read | 節點資訊；資料夾加 `stats=1` 會回傳檔案數與總大小 |
| GET | `/fs/download?path=/a.txt` | read | 下載（支援 `Range`） |
| PUT/POST | `/fs/upload?path=/dir/a.txt[&on_conflict=fail\|rename][&mtime=<unix秒>][&mkdirs=0]` | write | 串流上傳原始 body，預設自動建立上層資料夾 |
| POST | `/fs/mkdir?path=/a/b` | write | 建立資料夾（含上層） |

## 以 ID 操作

| 方法 | 端點 | 權限 | 說明 |
|---|---|---|---|
| GET | `/whoami` | 任一 | 呼叫者資訊（Key 的範圍、權限、到期時間） |
| GET | `/nodes/{id}` | list 或 read | 節點資訊（`?stats=1`） |
| GET | `/nodes/{id}/children` | list | 列出資料夾 |
| POST | `/nodes/{id}/children` | write | 建立資料夾，body：`{"name":"…"}` |
| POST/PUT | `/nodes/{id}/files?name=…` 或 `?path=a/b/c.txt` | write | 上傳到資料夾（`on_conflict`、`mtime`） |
| GET | `/nodes/{id}/content[?inline=1]` | read | 下載；`inline=1` 只對安全的圖片、PDF、影音生效 |
| PATCH | `/nodes/{id}` | write | `{"name":"新名稱"}`、`{"parent_id":"目的資料夾"}`；`sensitive` 只限管理者 |
| DELETE | `/nodes/{id}[?permanent=1]` | write | 移到垃圾桶（資料夾含所有內容），回傳 `{permanent, stats}`。`permanent=1` 或對垃圾桶內的項目執行時為永久刪除（只限管理者，API Key 回 403） |
| GET | `/search?q=…&limit=100` | list | 檔名子字串搜尋，只搜尋授權範圍內 |
| GET | `/nodes/{id}/text[?encoding=Big5][&partial=1]` | read | 文字預覽（自動偵測編碼；超過上限回 413，`partial=1` 只取開頭） |
| GET | `/nodes/{id}/resolve?path=../img/a.png` | list 或 read | 以檔案所在資料夾為基準解析相對路徑 |
| GET | `/nodes/{id}/relative?path=img/a.png` | read（目標） | 取得相對資源內容（Markdown 圖片） |
| POST | `/nodes/{id}/latex` | read | 在隔離容器編譯 .tex：`{ok, engine, files, missing, pdf_url}` |
| GET | `/nodes/{id}/latex/pdf` | read | 取得最近一次編譯（內容未變更）的 PDF |
| PUT | `/nodes/{id}/content` | write | **覆寫內容**（編輯器使用）。必須帶 `If-Match: "<目前版本的 sha256>"`：缺少時回 428，版本不符時回 412（`version_conflict`），不會覆寫 |
| GET | `/rag/status` | 任一 | 語意搜尋狀態：模型、已索引檔案數、片段數、待處理數 |
| GET | `/rag/search?q=…&k=10` | read | 語意搜尋（本地向量模型，含 PDF 內文）。每個檔案最多 3 個片段：`{results:[{node, score, seq, line_start, line_end, unit, text}]}`；`unit` 為 `line`（行號）或 `page`（PDF 頁碼，此時 line_start = line_end = 頁碼） |
| GET | `/rag/context?q=…&k=5&max_chars=6000` | read | 產生給 LLM 的 RAG 上下文：`{context:"[1] /path:1-20\n…", sources:[…]}`（PDF 為 `/path.pdf:p.3`） |

管理者專用（只限 session，須帶 CSRF）：

| 方法 | 端點 | 說明 |
|---|---|---|
| POST | `/auth/login` | `{"username","password"}` → 設定 cookie，回傳 `csrf_token` |
| POST | `/auth/logout` | 登出 |
| GET | `/auth/me` | 目前使用者與 `csrf_token` |
| POST | `/auth/password` | `{"current_password","new_password"}`，會登出所有其他 session |
| GET | `/keys` | 列出 Key（不含 secret） |
| POST | `/keys` | `{"name","permissions":{"list","read","write"},"scope_node_id","expires_at":RFC3339\|null,"allow_sensitive"}` → `{"key": 完整值（只顯示這一次）, "api_key": …}` |
| DELETE | `/keys/{id}` | 撤銷（保留紀錄） |
| GET / PUT | `/settings` | 介面設定：`{"background_url","background_dim","background_blur"}`。背景網址只接受 https URL 或站內路徑，不可含引號、括號、空白 |
| POST | `/rag/reindex` | 清除並在背景重建語意索引 |
| GET | `/trash` | 垃圾桶內容：原名稱、原路徑、`trashed_at`、`purge_at` |
| POST | `/trash/{id}/restore` | 還原到原位置（同名時自動編號；原資料夾不存在則還原到根目錄） |
| DELETE | `/trash` | 清空垃圾桶（永久刪除） |

`GET /health`：不需認證，回傳 `{"status":"ok"}`。

## 上傳與完整性

- 上傳 body 是**原始位元組**（不是 multipart），伺服器會串流寫入暫存檔並同時計算 SHA-256。寫完會 fsync，再以原子 rename 提交，最後才寫入資料庫。上傳失敗或中斷不會留下可見的半成品。
- 可帶 `X-Content-SHA256: <hex>` 請伺服器驗證，不符時回 422，不會儲存。
- 下載回應附 `X-Content-SHA256` 與 `ETag`。
- 同名衝突預設回 409。`on_conflict=rename` 會另存為 `名稱 (1).副檔名`。沒有任何會覆寫既有檔案的選項。

## 下載回應標頭

`Content-Type`（非內嵌時一律 `application/octet-stream`）、`Content-Disposition`（含 RFC 5987 `filename*` UTF-8 檔名）、`X-Content-Type-Options: nosniff`、`Content-Security-Policy: sandbox; …`、`Accept-Ranges: bytes`。HTML、JS 等可執行內容永遠不會內嵌。SVG 只在 CSP sandbox 下內嵌（腳本不會執行）。

## curl 範例

```bash
export MB=https://drive.example.com
export MISTBOX_KEY='mbk_0123456789abcdef_…'      # 在網頁「API Keys」建立
H="Authorization: Bearer $MISTBOX_KEY"

# 查詢自己的範圍與權限
curl -sS -H "$H" "$MB/api/v1/whoami"

# 列出（範圍根目錄 / 子資料夾）
curl -sS -H "$H" "$MB/api/v1/fs/list?path=/"
curl -sS -H "$H" "$MB/api/v1/fs/list?path=/2026/%E5%A0%B1%E5%91%8A" | jq -r '.children[] | "\(.kind)\t\(.size)\t\(.name)"'

# 上傳（串流；附 SHA-256 驗證；同名時另存新名）
curl -sS -H "$H" -H "X-Content-SHA256: $(sha256sum report.pdf | cut -d' ' -f1)" \
     -T report.pdf "$MB/api/v1/fs/upload?path=/2026/report.pdf&on_conflict=rename"

# 下載（-OJ 使用伺服器提供的檔名；-C - 可續傳）
curl -sS -H "$H" -OJ "$MB/api/v1/fs/download?path=/2026/report.pdf"

# 查詢資訊並驗證下載內容
curl -sS -H "$H" "$MB/api/v1/fs/info?path=/2026/report.pdf" | jq -r .sha256
sha256sum report.pdf

# 搜尋（檔名）
curl -sS -H "$H" "$MB/api/v1/search?q=report" | jq -r '.results[].path'

# 語意搜尋（內容相近，需要 read 權限）
curl -sS -G -H "$H" "$MB/api/v1/rag/search" --data-urlencode "q=如何設定反向代理" --data-urlencode "k=5" \
  | jq -r '.results[] | "\(.score)\t\(.node.path):\(.line_start)"'

# 取得 RAG 上下文，交給本地 LLM（例如 Ollama）回答
CTX=$(curl -sS -G -H "$H" "$MB/api/v1/rag/context" --data-urlencode "q=備份要怎麼還原？" | jq -r .context)
printf '根據以下資料回答，並標註出處編號：\n%s\n\n問題：備份要怎麼還原？' "$CTX" | ollama run qwen2.5

# 編輯檔案內容（樂觀鎖：必須帶目前版本的 SHA-256）
SHA=$(curl -sS -H "$H" "$MB/api/v1/fs/info?path=/notes.md" | jq -r .sha256)
ID=$(curl -sS -H "$H" "$MB/api/v1/fs/info?path=/notes.md" | jq -r .id)
curl -sS -H "$H" -H "If-Match: \"$SHA\"" -X PUT --data-binary @notes.md "$MB/api/v1/nodes/$ID/content"
```

路徑中的中文或空白要做 URL 編碼（例如 `jq -rn --arg s '/中文 檔.txt' '$s|@uri'`），或用 `curl -G --data-urlencode "path=/中文 檔.txt"`。
