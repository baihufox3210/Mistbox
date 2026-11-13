import { useEffect, useState } from 'react';
import { api, errorText, type CodeStatus, type RagStatus, type UiSettings } from '../api';
import { applyBackground, applyLogo, applyTheme, getLogoPref, getThemePref, loadCustomCss, type Layout, type LogoPref, type ThemePref } from '../theme';
import { Icon } from './Icon';
import { Modal } from './Modal';

const CODE_STATE: Record<string, string> = { starting: '啟動中', indexing: '分析中', embedding: '建立程式碼向量中', ready: '就緒' };
const LANG_LABEL: Record<string, string> = { c: 'C', cpp: 'C++', python: 'Python', pio: 'PIO', generic: '其他' };

export default function SettingsDialog({
  onClose,
  layout,
  setLayout,
}: {
  onClose: () => void;
  layout: Layout;
  setLayout: (l: Layout) => void;
}) {
  const [theme, setTheme] = useState<ThemePref>(getThemePref());
  const [logo, setLogo] = useState<LogoPref>(getLogoPref());
  const [s, setS] = useState<UiSettings | null>(null);
  const [url, setUrl] = useState('');
  const [dim, setDim] = useState(0);
  const [blur, setBlur] = useState(0);
  const [msg, setMsg] = useState('');
  const [rag, setRag] = useState<RagStatus | null>(null);
  const [code, setCode] = useState<CodeStatus | null>(null);

  useEffect(() => {
    api.settings().then((v) => {
      setS(v);
      setUrl(v.background_url);
      setDim(v.background_dim);
      setBlur(v.background_blur);
    }, (e) => setMsg(errorText(e)));
    const poll = () => {
      api.ragStatus().then(setRag, () => {});
      api.codeStatus().then(setCode, () => {});
    };
    poll();
    const t = setInterval(poll, 4000);
    return () => clearInterval(t);
  }, []);

  // Live preview while editing.
  useEffect(() => {
    if (s) applyBackground({ background_url: url.trim(), background_dim: dim, background_blur: blur });
  }, [url, dim, blur, s]);

  const saveBg = async () => {
    try {
      const v = await api.saveSettings({ background_url: url.trim(), background_dim: dim, background_blur: blur });
      setS(v);
      applyBackground(v);
      setMsg('背景已儲存（所有裝置都會套用）');
    } catch (e) {
      setMsg(errorText(e));
    }
  };

  return (
    <Modal
      title="設定"
      onClose={() => {
        if (s) applyBackground(s); // discard unsaved preview
        onClose();
      }}
      wide
    >
      <div className="settings">
        <section>
          <h3>外觀</h3>
          <div className="seg-row">
            <span>主題</span>
            <div className="segmented">
              {(
                [
                  ['light', '淺色', 'sun'],
                  ['dark', '深色', 'moon'],
                  ['system', '跟隨系統', 'layout'],
                ] as const
              ).map(([k, label, icon]) => (
                <button key={k} className={theme === k ? 'on' : ''} aria-pressed={theme === k} onClick={() => (setTheme(k), applyTheme(k))}>
                  <Icon name={icon} /> {label}
                </button>
              ))}
            </div>
          </div>
          <div className="seg-row">
            <span>Logo 配色</span>
            <div className="segmented">
              {([['auto', '跟隨主題'], ['dark', '深色'], ['light', '淺藍色']] as const).map(([value, label]) => (
                <button key={value} className={logo === value ? 'on' : ''} aria-pressed={logo === value}
                  onClick={() => { setLogo(value); applyLogo(value); }}>{label}</button>
              ))}
            </div>
          </div>
          <div className="seg-row">
            <span>版面</span>
            <div className="segmented">
              <button className={layout === 'drive' ? 'on' : ''} aria-pressed={layout === 'drive'} onClick={() => setLayout('drive')}>
                <Icon name="grid" /> 雲端硬碟
              </button>
              <button className={layout === 'ide' ? 'on' : ''} aria-pressed={layout === 'ide'} onClick={() => setLayout('ide')}>
                <Icon name="code" /> IDE
              </button>
            </div>
          </div>
        </section>

        <section>
          <h3>背景圖片</h3>
          <label className="field">
            圖片網址
            <input
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="https://… 或 /theme/bg.jpg 或雲端圖片的「設為背景」"
              spellCheck={false}
            />
          </label>
          <p className="muted small">
            可用 https:// 外部網址（只會把該網域加入圖片 CSP 白名單）、放在伺服器 <code>theme/</code> 資料夾的圖片（<code>/theme/檔名</code>），
            或在雲端圖片上按右鍵 →「設為背景」。留空即移除背景。
          </p>
          <label className="field range">
            整體透明度 {Math.round((1 - dim) * 100)}%
            <input type="range" min={0} max={100} step={1} value={Math.round((1 - dim) * 100)} onChange={(e) => setDim((100 - Number(e.target.value)) / 100)} />
          </label>
          <p className="muted small">100% 完全透明，0% 完全不透明；同步調整背景遮罩與內容面板。</p>
          <label className="field range">
            模糊 {blur}px
            <input type="range" min={0} max={40} step={1} value={blur} onChange={(e) => setBlur(Number(e.target.value))} />
          </label>
          <div className="row">
            <button className="btn primary" onClick={saveBg} disabled={!s}>儲存背景</button>
            <button className="btn" onClick={() => setUrl('')}>清除</button>
          </div>
        </section>

        <section>
          <h3>自訂 CSS</h3>
          {s?.theme_css_url ? (
            <>
              <p className="muted small">
                編輯伺服器上的 <code>theme/custom.css</code>（容器內 <code>/app/theme/custom.css</code>，唯讀掛載），存檔後按「重新載入」即可套用，
                不需重建映像。可用的 CSS 變數、穩定的選擇器與範例見 <code>theme/README.md</code> 與 <code>theme/custom.demo.css</code>。
              </p>
              <button className="btn" onClick={() => (loadCustomCss(true), setMsg('已重新載入 custom.css'))}>
                <Icon name="refresh" /> 重新載入 custom.css
              </button>
            </>
          ) : (
            <p className="muted small">伺服器未設定 MISTBOX_THEME_DIR，自訂 CSS 停用。</p>
          )}
        </section>

        <section>
          <h3>語意搜尋（本地模型）</h3>
          {!rag ? (
            <p className="muted">載入中…</p>
          ) : !rag.enabled ? (
            <p className="muted">未啟用（MISTBOX_RAG_MODEL=off，或映像未包含模型）。</p>
          ) : (
            <>
              <dl className="kv">
                <dt>模型</dt>
                <dd>{rag.model_description ?? rag.model}</dd>
                <dt>狀態</dt>
                <dd>
                  {{ loading: '載入模型中', ready: '就緒', indexing: '索引中', error: '錯誤' }[rag.state ?? 'loading']}
                  {rag.error && <span className="err"> — {rag.error}</span>}
                  {rag.state !== 'error' && rag.model_loaded === false && (
                    <span className="muted">（模型閒置已卸載以節省記憶體，下次搜尋時自動載入）</span>
                  )}
                </dd>
                <dt>PDF</dt>
                <dd>{rag.pdf ? '會擷取 PDF 文字（於隔離的 worker 容器中解析）' : '未啟用（需要 LaTeX / PDF worker）'}</dd>
                <dt>已索引</dt>
                <dd>
                  {rag.indexed_files} 個檔案 · {rag.chunks} 個片段
                </dd>
                <dt>待處理</dt>
                <dd>{rag.pending}</dd>
                <dt>略過 / 失敗</dt>
                <dd>
                  {rag.skipped}（非文字、過大、空白、加密或掃描型 PDF） / {rag.failed}
                </dd>
              </dl>
              <p className="muted small">敏感檔（.env、私鑰等）不會被索引；搜尋結果依登入者或 API Key 權限過濾。按 Ctrl+Shift+F 或在指令面板輸入 ? 開始搜尋。</p>
              <button className="btn" onClick={() => api.ragReindex().then(() => setMsg('已清除索引，背景重新建立中'), (e) => setMsg(errorText(e)))}>
                重建索引
              </button>
            </>
          )}
        </section>
        <section>
          <h3>程式碼分析與相似程式碼搜尋</h3>
          {!code ? (
            <p className="muted">載入中…</p>
          ) : (
            <>
              <dl className="kv">
                <dt>狀態</dt>
                <dd>{CODE_STATE[code.state] ?? code.state}</dd>
                <dt>已分析</dt>
                <dd>
                  {code.files} 個程式檔
                  {Object.keys(code.by_lang).length > 0 && (
                    <span className="muted">
                      （
                      {Object.entries(code.by_lang)
                        .filter(([l]) => l !== 'markdown')
                        .sort((a, b) => b[1] - a[1])
                        .map(([l, n]) => `${LANG_LABEL[l] ?? l} ${n}`)
                        .join('、')}
                      ）
                    </span>
                  )}
                  {' · '}
                  {code.notes} 篇 Markdown 筆記
                </dd>
                <dt>待處理</dt>
                <dd>{code.pending}</dd>
                <dt>分析結果</dt>
                <dd>
                  錯誤 {code.findings.error ?? 0} · 警告 {code.findings.warning ?? 0} · 可優化 {code.findings.perf ?? 0} · 建議 {code.findings.info ?? 0}
                </dd>
                <dt>程式碼模型</dt>
                <dd>
                  {code.semantic ? (
                    <>
                      {code.semantic.model === 'jina-code-q8' ? 'jina-embeddings-v2-base-code（int8 · 768 維）' : code.semantic.model}
                      {code.semantic.error ? (
                        <span className="err"> — {code.semantic.error}</span>
                      ) : code.semantic.model_loaded ? (
                        <span className="muted">（已載入）</span>
                      ) : (
                        <span className="muted">（閒置已卸載以節省記憶體，下次使用時自動載入）</span>
                      )}
                    </>
                  ) : (
                    <span className="muted">未啟用（MISTBOX_CODE_MODEL=off，或映像未包含模型）；診斷模式的其他功能照常</span>
                  )}
                </dd>
                {code.semantic && (
                  <>
                    <dt>程式碼向量</dt>
                    <dd>
                      {code.semantic.files} 個檔案 · {code.semantic.chunks} 個片段
                    </dd>
                  </>
                )}
              </dl>
              <p className="muted small">
                不需要 AVX2。可在這台機器上執行 <code>docker compose exec app mistbox rag selftest</code> 確認兩個模型都能運作。敏感檔與垃圾桶不分析。
              </p>
              <button className="btn" onClick={() => api.codeReindex().then(() => setMsg('已清除程式碼分析結果，背景重新分析中'), (e) => setMsg(errorText(e)))}>
                重新分析
              </button>
            </>
          )}
        </section>
        {msg && <p className="form-ok" role="status">{msg}</p>}
      </div>
    </Modal>
  );
}
