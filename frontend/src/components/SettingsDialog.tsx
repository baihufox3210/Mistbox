import { useEffect, useId, useState } from 'react';
import { api, errorText, type CodeStatus, type RagStatus, type UiSettings } from '../api';
import { applyBackground, applyLogo, applyTheme, getLogoPref, getThemePref, loadCustomCss, type Layout, type LogoPref, type ThemePref } from '../theme';
import { Icon } from './Icon';
import { Modal } from './Modal';

const CODE_STATE: Record<string, string> = { starting: '啟動中', indexing: '分析中', embedding: '索引中', ready: '就緒' };
type Page = 'appearance' | 'background' | 'system';

export default function SettingsDialog({ onClose, layout, setLayout, section }: {
  onClose: () => void; layout: Layout; setLayout: (l: Layout) => void; section?: Page;
}) {
  const id = useId();
  const [page, setPage] = useState<Page>(section ?? 'appearance');
  const [theme, setTheme] = useState<ThemePref>(getThemePref());
  const [logo, setLogo] = useState<LogoPref>(getLogoPref());
  const [s, setS] = useState<UiSettings | null>(null);
  const [url, setUrl] = useState('');
  const [dim, setDim] = useState(0);
  const [blur, setBlur] = useState(0);
  const [notice, setNotice] = useState<{ kind: 'ok' | 'error'; text: string } | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [rag, setRag] = useState<RagStatus | null>(null);
  const [code, setCode] = useState<CodeStatus | null>(null);
  const [ragError, setRagError] = useState('');
  const [codeError, setCodeError] = useState('');

  useEffect(() => {
    let live = true;
    api.settings().then((v) => {
      if (!live) return;
      setS(v); setUrl(v.background_url); setDim(v.background_dim); setBlur(v.background_blur);
    }, (e) => live && setNotice({ kind: 'error', text: errorText(e) }));
    return () => { live = false; };
  }, []);

  useEffect(() => {
    if (page !== 'system') return;
    let live = true;
    const poll = () => {
      api.ragStatus().then((v) => { if (live) { setRag(v); setRagError(''); } }, (e) => live && setRagError(errorText(e)));
      api.codeStatus().then((v) => { if (live) { setCode(v); setCodeError(''); } }, (e) => live && setCodeError(errorText(e)));
    };
    poll();
    const timer = setInterval(poll, 4000);
    return () => { live = false; clearInterval(timer); };
  }, [page]);

  useEffect(() => {
    if (s) applyBackground({ background_url: url.trim(), background_dim: dim, background_blur: blur });
  }, [url, dim, blur, s]);

  const changed = !!s && (url.trim() !== s.background_url || dim !== s.background_dim || blur !== s.background_blur);
  const saveBg = async () => {
    setBusy('background'); setNotice(null);
    try {
      const v = await api.saveSettings({ background_url: url.trim(), background_dim: dim, background_blur: blur });
      setS(v); applyBackground(v); setNotice({ kind: 'ok', text: '背景已儲存' });
    } catch (e) { setNotice({ kind: 'error', text: errorText(e) }); }
    finally { setBusy(null); }
  };
  const reindex = async (kind: 'rag' | 'code') => {
    setBusy(kind); setNotice(null);
    try {
      if (kind === 'rag') await api.ragReindex(); else await api.codeReindex();
      setNotice({ kind: 'ok', text: kind === 'rag' ? '正在重建索引' : '正在重新分析' });
    } catch (e) { setNotice({ kind: 'error', text: errorText(e) }); }
    finally { setBusy(null); }
  };

  return (
    <Modal title="設定" onClose={() => { if (s) applyBackground(s); onClose(); }} wide>
      <div className="settings">
        <nav className="settings-nav" aria-label="設定分類">
          {([['appearance', '外觀', 'sun'], ['background', '背景', 'image'], ['system', '系統', 'gear']] as const).map(([key, label, icon]) => (
            <button key={key} className={page === key ? 'on' : ''} aria-current={page === key ? 'page' : undefined}
              onClick={() => { setPage(key); setNotice(null); }}><Icon name={icon} size={16} />{label}</button>
          ))}
        </nav>
        <div className="settings-content">
          {page === 'appearance' && <section aria-label="外觀設定">
            <h3>外觀</h3>
            <div className="seg-row">
              <span>主題</span>
              <div className="segmented" role="group" aria-label="主題">
                {([['light', '淺色', 'sun'], ['dark', '深色', 'moon'], ['system', '跟隨系統', 'layout']] as const).map(([key, label, icon]) => (
                  <button key={key} className={theme === key ? 'on' : ''} aria-pressed={theme === key} onClick={() => { setTheme(key); applyTheme(key); }}><Icon name={icon} />{label}</button>
                ))}
              </div>
            </div>
            <div className="seg-row">
              <span>標誌配色</span>
              <div className="segmented" role="group" aria-label="標誌配色">
                {([['auto', '跟隨主題'], ['dark', '深色'], ['light', '淺藍色']] as const).map(([key, label]) => (
                  <button key={key} className={logo === key ? 'on' : ''} aria-pressed={logo === key} onClick={() => { setLogo(key); applyLogo(key); }}>{label}</button>
                ))}
              </div>
            </div>
            <div className="seg-row">
              <span>版面</span>
              <div className="segmented" role="group" aria-label="版面">
                <button className={layout === 'drive' ? 'on' : ''} aria-pressed={layout === 'drive'} onClick={() => setLayout('drive')}><Icon name="grid" />雲端硬碟</button>
                <button className={layout === 'ide' ? 'on' : ''} aria-pressed={layout === 'ide'} onClick={() => setLayout('ide')}><Icon name="code" />IDE</button>
              </div>
            </div>
          </section>}
          {page === 'background' && <section aria-label="背景設定">
            <h3>背景圖片</h3>
            <label className="field">圖片網址
              <input value={url} onChange={(e) => setUrl(e.target.value)} placeholder="貼上圖片網址" spellCheck={false} disabled={!s} />
            </label>
            <label className="field range" htmlFor={`${id}-transparency`}>
              <span className="field-heading">透明度 <output htmlFor={`${id}-transparency`} aria-live="off">{Math.round((1 - dim) * 100)}%</output></span>
              <input id={`${id}-transparency`} aria-valuetext={`${Math.round((1 - dim) * 100)}%`} type="range" min={0} max={100} step={1} value={Math.round((1 - dim) * 100)} onChange={(e) => setDim((100 - Number(e.target.value)) / 100)} disabled={!s} />
            </label>
            <label className="field range" htmlFor={`${id}-blur`}>
              <span className="field-heading">模糊 <output htmlFor={`${id}-blur`} aria-live="off">{blur}px</output></span>
              <input id={`${id}-blur`} aria-valuetext={`${blur} 像素`} type="range" min={0} max={40} step={1} value={blur} onChange={(e) => setBlur(Number(e.target.value))} disabled={!s} />
            </label>
            <div className="settings-actions">
              <button className="btn ghost" onClick={() => setUrl('')} disabled={!s || !url}>移除背景</button>
              <button className="btn primary" onClick={saveBg} disabled={!changed || !!busy}>{busy === 'background' ? '儲存中…' : '儲存背景'}</button>
            </div>
          </section>}
          {page === 'system' && <>
            <section aria-label="搜尋索引">
              <h3>搜尋索引</h3>
              {ragError ? <p className="form-error" role="alert">{ragError}</p> : !rag ? <p className="muted">載入中…</p> : !rag.enabled ? <span className="badge muted">未啟用</span> : <>
                <dl className="kv">
                  <dt>狀態</dt><dd>{{ loading: '載入中', ready: '就緒', indexing: '索引中', error: '錯誤' }[rag.state ?? 'loading']}{rag.error && <span className="err"> · {rag.error}</span>}</dd>
                  <dt>已索引</dt><dd>{rag.indexed_files ?? 0} 個檔案</dd>
                  <dt>待處理</dt><dd>{rag.pending ?? 0}</dd>
                  <dt>略過 / 失敗</dt><dd>{rag.skipped ?? 0} / {rag.failed ?? 0}</dd>
                  <dt>PDF 搜尋</dt><dd>{rag.pdf ? '已啟用' : '未啟用'}</dd>
                </dl>
                <button className="btn" onClick={() => reindex('rag')} disabled={!!busy}>{busy === 'rag' ? '處理中…' : '重建索引'}</button>
              </>}
            </section>
            <section aria-label="程式碼分析">
              <h3>程式碼分析</h3>
              {codeError ? <p className="form-error" role="alert">{codeError}</p> : !code ? <p className="muted">載入中…</p> : <>
                <dl className="kv">
                  <dt>狀態</dt><dd>{CODE_STATE[code.state] ?? code.state}</dd>
                  <dt>已分析</dt><dd>{code.files} 個檔案 · {code.notes} 篇筆記</dd>
                  <dt>待處理</dt><dd>{code.pending}</dd>
                  <dt>分析結果</dt><dd>錯誤 {code.findings.error ?? 0} · 警告 {code.findings.warning ?? 0} · 可優化 {code.findings.perf ?? 0} · 建議 {code.findings.info ?? 0}</dd>
                  <dt>相似程式碼搜尋</dt><dd>{code.semantic ? '已啟用' : '未啟用'}{code.semantic?.error && <span className="err"> · {code.semantic.error}</span>}</dd>
                </dl>
                <button className="btn" onClick={() => reindex('code')} disabled={!!busy}>{busy === 'code' ? '處理中…' : '重新分析'}</button>
              </>}
            </section>
            <section aria-label="自訂樣式">
              <h3>自訂樣式</h3>
              {s?.theme_css_url ? <button className="btn" onClick={() => { loadCustomCss(true); setNotice({ kind: 'ok', text: '正在重新載入樣式' }); }}><Icon name="refresh" />重新載入樣式</button> : <span className="badge muted">未啟用</span>}
            </section>
          </>}
          {notice && <p className={notice.kind === 'error' ? 'form-error' : 'form-ok'} role={notice.kind === 'error' ? 'alert' : 'status'}>{notice.text}</p>}
        </div>
      </div>
    </Modal>
  );
}
