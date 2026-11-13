import { useEffect, useState } from 'react';
import { api, errorText, type RagStatus, type UiSettings } from '../api';
import { applyBackground, applyTheme, getThemePref, loadCustomCss, type Layout, type ThemePref } from '../theme';
import { Icon } from './Icon';
import { Modal } from './Modal';

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
  const [s, setS] = useState<UiSettings | null>(null);
  const [url, setUrl] = useState('');
  const [dim, setDim] = useState(0.35);
  const [blur, setBlur] = useState(0);
  const [msg, setMsg] = useState('');
  const [rag, setRag] = useState<RagStatus | null>(null);

  useEffect(() => {
    api.settings().then((v) => {
      setS(v);
      setUrl(v.background_url);
      setDim(v.background_dim);
      setBlur(v.background_blur);
    }, (e) => setMsg(errorText(e)));
    const poll = () => api.ragStatus().then(setRag, () => {});
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
            遮罩不透明度 {Math.round(dim * 100)}%
            <input type="range" min={0} max={100} step={1} value={Math.round(dim * 100)} onChange={(e) => setDim(Number(e.target.value) / 100)} />
          </label>
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
        {msg && <p className="form-ok" role="status">{msg}</p>}
      </div>
    </Modal>
  );
}
