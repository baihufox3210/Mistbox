import { useCallback, useEffect, useState } from 'react';
import { api, contentUrl, type NodeInfo } from '../api';
import { ErrorState } from '../components/ErrorState';
import { Icon } from '../components/Icon';
import { languageFor, previewKind } from '../fileTypes';
import { formatDate, formatSize } from '../format';
import CodeView from './CodeView';
import { Info } from './Info';
import LatexView from './LatexView';
import Markdown from './Markdown';
import PdfView from './PdfView';
import { EncodingSelect, TextGate, useEditing, useText, type EditorHooks } from './text';

export { ErrorState };

const PDF_PREVIEW_MAX = 100 * 1024 * 1024;

export interface PreviewProps {
  node: NodeInfo;
  /** Open a path referenced from inside `node` (relative Markdown links). */
  onOpenPath: (from: NodeInfo, rel: string) => void;
  editor?: EditorHooks;
  /** Extra buttons in the header (e.g. "設為背景"). */
  headerExtra?: React.ReactNode;
  /** Hide the built-in header (the Drive overlay renders its own). */
  bare?: boolean;
}

function MarkdownPreview({ node, onOpenPath, editor }: PreviewProps) {
  const t = useText(node.id);
  const [mode, setMode] = useState<'render' | 'source' | 'split'>(editor?.startEditing ? 'split' : 'render');
  return (
    <TextGate node={node} t={t}>
      {(d) => <MarkdownBody node={node} d={d} t={t} mode={mode} setMode={setMode} onOpenPath={onOpenPath} editor={editor} />}
    </TextGate>
  );
}

function MarkdownBody({
  node,
  d,
  t,
  mode,
  setMode,
  onOpenPath,
  editor,
}: {
  node: NodeInfo;
  d: import('../api').TextPreview;
  t: ReturnType<typeof useText>;
  mode: 'render' | 'source' | 'split';
  setMode: (m: 'render' | 'source' | 'split') => void;
  onOpenPath: PreviewProps['onOpenPath'];
  editor?: EditorHooks;
}) {
  const ed = useEditing(node, d, editor);
  return (
    <div className="md-preview">
      <div className="viewer-toolbar">
        <div className="segmented" role="tablist" aria-label="Markdown 檢視模式">
          {(
            [
              ['render', '渲染'],
              ['source', '原始碼'],
              ['split', '分割'],
            ] as const
          ).map(([k, label]) => (
            <button key={k} role="tab" aria-selected={mode === k} className={mode === k ? 'on' : ''} onClick={() => setMode(k)}>
              {label}
            </button>
          ))}
        </div>
        <EncodingSelect t={t} />
        <span className="spacer" />
        {ed.controls}
      </div>
      <div className={`md-panes ${mode}`}>
        {mode !== 'render' && (
          <div className="md-source">
            <CodeView
              key={`${t.encoding}-${d.text.length}`}
              text={ed.text}
              lang="markdown"
              editable={ed.editing}
              onChange={ed.setDraft}
              onSave={ed.save}
              line={editor?.line}
            />
          </div>
        )}
        {mode !== 'source' && (
          <div className="md-rendered">
            <Markdown source={ed.text} fileId={node.id} onOpenPath={(rel) => onOpenPath(node, rel)} />
          </div>
        )}
      </div>
    </div>
  );
}

function CodePreview({ node, editor }: PreviewProps) {
  const t = useText(node.id);
  return <TextGate node={node} t={t}>{(d) => <CodeBody node={node} d={d} t={t} editor={editor} />}</TextGate>;
}

function CodeBody({ node, d, t, editor }: { node: NodeInfo; d: import('../api').TextPreview; t: ReturnType<typeof useText>; editor?: EditorHooks }) {
  const ed = useEditing(node, d, editor);
  return (
    <CodeView
      key={`${t.encoding}-${d.text.length}`}
      text={ed.text}
      lang={languageFor(node.name)}
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

function PdfPreview({ node, editor }: PreviewProps) {
  const [data, setData] = useState<ArrayBuffer | null>(null);
  const [error, setError] = useState<unknown>(null);
  const load = useCallback(() => {
    setError(null);
    api.bytes(contentUrl(node.id, true)).then(setData, setError);
  }, [node.id]);
  useEffect(() => {
    if (node.size <= PDF_PREVIEW_MAX) load();
  }, [load, node.size]);
  if (node.size > PDF_PREVIEW_MAX) return <Info node={node} reason={`PDF 超過 ${formatSize(PDF_PREVIEW_MAX)}，請下載後閱讀。`} />;
  if (error) return <ErrorState error={error} node={node} retry={load} />;
  if (!data) return <div className="state"><span className="spinner" /> 載入 PDF…</div>;
  return <PdfView data={data} page={editor?.line} />;
}

function MediaPreview({ node, kind }: PreviewProps & { kind: 'image' | 'audio' | 'video' }) {
  const [failed, setFailed] = useState(false);
  const src = contentUrl(node.id, true);
  if (failed) return <Info node={node} reason="瀏覽器無法顯示或播放此檔案格式；檔案本身已完整保存，可下載後開啟。" />;
  if (kind === 'image')
    return (
      <div className="media-view checker">
        <img src={src} alt={node.name} onError={() => setFailed(true)} />
      </div>
    );
  if (kind === 'audio')
    return (
      <div className="media-view">
        <audio controls preload="metadata" src={src} onError={() => setFailed(true)} />
      </div>
    );
  return (
    <div className="media-view">
      <video controls preload="metadata" src={src} onError={() => setFailed(true)} />
    </div>
  );
}

export default function Preview(props: PreviewProps) {
  const { node } = props;
  if (node.kind === 'dir') return <div className="state">資料夾</div>;
  const kind = previewKind(node.name);
  let body: React.ReactNode;
  switch (kind) {
    case 'markdown':
      body = <MarkdownPreview {...props} />;
      break;
    case 'latex':
      body = <LatexView node={node} editor={props.editor} />;
      break;
    case 'code':
    case 'unknown':
      body = <CodePreview {...props} />;
      break;
    case 'pdf':
      body = <PdfPreview {...props} />;
      break;
    case 'image':
    case 'audio':
    case 'video':
      body = <MediaPreview {...props} kind={kind} />;
      break;
    default:
      body = <Info node={node} />;
  }
  return (
    <div className="preview" data-region="preview">
      {!props.bare && (
        <div className="preview-head">
          <div className="preview-title">
            <strong title={node.path ?? node.name}>{node.name}</strong>
            {node.sensitive && <span className="badge warn" title="匯入或上傳時判定為可能含有機密">敏感</span>}
            <span className="muted">
              {formatSize(node.size)} · {formatDate(node.modified_at)}
            </span>
          </div>
          {props.headerExtra}
          <a className="btn sm" href={contentUrl(node.id)} download>
            <Icon name="download" /> 下載
          </a>
        </div>
      )}
      <div className="preview-body">{body}</div>
    </div>
  );
}
