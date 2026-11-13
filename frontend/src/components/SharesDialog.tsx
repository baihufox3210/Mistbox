// Share links: create one for a file / folder (permission + expiry), and list,
// change or delete existing links. With no `node`, lists every link.
import { useCallback, useEffect, useState } from 'react';
import { api, errorText, type NodeInfo, type ShareInfo, type SharePermission } from '../api';
import { formatDate } from '../format';
import { Icon } from './Icon';
import { Modal } from './Modal';

const STATUS: Record<ShareInfo['status'], string> = { active: '有效', expired: '已過期', unavailable: '項目已移除' };

const EXPIRY: { value: string; label: string; hours: number | null }[] = [
  { value: '1h', label: '1 小時', hours: 1 },
  { value: '1d', label: '1 天', hours: 24 },
  { value: '7d', label: '7 天', hours: 24 * 7 },
  { value: '30d', label: '30 天', hours: 24 * 30 },
  { value: 'never', label: '永不過期', hours: null },
  { value: 'custom', label: '自訂…', hours: null },
];

/** RFC 3339 expiry for a preset, or for a `datetime-local` value. */
function expiryIso(preset: string, custom: string): string | null | 'invalid' {
  if (preset === 'custom') {
    const t = new Date(custom);
    return custom && !Number.isNaN(t.getTime()) && t.getTime() > Date.now() ? t.toISOString() : 'invalid';
  }
  const h = EXPIRY.find((e) => e.value === preset)?.hours;
  return h ? new Date(Date.now() + h * 3600_000).toISOString() : null;
}

function ExpiryPicker({ preset, setPreset, custom, setCustom }: { preset: string; setPreset: (v: string) => void; custom: string; setCustom: (v: string) => void }) {
  return (
    <span className="expiry-picker">
      <select value={preset} onChange={(e) => setPreset(e.target.value)} aria-label="有效期限">
        {EXPIRY.map((e) => (
          <option key={e.value} value={e.value}>{e.label}</option>
        ))}
      </select>
      {preset === 'custom' && (
        <input type="datetime-local" value={custom} onChange={(e) => setCustom(e.target.value)} aria-label="自訂到期時間" />
      )}
    </span>
  );
}

export default function SharesDialog({ node, onClose, toast }: { node?: NodeInfo; onClose: () => void; toast: (m: string) => void }) {
  const [shares, setShares] = useState<ShareInfo[] | null>(null);
  const [error, setError] = useState('');
  const [created, setCreated] = useState<string | null>(null);
  const [confirmDel, setConfirmDel] = useState<ShareInfo | null>(null);

  const load = useCallback(() => {
    setError('');
    api.shares(node?.id).then((r) => setShares(r.shares), (e) => setError(errorText(e)));
  }, [node?.id]);
  useEffect(load, [load]);

  const update = async (s: ShareInfo, patch: Parameters<typeof api.updateShare>[1]) => {
    try {
      const n = await api.updateShare(s.id, patch);
      setShares((l) => l?.map((x) => (x.id === n.id ? n : x)) ?? null);
      toast('已更新分享連結');
    } catch (e) {
      setError(errorText(e));
    }
  };

  return (
    <Modal title={node ? `分享「${node.name}」` : '分享連結'} onClose={onClose} wide>
      {node && (created ? <NewLink link={created} onDone={() => setCreated(null)} /> : <CreateShare node={node} onCreated={(l) => (setCreated(l), load())} />)}

      {node && <h3 className="share-sub">這個項目的分享連結</h3>}
      {!node && (
        <p className="muted small">
          在檔案或資料夾上按右鍵 →「分享連結…」即可建立。拿到連結的人不需要登入，只能看到被分享的項目（資料夾含所有子項目），永遠看不到敏感檔與垃圾桶。
        </p>
      )}
      {error && (
        <div className="state error">
          {error} <button className="btn sm" onClick={load}>重試</button>
        </div>
      )}
      {!shares && !error && <div className="state"><span className="spinner" /> 載入中…</div>}
      {shares && shares.length === 0 && <div className="state">尚未建立任何分享連結。</div>}
      {shares && shares.length > 0 && (
        <div className="table-wrap">
          <table className="keys-table share-table">
            <thead>
              <tr>
                {!node && <th>項目</th>}
                <th>權限</th>
                <th>到期</th>
                <th>狀態</th>
                <th>開啟次數</th>
                <th>最近使用</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {shares.map((s) => (
                <ShareRow key={s.id} s={s} showNode={!node} onUpdate={(p) => update(s, p)} onDelete={() => setConfirmDel(s)} />
              ))}
            </tbody>
          </table>
        </div>
      )}
      <p className="muted small">完整連結含有密鑰，只在建立時顯示一次；伺服器只保存雜湊值。要再分享給別人，請建立新的連結。</p>

      {confirmDel && (
        <Modal title="刪除分享連結" onClose={() => setConfirmDel(null)}>
          <p>刪除後，持有這個連結的人會立即無法再開啟{confirmDel.node ? `「${confirmDel.node.name}」` : ''}。檔案本身不受影響。</p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirmDel(null)} data-autofocus>取消</button>
            <button
              className="btn danger"
              onClick={async () => {
                try {
                  await api.deleteShare(confirmDel.id);
                  toast('已刪除分享連結');
                } catch (e) {
                  setError(errorText(e));
                }
                setConfirmDel(null);
                load();
              }}
            >
              刪除連結
            </button>
          </div>
        </Modal>
      )}
    </Modal>
  );
}

function PermissionChoice({ kind, value, onChange }: { kind: 'dir' | 'file'; value: SharePermission; onChange: (p: SharePermission) => void }) {
  return (
    <fieldset className="field">
      <legend>權限</legend>
      <label>
        <input type="radio" name="share-perm" checked={value === 'view'} onChange={() => onChange('view')} /> <Icon name="eye" size={15} /> 檢視（瀏覽、預覽、下載）
      </label>
      <label>
        <input type="radio" name="share-perm" checked={value === 'edit'} onChange={() => onChange('edit')} /> <Icon name="edit" size={15} />{' '}
        {kind === 'dir' ? '可編輯（另可上傳、新增、改名、移動、移到垃圾桶、修改文字檔）' : '可編輯（另可修改這個文字檔的內容）'}
      </label>
    </fieldset>
  );
}

function CreateShare({ node, onCreated }: { node: NodeInfo; onCreated: (link: string) => void }) {
  const [perm, setPerm] = useState<SharePermission>('view');
  const [preset, setPreset] = useState('7d');
  const [custom, setCustom] = useState('');
  const [label, setLabel] = useState('');
  const [err, setErr] = useState('');
  const [busy, setBusy] = useState(false);
  return (
    <form
      className="create-share"
      onSubmit={async (e) => {
        e.preventDefault();
        const expires_at = expiryIso(preset, custom);
        if (expires_at === 'invalid') return setErr('請選擇未來的到期時間');
        setBusy(true);
        setErr('');
        try {
          const r = await api.createShare({ node_id: node.id, permission: perm, expires_at, label: label.trim() });
          onCreated(window.location.origin + r.link);
        } catch (e) {
          setErr(errorText(e));
        } finally {
          setBusy(false);
        }
      }}
    >
      <PermissionChoice kind={node.kind} value={perm} onChange={setPerm} />
      {perm === 'edit' && (
        <p className="notice warn small">任何拿到連結的人都能修改內容。刪除的項目會進入你的垃圾桶，可以還原。</p>
      )}
      <div className="field">
        有效期限
        <ExpiryPicker preset={preset} setPreset={setPreset} custom={custom} setCustom={setCustom} />
      </div>
      <label className="field">
        備註（選填，只有你看得到）
        <input value={label} onChange={(e) => setLabel(e.target.value)} placeholder="例如：給王小明" maxLength={100} />
      </label>
      {err && <p className="form-error" role="alert">{err}</p>}
      <div className="modal-actions">
        <button className="btn primary" type="submit" disabled={busy}>
          <Icon name="link" /> {busy ? '建立中…' : '建立連結'}
        </button>
      </div>
    </form>
  );
}

function NewLink({ link, onDone }: { link: string; onDone: () => void }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    await navigator.clipboard.writeText(link).catch(() => {});
    setCopied(true);
  };
  useEffect(() => {
    copy();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return (
    <div className="new-key">
      <div className="notice warn">請現在複製並保存這個連結：它只會顯示這一次（伺服器只保存雜湊值）。</div>
      <div className="secret-box">
        <code>{link}</code>
        <button className="btn sm" onClick={copy}>
          <Icon name="copy" /> {copied ? '已複製' : '複製'}
        </button>
      </div>
      <div className="modal-actions">
        <button className="btn" onClick={onDone}>完成</button>
      </div>
    </div>
  );
}

function ShareRow({
  s,
  showNode,
  onUpdate,
  onDelete,
}: {
  s: ShareInfo;
  showNode: boolean;
  onUpdate: (p: Parameters<typeof api.updateShare>[1]) => void;
  onDelete: () => void;
}) {
  const [editingExpiry, setEditingExpiry] = useState(false);
  const [preset, setPreset] = useState('7d');
  const [custom, setCustom] = useState('');
  return (
    <tr className={s.status !== 'active' ? 'inactive' : ''}>
      {showNode && (
        <td>
          <Icon name={s.node?.kind === 'file' ? 'file' : 'folder'} /> {s.node?.path ?? '（已刪除）'}
          {s.label && <div className="muted small">{s.label}</div>}
        </td>
      )}
      <td>
        <select value={s.permission} onChange={(e) => onUpdate({ permission: e.target.value as SharePermission })} aria-label="權限">
          <option value="view">檢視</option>
          <option value="edit">可編輯</option>
        </select>
        {!showNode && s.label && <div className="muted small">{s.label}</div>}
      </td>
      <td>
        {editingExpiry ? (
          <span className="expiry-edit">
            <ExpiryPicker preset={preset} setPreset={setPreset} custom={custom} setCustom={setCustom} />
            <button
              className="btn sm primary"
              onClick={() => {
                const e = expiryIso(preset, custom);
                if (e === 'invalid') return;
                onUpdate({ expires_at: e });
                setEditingExpiry(false);
              }}
            >
              套用
            </button>
            <button className="btn sm" onClick={() => setEditingExpiry(false)}>取消</button>
          </span>
        ) : (
          <button className="btn ghost sm" onClick={() => setEditingExpiry(true)} title="修改到期時間">
            {s.expires_at ? formatDate(s.expires_at) : '永不過期'} <Icon name="edit" size={12} />
          </button>
        )}
      </td>
      <td>
        <span className={`badge ${s.status === 'active' ? 'ok' : 'muted'}`}>{STATUS[s.status]}</span>
      </td>
      <td>{s.use_count}</td>
      <td>{s.last_used_at ? formatDate(s.last_used_at) : '—'}</td>
      <td>
        <button className="btn danger sm" onClick={onDelete}>刪除</button>
      </td>
    </tr>
  );
}
