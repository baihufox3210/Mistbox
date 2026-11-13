import { useCallback, useEffect, useState } from 'react';
import { api, ApiError, errorText, type NodeInfo } from '../api';
import CodeView from './CodeView';
import PdfView from './PdfView';
import { EncodingSelect, TextGate, useEditing, useText, type EditorHooks } from './text';
import type { TextPreview } from '../api';

type Mode = 'pdf' | 'source' | 'split';

interface CompileState {
  status: 'idle' | 'compiling' | 'ok' | 'error';
  pdf?: ArrayBuffer;
  error?: ApiError | Error;
  missing?: string[];
  engine?: string;
  files?: string[];
}

/**
 * .tex preview: syntax-highlighted source *and* a real PDF compiled by the
 * isolated TeX Live worker. Highlighting is never presented as rendering.
 */
export default function LatexView({ node, editor }: { node: NodeInfo; editor?: EditorHooks }) {
  const t = useText(node.id, node.sha256);
  const [mode, setMode] = useState<Mode>('pdf');
  const [c, setC] = useState<CompileState>({ status: 'idle' });

  const compile = useCallback(async () => {
    setC({ status: 'compiling' });
    try {
      const r = await api.latexCompile(node.id);
      const pdf = await api.bytes(r.pdf_url);
      setC({ status: 'ok', pdf, missing: r.missing, engine: r.engine, files: r.files });
    } catch (e) {
      const err = e instanceof ApiError ? e : new Error(errorText(e));
      setC({
        status: 'error',
        error: err,
        missing: err instanceof ApiError ? (err.details.missing as string[] | undefined) : undefined,
        engine: err instanceof ApiError ? (err.details.engine as string | undefined) : undefined,
      });
      // On failure always show the source next to the error.
      setMode((m) => (m === 'pdf' ? 'split' : m));
    }
  }, [node.id]);

  useEffect(() => {
    if (mode !== 'source' && c.status === 'idle') compile();
  }, [mode, c.status, compile]);

  const pdfPane = (
    <div className="latex-pdf">
      {c.status === 'compiling' && (
        <div className="state">
          <span className="spinner" /> 以 TeX Live 編譯中（隔離環境，最長約 60 秒）…
        </div>
      )}
      {c.status === 'ok' && c.pdf && (
        <>
          {c.missing && c.missing.length > 0 && (
            <div className="notice warn">找不到或未授權的引用檔案：{c.missing.join('、')}</div>
          )}
          <PdfView data={c.pdf} />
        </>
      )}
      {c.status === 'error' && c.error && <CompileError error={c.error} missing={c.missing} />}
    </div>
  );

  return (
    <div className="latex-view">
      <div className="viewer-toolbar">
        <div className="segmented" role="tablist" aria-label="LaTeX 檢視模式">
          {(
            [
              ['pdf', 'PDF 預覽'],
              ['source', '原始碼'],
              ['split', '並排'],
            ] as const
          ).map(([k, label]) => (
            <button key={k} role="tab" aria-selected={mode === k} className={mode === k ? 'on' : ''} onClick={() => setMode(k)}>
              {label}
            </button>
          ))}
        </div>
        <button className="btn sm" onClick={compile} disabled={c.status === 'compiling'}>
          重新編譯
        </button>
        <span className="muted small">
          引擎：{c.engine ?? 'XeLaTeX'}（TeX Live；可用 <code>% !TEX program = pdflatex</code> 切換）
        </span>
      </div>
      <div className={`md-panes ${mode === 'pdf' ? 'render' : mode}`}>
        {mode !== 'pdf' && (
          <div className="md-source">
            <TextGate node={node} t={t}>
              {(d) => (
                <LatexSource
                  node={node}
                  d={d}
                  t={t}
                  editor={editor ? { ...editor, onSaved: (n) => (editor.onSaved?.(n), compile()) } : undefined}
                />
              )}
            </TextGate>
          </div>
        )}
        {mode !== 'source' && <div className="md-rendered">{pdfPane}</div>}
      </div>
    </div>
  );
}

function CompileError({ error, missing }: { error: ApiError | Error; missing?: string[] }) {
  const log = error instanceof ApiError ? String(error.details.log ?? '') : '';
  const code = error instanceof ApiError ? error.code : '';
  const title: Record<string, string> = {
    latex_failed: 'LaTeX 編譯失敗',
    latex_missing_file: '缺少檔案或套件',
    latex_timeout: '編譯逾時',
    latex_resource_limit: '超過資源限制',
    latex_output_too_large: '輸出過大',
    latex_disabled: 'LaTeX 編譯服務未啟用',
    latex_unavailable: 'LaTeX 編譯服務無法連線',
    latex_busy: '編譯佇列忙碌',
    latex_input_too_large: '輸入檔案過大',
  };
  return (
    <div className="latex-error" role="alert">
      <h4>{title[code] ?? '無法產生 PDF'}</h4>
      <pre className="err-msg">{error.message}</pre>
      {missing && missing.length > 0 && <p>找不到或未授權的引用檔案：{missing.join('、')}</p>}
      {log && (
        <details open={code !== 'latex_timeout'}>
          <summary>編譯記錄（最後部分）</summary>
          <pre className="latex-log">{log}</pre>
        </details>
      )}
    </div>
  );
}

function LatexSource({ node, d, t, editor }: { node: NodeInfo; d: TextPreview; t: ReturnType<typeof useText>; editor?: EditorHooks }) {
  const ed = useEditing(node, d, editor);
  return (
    <CodeView
      key={`${t.encoding}-${d.text.length}`}
      text={ed.text}
      lang="latex"
      editable={ed.editing}
      onChange={ed.setDraft}
      onSave={ed.save}
      line={editor?.line}
      toolbar={
        <>
          <EncodingSelect t={t} />
          {ed.controls}
        </>
      }
    />
  );
}
