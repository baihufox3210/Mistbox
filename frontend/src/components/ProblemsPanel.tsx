// IDE bottom panel: static-analysis findings and duplicate code, for the
// active file or the whole drive. Click a row to jump to the line.
import { useEffect, useMemo, useState } from 'react';
import { api, errorText, type CodeDuplicate, type CodeFinding, type CodeStatus, type DuplicateSide, type NodeInfo, type Severity } from '../api';
import { SEVERITY_LABEL, useCodeRules, useFileFindings } from '../state/codeFindings';
import type { DriveModel } from '../state/useDrive';
import { Icon } from './Icon';
import RuleNoteButton from './RuleNoteButton';

type Tab = 'problems' | 'duplicates';
type Scope = 'file' | 'all';

const SEV_ICON: Record<Severity, 'sevError' | 'sevWarning' | 'sevPerf' | 'sevInfo'> = {
  error: 'sevError',
  warning: 'sevWarning',
  perf: 'sevPerf',
  info: 'sevInfo',
};

/** `code` spans in rule hints. */
export function InlineCode({ text }: { text: string }) {
  return (
    <>
      {text.split('`').map((part, i) => (i % 2 ? <code key={i}>{part}</code> : <span key={i}>{part}</span>))}
    </>
  );
}

export function SevIcon({ s }: { s: Severity }) {
  return (
    <span className={`sev sev-${s}`} role="img" aria-label={SEVERITY_LABEL[s]} title={SEVERITY_LABEL[s]}>
      <Icon name={SEV_ICON[s]} size={14} />
    </span>
  );
}

/** Counts for the status bar. */
export function useActiveCounts(node: NodeInfo | undefined, enabled: boolean) {
  const st = useFileFindings(node, enabled);
  return useMemo(() => {
    const c: Record<Severity, number> = { error: 0, warning: 0, perf: 0, info: 0 };
    if (st.s === 'ready' || st.s === 'pending') for (const f of st.findings) c[f.severity] += 1;
    return { counts: c, state: st };
  }, [st]);
}

async function openAt(m: DriveModel, id: string, line: number) {
  try {
    m.openFile(await api.info(id), line);
  } catch (e) {
    m.toast(errorText(e));
  }
}

export default function ProblemsPanel({ m, active, onClose }: { m: DriveModel; active: NodeInfo | undefined; onClose: () => void }) {
  const [tab, setTab] = useState<Tab>('problems');
  const [scope, setScope] = useState<Scope>('file');
  const [status, setStatus] = useState<CodeStatus | null>(null);
  const rules = useCodeRules();
  useEffect(() => {
    api.codeStatus().then(setStatus, () => {});
  }, [m.version, tab, scope]);

  return (
    <section className="problems" aria-label="問題面板" data-region="problems">
      <div className="problems-head" role="toolbar">
        <div className="segmented sm" role="tablist">
          <button role="tab" aria-selected={tab === 'problems'} className={tab === 'problems' ? 'on' : ''} onClick={() => setTab('problems')}>
            問題
          </button>
          <button role="tab" aria-selected={tab === 'duplicates'} className={tab === 'duplicates' ? 'on' : ''} onClick={() => setTab('duplicates')}>
            重複程式碼
          </button>
        </div>
        <div className="segmented sm" role="group" aria-label="範圍">
          <button className={scope === 'file' ? 'on' : ''} aria-pressed={scope === 'file'} onClick={() => setScope('file')} title={active?.path}>
            目前檔案
          </button>
          <button className={scope === 'all' ? 'on' : ''} aria-pressed={scope === 'all'} onClick={() => setScope('all')}>
            整個雲端
          </button>
        </div>
        <span className="spacer" />
        {status && (
          <span className="muted small" title={Object.entries(status.by_lang).map(([k, v]) => `${k}: ${v}`).join('、')}>
            已分析 {status.files} 個程式檔{status.pending > 0 ? `（處理中 ${status.pending}）` : ''}
          </span>
        )}
        <button className="btn ghost icon-only xs" onClick={onClose} title="關閉（Ctrl+Shift+M）" aria-label="關閉問題面板">
          <Icon name="close" size={14} />
        </button>
      </div>
      <div className="problems-body">
        {scope === 'file' && !active ? (
          <div className="state small muted">沒有開啟的檔案。切到「整個雲端」查看所有檔案。</div>
        ) : tab === 'problems' ? (
          scope === 'file' ? (
            <FileProblems m={m} node={active!} rules={rules} />
          ) : (
            <AllProblems m={m} rules={rules} version={m.version} />
          )
        ) : (
          <Duplicates m={m} nodeId={scope === 'file' ? active!.id : undefined} version={m.version} sha={scope === 'file' ? active!.sha256 : null} />
        )}
      </div>
    </section>
  );
}

function FileProblems({ m, node, rules }: { m: DriveModel; node: NodeInfo; rules: ReturnType<typeof useCodeRules> }) {
  const st = useFileFindings(node, true);
  if (st.s === 'loading' || st.s === 'off') return <div className="state small"><span className="spinner tiny" /> 分析中…</div>;
  if (st.s === 'error') return <div className="state small muted">無法取得分析結果。</div>;
  if (st.s === 'skipped') return <div className="state small muted">這個檔案不分析：{st.detail}</div>;
  return (
    <>
      {st.s === 'pending' && <div className="problems-note small muted"><span className="spinner tiny" /> 檔案已變更，正在重新分析…</div>}
      <FindingList m={m} findings={st.findings} rules={rules} showFile={false} />
    </>
  );
}

function AllProblems({ m, rules, version }: { m: DriveModel; rules: ReturnType<typeof useCodeRules>; version: number }) {
  const [list, setList] = useState<CodeFinding[] | null>(null);
  const [sev, setSev] = useState<Severity | ''>('');
  useEffect(() => {
    setList(null);
    api.codeFindings(undefined, sev || undefined).then((r) => setList(r.findings), () => setList([]));
  }, [sev, version]);
  return (
    <>
      <div className="problems-filter">
        {(['', 'error', 'warning', 'perf', 'info'] as const).map((s) => (
          <button key={s} className={`chip ${sev === s ? 'on' : ''}`} onClick={() => setSev(s)} aria-pressed={sev === s}>
            {s ? <><SevIcon s={s} /> {SEVERITY_LABEL[s]}</> : '全部'}
          </button>
        ))}
      </div>
      {!list ? <div className="state small"><span className="spinner tiny" /> 載入中…</div> : <FindingList m={m} findings={list} rules={rules} showFile />}
    </>
  );
}

function FindingList({
  m,
  findings,
  rules,
  showFile,
}: {
  m: DriveModel;
  findings: CodeFinding[];
  rules: ReturnType<typeof useCodeRules>;
  showFile: boolean;
}) {
  const [open, setOpen] = useState<number | null>(null);
  if (findings.length === 0) return <div className="state small muted">沒有發現問題 ✓</div>;
  return (
    <ul className="finding-list">
      {findings.map((f, i) => {
        const r = rules?.get(f.rule);
        return (
          <li key={`${f.node_id}:${f.line}:${f.col}:${f.rule}`} className={open === i ? 'open' : ''}>
            <button
              className="finding"
              onClick={() => {
                setOpen(open === i ? null : i);
                openAt(m, f.node_id, f.line);
              }}
              aria-expanded={open === i}
            >
              <SevIcon s={f.severity} />
              <span className="finding-msg">
                <InlineCode text={f.message} />
              </span>
              {r && <span className="finding-rule muted">{r.title}</span>}
              <span className="finding-loc muted">
                {showFile ? `${f.path ?? f.name}:${f.line}` : `第 ${f.line} 行`}
                {f.symbol ? ` · ${f.symbol}` : ''}
              </span>
            </button>
            {open === i && r && (
              <div className="finding-hint">
                <InlineCode text={r.hint} />
                <span className="muted small"> （規則 {r.id}）</span>
                <RuleNoteButton m={m} target={{ kind: 'rule', id: r.id, title: r.title, steps: [r.hint] }} />
              </div>
            )}
          </li>
        );
      })}
    </ul>
  );
}

function Side({ m, s }: { m: DriveModel; s: DuplicateSide }) {
  return (
    <button className="dup-side" onClick={() => openAt(m, s.id, s.line_start)} title={`${s.path}:${s.line_start}-${s.line_end}`}>
      <span className="dup-path">{s.path}</span>
      <span className="muted">
        {s.line_start}–{s.line_end}
      </span>
    </button>
  );
}

function Duplicates({ m, nodeId, version, sha }: { m: DriveModel; nodeId?: string; version: number; sha: string | null }) {
  const [list, setList] = useState<CodeDuplicate[] | null>(null);
  const [err, setErr] = useState('');
  useEffect(() => {
    setList(null);
    setErr('');
    api.codeDuplicates(nodeId).then(
      (r) => setList(r.duplicates),
      (e) => setErr(errorText(e)),
    );
  }, [nodeId, version, sha]);
  if (err) return <div className="state small muted">{err}</div>;
  if (!list) return <div className="state small"><span className="spinner tiny" /> 比對中…</div>;
  if (list.length === 0) return <div className="state small muted">沒有找到重複的程式碼（至少約 50 個語彙單元才算）。</div>;
  return (
    <ul className="dup-list">
      {list.map((d, i) => (
        <li key={i}>
          <Side m={m} s={d.a} />
          <span className="dup-arrow" aria-hidden>⇄</span>
          <Side m={m} s={d.b} />
          <span className="muted small">{d.tokens} 個語彙單元</span>
          <button
            className="btn ghost xs"
            title="左右並排比較"
            onClick={() => window.dispatchEvent(new CustomEvent('mistbox:compare', { detail: { a: d.a, b: d.b } }))}
          >
            <Icon name="splitRight" size={13} /> 並排
          </button>
        </li>
      ))}
    </ul>
  );
}
