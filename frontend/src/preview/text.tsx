import { useCallback, useEffect, useState } from 'react';
import { api, ApiError, contentUrl, errorText, type NodeInfo, type TextPreview } from '../api';
import { ErrorState } from '../components/ErrorState';
import { Icon } from '../components/Icon';
import { formatSize } from '../format';
import { Info } from './Info';

export function useText(id: string) {
  const [data, setData] = useState<TextPreview | null>(null);
  const [error, setError] = useState<ApiError | null>(null);
  const [encoding, setEncoding] = useState<string | undefined>();
  const [partial, setPartial] = useState(false);
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    api.text(id, encoding, partial).then(
      (d) => !cancelled && (setData(d), setLoading(false)),
      (e) => !cancelled && (setError(e instanceof ApiError ? e : new ApiError(0, 'unknown', String(e))), setLoading(false)),
    );
    return () => {
      cancelled = true;
    };
  }, [id, encoding, partial]);
  return { data, error, loading, encoding, setEncoding, loadPartial: () => setPartial(true) };
}

export type TextState = ReturnType<typeof useText>;

/** Shared loading / error / limit / encoding UI around decoded text. */
export function TextGate({ node, t, children }: { node: NodeInfo; t: TextState; children: (d: TextPreview) => React.ReactNode }) {
  if (t.loading && !t.data) return <div className="state"><span className="spinner" /> 載入中…</div>;
  if (t.error) {
    if (t.error.code === 'preview_too_large') {
      const limit = Number(t.error.details.limit ?? 0);
      return (
        <div className="state">
          <p>
            檔案大小 {formatSize(node.size)}，超過預覽上限 {formatSize(limit)}。
          </p>
          <div className="row">
            <button className="btn" onClick={t.loadPartial}>只預覽前 {formatSize(limit)}</button>
            <a className="btn primary" href={contentUrl(node.id)} download>
              <Icon name="download" /> 下載完整檔案
            </a>
          </div>
        </div>
      );
    }
    return <ErrorState error={t.error} node={node} />;
  }
  const d = t.data!;
  if (d.binary) return <Info node={node} reason="這看起來是二進位檔案，無法以文字預覽。檔案已完整保存，可直接下載。" />;
  return (
    <div className="text-gate">
      {!d.valid_utf8 && (
        <div className="notice warn" role="status">
          此檔案不是有效的 UTF-8。目前以 <b>{d.encoding}</b> 解碼顯示{t.encoding ? '' : '（自動偵測）'}；原始檔案未被修改。為避免改變編碼，此檔案只能檢視、不能編輯。
        </div>
      )}
      {d.had_errors && <div className="notice warn">部分位元組無法以 {d.encoding} 解碼，已以 � 顯示。可嘗試其他編碼。</div>}
      {d.truncated && (
        <div className="notice">
          僅顯示前 {formatSize(d.limit)}（共 {formatSize(d.size)}），因此無法編輯。
          <a href={contentUrl(node.id)} download>下載完整檔案</a>
        </div>
      )}
      {children(d)}
    </div>
  );
}

export function EncodingSelect({ t }: { t: TextState }) {
  if (!t.data) return null;
  return (
    <label className="enc-select">
      編碼
      <select value={t.encoding ?? ''} onChange={(e) => t.setEncoding(e.target.value || undefined)}>
        <option value="">自動（{t.data.detected}）</option>
        {t.data.encodings.map((e) => (
          <option key={e} value={e}>{e}</option>
        ))}
      </select>
    </label>
  );
}

/** Editor integration supplied by the layout (tabs know about dirty state). */
export interface EditorHooks {
  /** Start in edit mode (IDE) or show an "編輯" toggle (Drive). */
  startEditing?: boolean;
  onDirty?: (dirty: boolean) => void;
  onSaved?: (n: NodeInfo) => void;
  register?: (save: (() => Promise<void>) | null) => void;
  line?: number;
}

/** Draft / save state for an editable text file (UTF-8, not truncated). */
export function useEditing(node: NodeInfo, d: TextPreview | null, hooks?: EditorHooks) {
  const canEdit = !!d && !d.binary && !d.truncated && (d.valid_utf8 || d.size === 0) && d.encoding === 'UTF-8';
  const [editing, setEditing] = useState(!!hooks?.startEditing);
  const [base, setBase] = useState<string | null>(null);
  const [draft, setDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [msg, setMsg] = useState('');
  const [sha, setSha] = useState(node.sha256 ?? '');
  const original = base ?? d?.text ?? '';
  const text = draft ?? original;
  const dirty = draft !== null && draft !== original;

  useEffect(() => hooks?.onDirty?.(dirty), [dirty, hooks]);

  const save = useCallback(async () => {
    if (!dirty || draft === null || saving) return;
    setSaving(true);
    setMsg('');
    try {
      const n = await api.putContent(node.id, draft, sha);
      setSha(n.sha256 ?? '');
      setBase(draft);
      setDraft(null);
      setMsg('已儲存');
      hooks?.onSaved?.(n);
    } catch (e) {
      setMsg(e instanceof ApiError && e.status === 412 ? '儲存失敗：檔案已在其他地方被修改。請複製你的變更後重新開啟檔案。' : `儲存失敗：${errorText(e)}`);
    } finally {
      setSaving(false);
    }
  }, [dirty, draft, saving, node.id, sha, hooks]);

  useEffect(() => {
    hooks?.register?.(canEdit ? save : null);
    return () => hooks?.register?.(null);
  }, [save, canEdit, hooks]);

  useEffect(() => {
    if (!msg || msg.startsWith('儲存失敗')) return;
    const t = setTimeout(() => setMsg(''), 2000);
    return () => clearTimeout(t);
  }, [msg]);

  const controls = canEdit ? (
    <>
      {!hooks?.startEditing && (
        <button className={`btn ghost sm ${editing ? 'active' : ''}`} onClick={() => setEditing((e) => !e)} aria-pressed={editing}>
          <Icon name="edit" /> {editing ? '編輯中' : '編輯'}
        </button>
      )}
      {editing && (
        <button className="btn sm primary" onClick={save} disabled={!dirty || saving} title="儲存（Ctrl+S）">
          <Icon name="save" /> {saving ? '儲存中…' : dirty ? '儲存' : '已儲存'}
        </button>
      )}
      {msg && <span className={msg.startsWith('儲存失敗') ? 'err small' : 'ok small'}>{msg}</span>}
    </>
  ) : null;

  return { canEdit, editing: canEdit && editing, text, setDraft, dirty, save, controls };
}
