// Diagnose scratch tab: paste an error log or a code snippet on the left.
// The right pane shows matching code, fixes and related
// Markdown notes. Nothing is stored: the last result lives in memory only.
import { lazy, Suspense, useEffect, useRef, useState } from 'react';
import { api, errorText, type DiagLocation, type DiagnoseReport, type LocationReason, type NodeInfo } from '../api';
import { SEVERITY_LABEL, useCodeRules } from '../state/codeFindings';
import type { DriveModel } from '../state/useDrive';
import { Icon } from './Icon';
import { InlineCode, SevIcon } from './ProblemsPanel';
import RuleNoteButton from './RuleNoteButton';

const CodeView = lazy(() => import('../preview/CodeView'));
const Markdown = lazy(() => import('../preview/Markdown'));

type Mode = 'auto' | 'error' | 'code';

const REASON: Record<LocationReason, string> = {
  stack: '錯誤位置',
  symbol: '定義',
  text: '訊息出處',
  clone: '相同程式碼',
  similar: '相似程式碼',
  callee: '呼叫的函式',
  caller: '呼叫者',
};

const LANGS: [string, string][] = [
  ['auto', '自動判斷語言'],
  ['c', 'C'],
  ['cpp', 'C++ / CUDA'],
  ['python', 'Python'],
  ['pio', 'PIO'],
  ['generic', '其他'],
];

/** Last input and result (memory only, cleared on reload). */
const memory: { text: string; mode: Mode; lang: string; report: DiagnoseReport | null; analyzed?: string; view?: string } = { text: '', mode: 'auto', lang: 'auto', report: null };

async function openAt(m: DriveModel, id: string, line: number) {
  try {
    const n: NodeInfo = await api.info(id);
    m.openFile(n, line);
  } catch (e) {
    m.toast(errorText(e));
  }
}

export default function DiagnosePanel({ m }: { m: DriveModel }) {
  const [text, setText] = useState(memory.text);
  const [mode, setMode] = useState<Mode>(memory.mode);
  const [lang, setLang] = useState(memory.lang);
  const [report, setReport] = useState<DiagnoseReport | null>(memory.report);
  const [analyzed, setAnalyzed] = useState(memory.analyzed);
  const [view, setView] = useState(memory.view ?? 'split');
  const [line, setLine] = useState<number>();
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState('');
  const request = useRef(0);
  const rules = useCodeRules();
  const signature = JSON.stringify([text, mode, lang]);
  const latest = useRef(signature);
  latest.current = signature;
  const stale = !!report && analyzed !== signature;
  const effectiveLang = lang !== 'auto' ? lang : !stale && report?.kind === 'code' ? report.lang ?? 'text' : 'text';

  useEffect(() => {
    Object.assign(memory, { text, mode, lang, report, analyzed, view });
  }, [text, mode, lang, report, analyzed, view]);
  useEffect(() => () => { request.current++; }, []);

  const run = async () => {
    if (!text.trim() || busy) return;
    const id = ++request.current;
    const snapshot = signature;
    setBusy(true);
    setErr('');
    try {
      const result = await api.codeDiagnose({ text, mode, lang: lang === 'auto' ? undefined : lang });
      if (request.current === id && latest.current === snapshot) {
        setReport(result);
        setAnalyzed(snapshot);
        if (view === 'input') setView('split');
      }
    } catch (e) {
      if (request.current === id && latest.current === snapshot) setErr(errorText(e));
    } finally {
      if (request.current === id) setBusy(false);
    }
  };
  const clear = () => {
    request.current++;
    setBusy(false);
    setText('');
    setReport(null);
    setAnalyzed(undefined);
    setErr('');
    setLine(undefined);
  };
  const inputLine = (n: number) => {
    const numbered = text.split('\n').findIndex((l) => Number(l.match(/^\s*(\d+)\s*[|│:]\s?/)?.[1]) === n);
    return numbered >= 0 ? numbered + 1 : n - (report?.first_line ?? 1) + 1;
  };
  const findings = report && !stale ? report.findings.filter((f) => !f.file) : [];
  const jump = (n: number) => {
    setView(view === 'result' ? 'split' : view);
    setLine(undefined);
    requestAnimationFrame(() => setLine(n));
  };
  return (
    <div className={`diag diag-tab view-${view}`} data-region="diagnose" onKeyDown={(e) => {
      if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') { e.preventDefault(); run(); }
    }}>
      <div className="viewer-toolbar diag-controls">
        <div className="segmented" role="tablist" aria-label="診斷檢視模式">
          {([['input', '輸入'], ['result', '結果'], ['split', '分割']] as const).map(([v, label]) => (
            <button key={v} role="tab" aria-selected={view === v} className={view === v ? 'on' : ''} onClick={() => setView(v)}>{label}</button>
          ))}
        </div>

          <div className="segmented sm" role="group" aria-label="輸入類型">
            {(
              [
                ['auto', '自動'],
                ['error', '錯誤訊息'],
                ['code', '程式碼'],
              ] as const
            ).map(([k, l]) => (
              <button key={k} className={mode === k ? 'on' : ''} aria-pressed={mode === k} onClick={() => setMode(k)}>
                {l}
              </button>
            ))}
          </div>
          <select value={lang} onChange={(e) => setLang(e.target.value)} aria-label="程式語言">
            {LANGS.map(([v, l]) => (
              <option key={v} value={v}>{l}</option>
            ))}
          </select>
          <span className="spacer" />
          {(text || report) && (
            <button className="btn ghost sm" onClick={clear}>
              清除
            </button>
          )}
          <button className="btn primary sm" onClick={run} disabled={busy || !text.trim()}>
            {busy ? <><span className="spinner tiny" /> 分析中…</> : <><Icon name="diagnose" size={14} /> 分析</>}
          </button>
        </div>
      {err && <p className="form-error" role="alert">{err}</p>}
      <div className="diag-status muted small" role="status">
        {lang === 'auto' ? `自動判斷：${!report || stale ? '待分析' : report.lang === 'generic' || !report.lang ? '未辨識' : LANGS.find(([v]) => v === report.lang)?.[1] ?? '未辨識'}` : `指定語言：${LANGS.find(([v]) => v === lang)?.[1]}`}
        {stale && ' · 內容已變更，請重新分析'}
        <span> · Ctrl+Enter 分析 · 僅暫存於記憶體</span>
      </div>
      <div className="diag-workspace">
        <section className="diag-source" aria-label="診斷輸入">
          <Suspense fallback={<span className="spinner" />}>
            <CodeView text={text} controlled lang={effectiveLang === 'generic' ? 'text' : effectiveLang} editable onChange={setText} line={line}
              diagnostics={findings.map((f) => ({ line: inputLine(f.line), severity: f.severity, message: f.message }))} />
          </Suspense>
        </section>
        <div className="diag-output" aria-label="診斷結果">
      {report ? (
        <div className="diag-results">
          <section className="diag-col" aria-label="對應的程式碼">
            {!stale && report.frames.length > 0 && <>
              <h3 className="diag-title">錯誤位置</h3>
              <ul className="finding-list">{report.frames.map((f, i) => <li key={i}>
                <button className="finding" onClick={() => jump(f.input_line)}>
                  <span className="finding-msg">{f.message ?? f.file ?? f.func ?? '堆疊位置'}</span>
                  <span className="finding-loc muted">輸入第 {f.input_line} 行</span>
                </button>
              </li>)}</ul>
            </>}
            {findings.length > 0 && <>
              <h3 className="diag-title">輸入的分析結果</h3>
              <ul className="finding-list">{findings.map((f, i) => <li key={i}>
                <button className="finding" onClick={() => jump(inputLine(f.line))}>
                  <SevIcon s={f.severity} /><span className="finding-msg"><InlineCode text={f.message} /></span>
                  <span className="finding-loc muted">第 {f.line} 行</span>
                </button>
              </li>)}</ul>
            </>}
            <h3 className="diag-title">對應的程式碼 <span className="muted small">{report.locations.length} 處</span></h3>
            {report.locations.length === 0 && <p className="diag-hint muted small">雲端裡沒有找到對應的程式碼。</p>}
            {report.locations.map((l, i) => (
              <LocationCard key={i} l={l} onOpen={() => openAt(m, l.node_id, l.line)} />
            ))}
            {report.findings.some((f) => f.file) && (
              <>
                <h3 className="diag-title">附近的分析結果</h3>
                <ul className="finding-list">
                  {report.findings
                    .filter((f) => f.file)
                    .map((f, i) => (
                      <li key={i}>
                        <button className="finding" onClick={() => openAt(m, f.node_id!, f.line)}>
                          <SevIcon s={f.severity} />
                          <span className="finding-msg"><InlineCode text={f.message} /></span>
                          <span className="finding-loc muted">{f.file!.path}:{f.line}</span>
                        </button>
                      </li>
                    ))}
                </ul>
              </>
            )}
          </section>
          <section className="diag-col" aria-label="解決方式與筆記">
            <h3 className="diag-title">解決方式與可優化的地方</h3>
            {report.solutions.length === 0 && <p className="diag-hint muted small">沒有對應的已知錯誤或規則。</p>}
            {report.solutions.map((s) => {
              const r = s.rule ? rules?.get(s.rule) : undefined;
              return (
                <article key={s.source + s.id} className={`sol-card ${r ? `sev-border-${r.severity}` : ''}`}>
                  <header>
                    {r ? <SevIcon s={r.severity} /> : <Icon name="sevInfo" size={14} />}
                    <strong><InlineCode text={s.title} /></strong>
                    <span className="badge">{s.source === 'known' ? '常見錯誤' : SEVERITY_LABEL[r?.severity ?? 'info']}</span>
                    <span className="spacer" />
                    <RuleNoteButton
                      m={m}
                      target={{ kind: s.source === 'known' ? 'known' : 'rule', id: s.source === 'known' ? s.id : (s.rule ?? s.id), title: s.title.replace(/`/g, ''), steps: s.steps }}
                    />
                  </header>
                  <ul>
                    {s.steps.map((x, i) => (
                      <li key={i}><InlineCode text={x} /></li>
                    ))}
                  </ul>
                  {s.rule && <div className="muted small">規則 <code>{s.rule}</code></div>}
                </article>
              );
            })}
            <h3 className="diag-title">相關筆記 <span className="muted small">{report.notes.length} 篇</span></h3>
            {report.notes.length === 0 && (
              <p className="diag-hint muted small">
                沒有相關筆記。按上方的「新增筆記」建立，或在任何 Markdown 筆記開頭加上
                <code>---</code> <code>rules: [omp-race]</code> <code>errors: [&quot;錯誤訊息片段&quot;]</code> <code>---</code>
                ，之後遇到同樣的規則或錯誤就會自動列在這裡。
              </p>
            )}
            {report.notes.map((n) => (
              <article key={n.node_id} className="note-card">
                <header>
                  <Icon name="file" size={14} />
                  <button className="link" onClick={() => openAt(m, n.node_id, n.line)} title="開啟筆記">
                    {n.file.path}
                  </button>
                  {n.heading && <span className="muted">› {n.heading}</span>}
                </header>
                <div className="note-matched">
                  {n.matched.slice(0, 6).map((x) => (
                    <span key={x} className="chip">{x}</span>
                  ))}
                </div>
                <div className="note-excerpt">
                  <Suspense fallback={<pre>{n.excerpt}</pre>}>
                    <Markdown source={n.excerpt} fileId={n.node_id} onOpenPath={() => openAt(m, n.node_id, n.line)} />
                  </Suspense>
                </div>
              </article>
            ))}
          </section>
        </div>
      ) : (
        <div className="diag-empty muted">
          <p>可以貼上：</p>
          <ul>
            <li>GCC／Clang／連結器錯誤、pioasm 錯誤、CMake 錯誤</li>
            <li>Python traceback、GDB backtrace、Rust panic、ROS 2 log（<code>[ERROR] [時間] [node]: 訊息</code>）</li>
            <li>一段程式碼：找出它在雲端的位置、相似的實作、呼叫關係與可優化之處</li>
          </ul>
        </div>
      )}
        </div>
      </div>
    </div>
  );
}

function LocationCard({ l, onOpen }: { l: DiagLocation; onOpen: () => void }) {
  return (
    <article className={`loc-card reason-${l.reason}`}>
      <header>
        <span className="badge">{REASON[l.reason]}</span>
        <button className="link mono" onClick={onOpen} title="在編輯器開啟">
          {l.file.path}:{l.line}
        </button>
        <span className="spacer" />
        <button className="btn ghost xs" onClick={onOpen}>
          <Icon name="external" size={13} /> 開啟
        </button>
      </header>
      <div className="loc-label"><InlineCode text={l.label} /></div>
      {l.excerpt.length > 0 && (
        <pre className="loc-excerpt">
          {l.excerpt.map((x) => (
            <div key={x.n} className={x.n >= l.line_start && x.n <= l.line_end ? 'hit' : ''}>
              <span className="log-n">{x.n}</span>
              <span className="log-t">{x.text || ' '}</span>
            </div>
          ))}
        </pre>
      )}
    </article>
  );
}
