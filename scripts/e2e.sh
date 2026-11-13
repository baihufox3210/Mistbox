#!/usr/bin/env bash
# End-to-end acceptance test against the real Docker Compose deployment.
#
#   scripts/e2e.sh            # uses an isolated compose project "mistbox-e2e" on port 18080
#   E2E_VIA_CONTAINER=1 scripts/e2e.sh
#       Send HTTP requests from a helper container sharing the app's network
#       namespace (for hosts whose routing cannot reach Docker bridges, e.g. a
#       Tailscale exit node). Everything else still runs on the host.
#
# Requires: docker compose, curl, jq, sha256sum. Creates (and at the end removes)
# its own volume; never touches the default "mistbox" project data.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
export COMPOSE_PROJECT_NAME=${COMPOSE_PROJECT_NAME:-mistbox-e2e}
export MISTBOX_PORT=${MISTBOX_PORT:-18080}
export LATEX_TIMEOUT_SECS=${LATEX_TIMEOUT_SECS:-20}
export LATEX_CPU_SECS=${LATEX_CPU_SECS:-15}
export MISTBOX_RAG_IDLE_UNLOAD_SECS=${MISTBOX_RAG_IDLE_UNLOAD_SECS:-20}
BASE="http://127.0.0.1:${MISTBOX_PORT}"
DC="docker compose"
WORK=$(mktemp -d)
JAR="$WORK/cookies"
PASS="e2e-$(head -c 12 /dev/urandom | od -An -tx1 | tr -d ' \n')"
KEEP=${KEEP:-0}

pass=0; fail=0
ok()   { pass=$((pass+1)); printf '  \033[32m✓\033[0m %s\n' "$*"; }
bad()  { fail=$((fail+1)); printf '  \033[31m✗ %s\033[0m\n' "$*"; }
check() { local d=$1; shift; if "$@"; then ok "$d"; else bad "$d"; fi; }
section() { printf '\n\033[1m== %s\033[0m\n' "$*"; }

VIA=${E2E_VIA_CONTAINER:-0}
CLIENT="${COMPOSE_PROJECT_NAME}-client"
client_up() {
  [[ $VIA == 1 ]] || return 0
  docker rm -f "$CLIENT" >/dev/null 2>&1 || true
  docker run -d --name "$CLIENT" --network "container:$($DC ps -q app)" \
    -v "$ROOT:$ROOT" -v "$WORK:$WORK" mistbox-e2e-client sleep infinity >/dev/null
}
client_down() { [[ $VIA == 1 ]] && docker rm -f "$CLIENT" >/dev/null 2>&1 || true; }
if [[ $VIA == 1 ]]; then
  docker build -q --network host -t mistbox-e2e-client - >/dev/null <<'DOCKERFILE'
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates && rm -rf /var/lib/apt/lists/*
DOCKERFILE
  BASE="http://127.0.0.1:8080"
  curl() { docker exec -w "$PWD" "$CLIENT" curl "$@"; }
  export -f curl
  export CLIENT
fi

cleanup() {
  client_down
  if [[ $KEEP != 1 ]]; then $DC down -v --remove-orphans >/dev/null 2>&1 || true; fi
  rm -rf "$WORK"
}
trap cleanup EXIT

wait_healthy() {
  for _ in $(seq 1 60); do
    curl -fsS --max-time 3 "$BASE/api/v1/health" >/dev/null 2>&1 && return 0
    sleep 1
  done
  echo "server did not become healthy"; $DC logs app | tail -30; exit 1
}

csrf=""
login() {
  csrf=$(curl -fsS -c "$JAR" -H 'content-type: application/json' \
    -d "{\"username\":\"admin\",\"password\":\"$PASS\"}" "$BASE/api/v1/auth/login" | jq -r .csrf_token)
  export csrf
}
# Authenticated (session) request helpers.
sget()  { curl -sS -b "$JAR" "$@"; }
spost() { curl -sS -b "$JAR" -H "x-csrf-token: $csrf" "$@"; }
status() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
urlenc() { jq -rn --arg s "$1" '$s|@uri'; }
node_id() { sget "$BASE/api/v1/fs/info?path=$(urlenc "$1")" | jq -r .id; }
export -f sget spost status urlenc node_id
export BASE JAR DC

section "啟動 Docker Compose（project=$COMPOSE_PROJECT_NAME, port=$MISTBOX_PORT）"
$DC up -d --build >/dev/null 2>&1 || { $DC up -d --build; exit 1; }
client_up
wait_healthy
ok "服務健康檢查通過"
check "Compose 只綁定 127.0.0.1" bash -c "$DC port app 8080 | grep -q '^127.0.0.1:'"
check "latex 網路為 internal（無對外路由）" bash -c "docker network inspect ${COMPOSE_PROJECT_NAME}_latex-internal | jq -e '.[0].Internal == true' >/dev/null"
check "app 容器以非 root 執行" bash -c "[[ \$($DC exec -T app id -u) != 0 ]]"
check "latex 容器以非 root 執行" bash -c "[[ \$($DC exec -T latex id -u) != 0 ]]"
check "app 根檔案系統唯讀" bash -c "! $DC exec -T app sh -c 'touch /usr/x' 2>/dev/null"

section "建立管理者（不可有預設密碼）"
check "未建立帳號前無法登入" bash -c "[[ \$(status -H 'content-type: application/json' -d '{\"username\":\"admin\",\"password\":\"admin\"}' $BASE/api/v1/auth/login) == 401 ]]"
printf '%s\n' "$PASS" | $DC exec -T app mistbox admin create --username admin --password-stdin >/dev/null 2>&1
check "重複建立管理者被拒絕" bash -c "! printf 'x%.0s' {1..12} | $DC exec -T app mistbox admin create --username other --password-stdin >/dev/null 2>&1"
login
[[ -n $csrf && $csrf != null ]] && ok "登入成功，取得 CSRF token" || { bad "登入失敗"; exit 1; }
check "Session cookie 有 HttpOnly / SameSite=Strict / Secure" bash -c "curl -sS -D - -o /dev/null -H 'content-type: application/json' -d '{\"username\":\"admin\",\"password\":\"$PASS\"}' $BASE/api/v1/auth/login | grep -i '^set-cookie' | grep -q 'HttpOnly; SameSite=Strict.*Secure'"

section "來源匯入（唯讀、dry-run、敏感檔、符號連結）"
SRC="$WORK/source"
cp -a samples "$SRC"
printf 'API_TOKEN=secret\n' > "$SRC/.env"
ln -s ../misc/no-extension "$SRC/code/link-inside"
ln -s /etc/hostname "$SRC/code/link-outside"
chmod -R a+rX "$SRC"
before=$(cd "$SRC" && find . -printf '%p %s %T@ %m\n' | sort | sha256sum)
IMPORT="$DC run --rm -T --no-deps --user $(id -u):10001 -v $SRC:/import:ro app import /import --dest /samples --symlinks follow-inside"
dry=$($IMPORT --dry-run --show 0)
echo "$dry" | sed 's/^/    /' | head -40
check "dry-run 未寫入任何檔案" bash -c "[[ \$(sget '$BASE/api/v1/fs/info?path=/samples' -o /dev/null -w '%{http_code}') == 404 ]]"
check "dry-run 列出敏感檔 .env" grep -q '\.env' <<<"$dry"
check "dry-run 列出指向來源外的符號連結" grep -q 'link-outside.*來源之外' <<<"$dry"
$IMPORT --show 5 >/dev/null
after=$(cd "$SRC" && find . -printf '%p %s %T@ %m\n' | sort | sha256sum)
check "匯入後來源資料夾完全未變動" test "$before" == "$after"
check "未加 --include-sensitive 時 .env 未匯入" bash -c "[[ \$(sget '$BASE/api/v1/fs/info?path=/samples/.env' -o /dev/null -w '%{http_code}') == 404 ]]"
again=$($IMPORT --show 0)
check "重複匯入：全部視為未變更" grep -q '新檔案：0' <<<"$again"

section "上下載 SHA-256 一致（中文檔名、隱藏檔、二進位、多層目錄）"
mismatch=0; count=0
while IFS= read -r -d '' f; do
  rel=${f#samples/}
  remote="/samples/$rel"
  want=$(sha256sum "$f" | cut -d' ' -f1)
  got=$(sget "$BASE/api/v1/fs/download?path=$(urlenc "$remote")" | sha256sum | cut -d' ' -f1)
  count=$((count+1))
  [[ $want == "$got" ]] || { mismatch=$((mismatch+1)); echo "    mismatch: $rel"; }
done < <(find samples -type f -print0)
check "全部 $count 個樣本下載後 SHA-256 與原檔一致" test $mismatch -eq 0
check "隱藏檔可下載" bash -c "[[ \$(sget '$BASE/api/v1/fs/download?path=$(urlenc /samples/misc/.hidden-file)') == 'hidden file content' ]]"
check "符號連結（來源內）以複本保存" bash -c "[[ \$(sget '$BASE/api/v1/fs/download?path=/samples/code/link-inside' | sha256sum) == \$(sha256sum < samples/misc/no-extension) ]]"

# Web upload with checksum header.
dd if=/dev/urandom of="$WORK/big.bin" bs=1M count=200 status=none
want=$(sha256sum "$WORK/big.bin" | cut -d' ' -f1)
code=$(spost -o "$WORK/up.json" -w '%{http_code}' -H "x-content-sha256: $want" -T "$WORK/big.bin" "$BASE/api/v1/fs/upload?path=/大檔案/big%20file.bin")
check "200 MiB 串流上傳（附 X-Content-SHA256）" test "$code" == 201
got=$(sget "$BASE/api/v1/fs/download?path=$(urlenc '/大檔案/big file.bin')" | sha256sum | cut -d' ' -f1)
check "200 MiB 下載 SHA-256 一致" test "$want" == "$got"
mem=$(docker stats --no-stream --format '{{.MemUsage}}' "$($DC ps -q app)" | cut -d/ -f1)
echo "    app 記憶體用量（大檔上傳後）：$mem"
code=$(spost -o /dev/null -w '%{http_code}' -H "x-content-sha256: $(printf 0%.0s {1..64})" --data-binary 'abc' "$BASE/api/v1/fs/upload?path=/bad-checksum.txt")
check "SHA-256 不符時拒絕並不留下檔案 (422)" bash -c "[[ $code == 422 && \$(sget -o /dev/null -w '%{http_code}' '$BASE/api/v1/fs/info?path=/bad-checksum.txt') == 404 ]]"
code=$(spost -o /dev/null -w '%{http_code}' --data-binary 'x' "$BASE/api/v1/fs/upload?path=/samples/misc/binary.bin")
check "同名上傳不覆寫 (409)" test "$code" == 409
check "預覽超限：200 MiB 文字預覽回 413 preview_too_large" bash -c "[[ \$(sget '$BASE/api/v1/nodes/$(node_id '/大檔案/big file.bin')/text' | jq -r .error.code) == preview_too_large ]]"

section "文字 / 編碼預覽"
id=$(node_id "/samples/encoding/big5-繁體.txt")
check "Big5 檔案標示為非 UTF-8" bash -c "[[ \$(sget '$BASE/api/v1/nodes/$id/text' | jq -r .valid_utf8) == false ]]"
check "選擇 Big5 編碼後正確解碼" bash -c "sget '$BASE/api/v1/nodes/$id/text?encoding=Big5' | jq -r .text | grep -q '繁體中文'"
id=$(node_id "/samples/code/hello.c")
check "C 原始碼 Tab 與中文原樣保留" bash -c "[[ \$(sget '$BASE/api/v1/nodes/$id/text' | jq -r .text) == \"\$(cat samples/code/hello.c)\" ]]"
id=$(node_id "/samples/misc/binary.bin")
check "二進位檔案不以文字預覽" bash -c "[[ \$(sget '$BASE/api/v1/nodes/$id/text' | jq -r .binary) == true ]]"

section "Markdown 相對資源需授權"
md=$(node_id "/samples/markdown/README.md")
check "相對圖片可透過 /relative 取得" bash -c "[[ \$(sget '$BASE/api/v1/nodes/$md/relative?path=img%2Fchart.png' | sha256sum) == \$(sha256sum < samples/markdown/img/chart.png) ]]"
check "未登入取得相對圖片被拒 (401)" bash -c "[[ \$(status '$BASE/api/v1/nodes/$md/relative?path=img%2Fchart.png') == 401 ]]"

section "XSS / 標頭"
h=$(sget -D - -o /dev/null "$BASE/api/v1/nodes/$(node_id /samples/misc/evil.html)/content?inline=1")
check "HTML 一律以附件下載" grep -qi 'content-disposition: attachment' <<<"$h"
check "下載帶 nosniff" grep -qi 'x-content-type-options: nosniff' <<<"$h"
h=$(sget -D - -o /dev/null "$BASE/api/v1/nodes/$(node_id /samples/misc/evil.svg)/content?inline=1")
check "SVG 內嵌時有 CSP sandbox" grep -qi 'content-security-policy: sandbox' <<<"$h"
h=$(curl -sS -D - -o /dev/null "$BASE/")
check "前端頁面有嚴格 CSP" grep -qi "content-security-policy: default-src 'self'; script-src 'self'" <<<"$h"
check "前端 bundle 不含 API Key 或密碼" bash -c "! $DC exec -T app grep -rIl -e '$PASS' -e 'mbk_[0-9a-f]\{16\}_' /app/web"

section "LaTeX 編譯（隔離容器）"
latex() { spost -o "$WORK/latex.json" -w '%{http_code}' -X POST "$BASE/api/v1/nodes/$(node_id "$1")/latex"; }
code=$(latex /samples/latex/ok/main.tex)
check "中文 + 數學 + 圖片 + \\input + bibtex 編譯成功" test "$code" == 200
sget "$BASE/api/v1/nodes/$(node_id /samples/latex/ok/main.tex)/latex/pdf" -o "$WORK/main.pdf"
check "回傳真正的 PDF" bash -c "head -c 5 '$WORK/main.pdf' | grep -q '%PDF-'"
if command -v pdffonts >/dev/null; then
  check "PDF 嵌入 CJK 字型" bash -c "pdffonts '$WORK/main.pdf' | grep -Eiq 'fandol|noto|cjk'"
  check "PDF 可擷取中文文字" bash -c "pdftotext '$WORK/main.pdf' - | grep -q '畢氏定理'"
fi
check "只送出必要檔案（main, intro, plot.png, refs.bib）" bash -c "jq -e '.files|sort == [\"figs/plot.png\",\"main.tex\",\"refs.bib\",\"sections/intro.tex\"]' '$WORK/latex.json' >/dev/null"
code=$(latex /samples/latex/errors/syntax-error.tex)
check "語法錯誤：422 latex_failed + 錯誤行" bash -c "[[ $code == 422 ]] && jq -e '.error.code==\"latex_failed\" and (.error.message|test(\"Undefined control sequence\")) and (.error.log|length>0)' '$WORK/latex.json' >/dev/null"
code=$(latex /samples/latex/errors/missing-package.tex)
check "缺少套件：latex_missing_file" bash -c "[[ $code == 422 ]] && jq -e '.error.code==\"latex_missing_file\"' '$WORK/latex.json' >/dev/null"
code=$(latex /samples/latex/errors/missing-image.tex)
check "缺少圖片：列在 missing 並失敗" bash -c "[[ $code == 422 ]] && jq -e '.error.missing|index(\"not-uploaded.png\")' '$WORK/latex.json' >/dev/null"
code=$(latex /samples/latex/errors/shell-escape.tex)
check "shell escape 停用（文件在 \\shellescape=0 時才能成功）" test "$code" == 200
check "latex 容器內未產生 /tmp/pwned" bash -c "! $DC exec -T latex test -e /tmp/pwned"
code=$(latex /samples/latex/errors/read-host-file.tex)
check "讀取 /etc/passwd 被拒（openin_any=p）" bash -c "[[ $code == 422 ]] && ! jq -r .error.log '$WORK/latex.json' | grep -q 'root:x:0'"
code=$(latex /samples/latex/errors/pipe-input.tex)
check "\\input{|cmd} 不執行指令" bash -c "[[ $code == 422 ]] && ! jq -r .error.log '$WORK/latex.json' | grep -q 'uid='"
t0=$(date +%s)
code=$(latex /samples/latex/errors/infinite-loop.tex)
t1=$(date +%s)
check "無窮迴圈被時間 / CPU 限制終止（$((t1-t0)) 秒）" bash -c "[[ $code == 422 ]] && jq -e '.error.code==\"latex_timeout\" or .error.code==\"latex_resource_limit\" or .error.code==\"latex_failed\"' '$WORK/latex.json' >/dev/null && (( $t1 - $t0 <= 40 ))"
check "latex 容器無法連外" bash -c "! $DC exec -T latex getent hosts example.com >/dev/null 2>&1"

section "API Key：範圍、權限、到期、撤銷"
scope=$(node_id /samples/code)
other=$(node_id /samples/misc/binary.bin)
key=$(spost -H 'content-type: application/json' -d "{\"name\":\"e2e\",\"permissions\":{\"list\":true,\"read\":true,\"write\":true},\"scope_node_id\":\"$scope\"}" "$BASE/api/v1/keys" | jq -r .key)
AUTH=(-H "Authorization: Bearer $key")
check "list：只看到範圍內" bash -c "curl -sS -H 'Authorization: Bearer $key' '$BASE/api/v1/fs/list?path=/' | jq -e '.node.path==\"/\" and (.children|map(.name)|index(\"hello.c\"))' >/dev/null"
check "read：下載範圍內檔案" bash -c "[[ \$(curl -sS -H 'Authorization: Bearer $key' '$BASE/api/v1/fs/download?path=/hello.c' | sha256sum) == \$(sha256sum < samples/code/hello.c) ]]"
check "猜測範圍外 ID → 404" bash -c "[[ \$(status -H 'Authorization: Bearer $key' '$BASE/api/v1/nodes/$other/content') == 404 ]]"
check "路徑穿越 /../ → 400/404" bash -c "[[ \$(status -H 'Authorization: Bearer $key' '$BASE/api/v1/fs/download?path=/../misc/binary.bin') =~ ^40[04]$ ]]"
check "write：上傳到範圍內" bash -c "[[ \$(curl -s -o /dev/null -w '%{http_code}' -H 'Authorization: Bearer $key' --data-binary 'from-key' '$BASE/api/v1/fs/upload?path=/uploaded/by-key.txt') == 201 ]]"
check "Key 無法管理 Key (403)" bash -c "[[ \$(status -H 'Authorization: Bearer $key' '$BASE/api/v1/keys') == 403 ]]"
check "Key 放在網址無效 (401)" bash -c "[[ \$(status '$BASE/api/v1/whoami?api_key=$key') == 401 ]]"
check "日誌不含完整 Key / 密碼" bash -c "! $DC logs app 2>&1 | grep -q -e '${key:20}' -e '$PASS'"
kid=${key:4:16}
spost -X DELETE "$BASE/api/v1/keys/$kid" >/dev/null
check "撤銷後立即失效 (401)" bash -c "[[ \$(status -H 'Authorization: Bearer $key' '$BASE/api/v1/whoami') == 401 ]]"
code=$(spost -o /dev/null -w '%{http_code}' -H 'content-type: application/json' -d '{"name":"old","permissions":{"read":true},"expires_at":"2001-01-01T00:00:00Z"}' "$BASE/api/v1/keys")
check "不可建立已過期的 Key (400)" test "$code" == 400

section "分享連結"
sres=$(spost -H 'content-type: application/json' -d "{\"node_id\":\"$scope\",\"permission\":\"view\",\"expires_at\":null}" "$BASE/api/v1/shares")
slink=$(jq -r .link <<<"$sres"); sid=$(jq -r .share.id <<<"$sres"); ssec=${slink#*#}
STOK="mbs_${sid}_${ssec}"
check "連結格式 /s/<id>#<密鑰>（密鑰在 fragment）" bash -c "[[ '$slink' =~ ^/s/[0-9a-f]{16}#[0-9a-f]{64}$ ]]"
check "分享頁面可載入（SPA）" bash -c "curl -sS '$BASE/s/$sid' | grep -q '<div id=\"root\">'"
check "持有連結可列出分享的資料夾" bash -c "curl -sS -H 'X-Share-Token: $STOK' '$BASE/api/v1/nodes/root/children' | jq -e '.node.name==\"code\" and (.children|map(.name)|index(\"hello.c\"))' >/dev/null"
check "分享範圍外 → 404" bash -c "[[ \$(status -H 'X-Share-Token: $STOK' '$BASE/api/v1/nodes/$other/content') == 404 ]]"
check "檢視權限不能上傳 (403)" bash -c "[[ \$(curl -s -o /dev/null -w '%{http_code}' -H 'X-Share-Token: $STOK' --data-binary x '$BASE/api/v1/fs/upload?path=/x.txt') == 403 ]]"
check "分享連結不能使用管理功能 (403)" bash -c "[[ \$(status -H 'X-Share-Token: $STOK' '$BASE/api/v1/shares') == 403 ]]"
spost -o /dev/null -X PATCH -H 'content-type: application/json' -d '{"permission":"edit"}' "$BASE/api/v1/shares/$sid"
check "改為可編輯後可上傳 (201)" bash -c "[[ \$(curl -s -o /dev/null -w '%{http_code}' -H 'X-Share-Token: $STOK' --data-binary x '$BASE/api/v1/fs/upload?path=/from-share.txt') == 201 ]]"
check "日誌不含分享密鑰" bash -c "! $DC logs app 2>&1 | grep -q '$ssec'"
spost -o /dev/null -X DELETE "$BASE/api/v1/shares/$sid"
check "刪除後連結立即失效 (404)" bash -c "[[ \$(status -H 'X-Share-Token: $STOK' '$BASE/api/v1/nodes/root/children') == 404 ]]"

section "主題、背景、內容覆寫"
h=$(curl -sS -D - -o "$WORK/custom.css" "$BASE/theme/custom.css")
check "/theme/custom.css 可取得" grep -q '^HTTP/1.1 200' <<<"$h"
check "/theme/ 回應帶 CSP sandbox" grep -qi 'content-security-policy: sandbox' <<<"$h"
code=$(spost -o /dev/null -w '%{http_code}' -X PUT -H 'content-type: application/json' -d '{"background_url":"javascript:alert(1)"}' "$BASE/api/v1/settings")
check "危險背景網址被拒 (400)" test "$code" == 400
spost -o /dev/null -X PUT -H 'content-type: application/json' -d '{"background_url":"https://images.example.com/bg.jpg"}' "$BASE/api/v1/settings"
check "背景網域只加入 img-src" bash -c "sget -D - -o /dev/null '$BASE/' | grep -i '^content-security-policy' | grep -q \"img-src 'self' data: blob: https://images.example.com;\""
spost -o /dev/null -X PUT -H 'content-type: application/json' -d '{"background_url":""}' "$BASE/api/v1/settings"
nid=$(spost -H 'content-type: application/json' --data-binary '' -X POST "$BASE/api/v1/nodes/root/files?name=edit-me.md" | jq -r .id)
sha0=$(sget "$BASE/api/v1/nodes/$nid" | jq -r .sha256)
code=$(spost -o /dev/null -w '%{http_code}' -X PUT --data-binary 'v1' "$BASE/api/v1/nodes/$nid/content")
check "覆寫內容缺少 If-Match → 428" test "$code" == 428
code=$(spost -o /dev/null -w '%{http_code}' -X PUT -H "If-Match: \"$sha0\"" --data-binary '# 編輯後' "$BASE/api/v1/nodes/$nid/content")
check "If-Match 正確時覆寫成功" test "$code" == 200
code=$(spost -o /dev/null -w '%{http_code}' -X PUT -H "If-Match: \"$sha0\"" --data-binary 'stale' "$BASE/api/v1/nodes/$nid/content")
check "舊版本 If-Match → 412，不覆蓋" bash -c "[[ $code == 412 && \$(sget '$BASE/api/v1/nodes/$nid/content') == '# 編輯後' ]]"

section "垃圾桶"
tid=$(spost --data-binary 'trash me' "$BASE/api/v1/fs/upload?path=/to-trash.txt" | jq -r .id)
r=$(spost -X DELETE "$BASE/api/v1/nodes/$tid")
check "刪除預設移到垃圾桶" bash -c "jq -e '.permanent==false' <<<'$r' >/dev/null && [[ \$(sget -o /dev/null -w '%{http_code}' '$BASE/api/v1/fs/info?path=/to-trash.txt') == 404 ]]"
check "垃圾桶列出原位置" bash -c "sget '$BASE/api/v1/trash' | jq -e '.items|any(.path==\"/to-trash.txt\")' >/dev/null"
spost -o /dev/null -X POST "$BASE/api/v1/trash/$tid/restore"
check "還原後內容不變" bash -c "[[ \$(sget '$BASE/api/v1/fs/download?path=/to-trash.txt') == 'trash me' ]]"
spost -o /dev/null -X DELETE "$BASE/api/v1/nodes/$tid?permanent=1"
check "永久刪除" bash -c "[[ \$(sget -o /dev/null -w '%{http_code}' '$BASE/api/v1/nodes/$tid') == 404 ]]"

section "語意搜尋（本地模型，容器內）"
st=""
for _ in $(seq 1 240); do
  st=$(sget "$BASE/api/v1/rag/status")
  [[ $(jq -r '.state' <<<"$st") == ready && $(jq -r '.pending' <<<"$st") == 0 && $(jq -r '.indexed_files' <<<"$st") -gt 10 ]] && break
  sleep 2
done
echo "    $(jq -c '{model,state,indexed_files,chunks,pending,skipped}' <<<"$st")"
check "模型載入且索引完成" bash -c "jq -e '.state==\"ready\" and .pending==0 and .indexed_files>10' <<<'$st' >/dev/null"
res=$(sget "$BASE/api/v1/rag/search?q=$(urlenc '如何把 tex 文件編譯成 PDF')&k=5")
echo "$res" | jq -r '.results[] | "    \(.score|.*1000|round/1000)  \(.node.path):\(.line_start)"'
check "語意查詢命中 LaTeX 相關檔案" bash -c "jq -e '[.results[:5][].node.path] | any(test(\"latex|\\\\.tex\"))' <<<'$res' >/dev/null"
check ".env 等敏感檔不在索引中" bash -c "! sget '$BASE/api/v1/rag/search?q=API_TOKEN%20secret&k=20' | jq -r '.results[].node.name' | grep -qx '.env'"
skey=$(spost -H 'content-type: application/json' -d "{\"name\":\"rag\",\"permissions\":{\"list\":true,\"read\":true},\"scope_node_id\":\"$(node_id /samples/code)\"}" "$BASE/api/v1/keys" | jq -r .key)
check "API Key 語意搜尋只回傳範圍內結果" bash -c "curl -sS -H 'Authorization: Bearer $skey' '$BASE/api/v1/rag/search?q=$(urlenc 'LaTeX PDF 編譯')&k=10' | jq -e '.results|length>0 and all(.node.path|startswith(\"/\") and (test(\"samples\")|not))' >/dev/null"
check "RAG context 端點產生帶出處的段落" bash -c "curl -sS -H 'Authorization: Bearer $skey' '$BASE/api/v1/rag/context?q=python&k=2' | jq -e '.context|startswith(\"[1] /\")' >/dev/null"

# PDF: index the Chinese PDF compiled above; text is extracted in the latex worker.
mem_mib() {  # current memory of a compose service in MiB
  docker stats --no-stream --format '{{.MemUsage}}' "$($DC ps -q "$1")" | awk '{v=$1; u=v; gsub(/[0-9.]/,"",u); gsub(/[^0-9.]/,"",v);
    if (u=="GiB") v*=1024; else if (u=="KiB") v/=1024; else if (u=="B") v/=1048576; printf "%d", v}'
}
spost -o /dev/null -T "$WORK/main.pdf" "$BASE/api/v1/fs/upload?path=/pdf/%E8%AB%96%E6%96%87.pdf"
for _ in $(seq 1 90); do
  st=$(sget "$BASE/api/v1/rag/status")
  [[ $(jq -r '.pending' <<<"$st") == 0 ]] && break
  sleep 2
done
res=$(sget "$BASE/api/v1/rag/search?q=$(urlenc '畢氏定理')&k=5")
echo "$res" | jq -r '.results[] | "    \(.score|.*1000|round/1000)  \(.node.path) \(.unit) \(.line_start)"'
check "PDF 內文可被語意搜尋（以頁碼定位）" bash -c "jq -e '.results|any(.node.path==\"/pdf/論文.pdf\" and .unit==\"page\" and .line_start>=1)' <<<'$res' >/dev/null"
check "PDF 解析在 worker 容器（app 映像內沒有 pdftotext）" bash -c "! $DC exec -T app test -e /usr/bin/pdftotext && $DC exec -T latex test -x /usr/bin/pdftotext"
check "損壞的 PDF 被略過而非失敗" bash -c "spost -o /dev/null --data-binary 'not a pdf' '$BASE/api/v1/fs/upload?path=/pdf/broken.pdf' && sleep 6 && sget '$BASE/api/v1/rag/status' | jq -e '.failed==0' >/dev/null"
m_busy=$(mem_mib app)
check "app 記憶體（模型已載入）${m_busy} MiB < 500 MiB" test "$m_busy" -lt 500
for _ in $(seq 1 45); do
  [[ $(sget "$BASE/api/v1/rag/status" | jq -r '.model_loaded') == false ]] && break
  sleep 2
done
m_idle=$(mem_mib app)
check "閒置後自動卸載模型（${m_idle} MiB < 150 MiB）" bash -c "[[ \$(sget '$BASE/api/v1/rag/status' | jq -r .model_loaded) == false ]] && (( $m_idle < 150 ))"
check "卸載後搜尋會自動重新載入模型" bash -c "sget '$BASE/api/v1/rag/search?q=$(urlenc '畢氏定理')&k=3' | jq -e '.results|length>0' >/dev/null"

section "程式碼分析（靜態分析）"
printf '#include "hardware/pio.h"\nint f(unsigned x) {\n  return x & 0x4 == 0;\n}\n' > "$WORK/bug.c"
spost -o /dev/null -T "$WORK/bug.c" "$BASE/api/v1/fs/upload?path=/code/bug.c"
printf '.program p\n    set pins, 40\n' > "$WORK/p.pio"
spost -o /dev/null -T "$WORK/p.pio" "$BASE/api/v1/fs/upload?path=/code/p.pio"
bug=$(node_id /code/bug.c)
for _ in $(seq 1 30); do
  [[ $(sget "$BASE/api/v1/code/findings?node_id=$bug" | jq -r '.index.status // empty') == indexed ]] && break
  sleep 1
done
check "背景分析 C：找到位元運算優先順序錯誤（第 3 行）" bash -c "sget '$BASE/api/v1/code/findings?node_id=$bug' | jq -e '.findings | any(.rule == \"bit-precedence\" and .line == 3)' >/dev/null"
check "背景分析 PIO：set 超過 31" bash -c "sget '$BASE/api/v1/code/findings?node_id=$(node_id /code/p.pio)' | jq -e '.findings | any(.rule == \"pio-set-range\")' >/dev/null"
check "規則目錄可讀取" bash -c "sget '$BASE/api/v1/code/rules' | jq -e '.rules | length > 30' >/dev/null"

section "未登入 / 路徑穿越"
check "未登入列出 → 401" bash -c "[[ \$(status '$BASE/api/v1/nodes/root/children') == 401 ]]"
check "未登入下載 → 401" bash -c "[[ \$(status '$BASE/api/v1/nodes/$other/content') == 401 ]]"
check "惡意檔名 ../../etc/passwd → 400" bash -c "[[ \$(spost -o /dev/null -w '%{http_code}' --data-binary x '$BASE/api/v1/nodes/root/files?name=..%2F..%2Fetc%2Fpasswd') == 400 ]]"

section "重啟後資料仍存在 + 備份 / 還原"
want=$(sget "$BASE/api/v1/fs/download?path=$(urlenc /samples/markdown/README.md)" | sha256sum)
client_down
$DC down >/dev/null 2>&1
$DC up -d >/dev/null 2>&1
client_up
wait_healthy
login
got=$(sget "$BASE/api/v1/fs/download?path=$(urlenc /samples/markdown/README.md)" | sha256sum)
check "down/up 後檔案仍在且一致" test "$want" == "$got"
$DC exec -T app mistbox backup - > "$WORK/backup.tar"
check "線上備份產生 tar（含資料庫與檔案）" bash -c "tar tf '$WORK/backup.tar' | grep -q 'mistbox-backup/mistbox.db' && [[ \$(tar tf '$WORK/backup.tar' | grep -c blobs/) -gt 40 ]]"
check "verify：所有檔案 SHA-256 正確" bash -c "$DC exec -T app mistbox verify | grep -q '完整性檢查通過'"
docker volume create "${COMPOSE_PROJECT_NAME}_restore" >/dev/null
docker run --rm -i -v "${COMPOSE_PROJECT_NAME}_restore:/data" mistbox-app:local restore - < "$WORK/backup.tar" >/dev/null 2>&1
check "還原到新的 volume 後 verify 通過" bash -c "docker run --rm -v ${COMPOSE_PROJECT_NAME}_restore:/data mistbox-app:local verify | grep -q '完整性檢查通過'"
docker volume rm "${COMPOSE_PROJECT_NAME}_restore" >/dev/null

printf '\n\033[1m結果：%d 通過，%d 失敗\033[0m\n' "$pass" "$fail"
[[ $fail -eq 0 ]]
