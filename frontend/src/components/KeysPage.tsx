import { useCallback, useEffect, useState } from 'react';
import { api, errorText, type ApiKeyInfo } from '../api';
import { formatDate } from '../format';
import { Icon } from './Icon';
import { Modal, NodePicker } from './Modal';

const STATUS: Record<ApiKeyInfo['status'], string> = {
  active: '有效',
  revoked: '已撤銷',
  expired: '已過期',
  scope_deleted: '範圍已刪除',
};

export default function KeysPage({ onClose }: { onClose: () => void }) {
  const [keys, setKeys] = useState<ApiKeyInfo[] | null>(null);
  const [error, setError] = useState('');
  const [creating, setCreating] = useState(false);
  const [created, setCreated] = useState<string | null>(null);
  const [confirmRevoke, setConfirmRevoke] = useState<ApiKeyInfo | null>(null);

  const load = useCallback(() => {
    setError('');
    api.keys().then((r) => setKeys(r.keys), (e) => setError(errorText(e)));
  }, []);
  useEffect(load, [load]);

  return (
    <Modal title="API Keys" onClose={onClose} wide>
      {created ? (
        <NewKey secret={created} onDone={() => setCreated(null)} />
      ) : creating ? (
        <CreateKey
          onCancel={() => setCreating(false)}
          onCreated={(k) => {
            setCreating(false);
            setCreated(k);
            load();
          }}
        />
      ) : (
        <>
          <div className="dialog-toolbar">
            <button className="btn primary" onClick={() => setCreating(true)}>
              <Icon name="plus" /> 建立 Key
            </button>
          </div>
          {error && (
            <div className="state error">
              {error} <button className="btn sm" onClick={load}>重試</button>
            </div>
          )}
          {!keys && !error && <div className="state"><span className="spinner" /> 載入中…</div>}
          {keys && keys.length === 0 && <div className="state">尚未建立任何 API Key。</div>}
          {keys && keys.length > 0 && (
            <div className="table-wrap">
              <table className="keys-table">
                <thead>
                  <tr>
                    <th>名稱</th>
                    <th>權限</th>
                    <th>範圍</th>
                    <th>狀態</th>
                    <th>建立</th>
                    <th>到期</th>
                    <th>最近使用</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  {keys.map((k) => (
                    <tr key={k.id} className={k.status !== 'active' ? 'inactive' : ''}>
                      <td>
                        <div>{k.name}</div>
                        <code className="small">{k.prefix}</code>
                      </td>
                      <td>
                        {(['list', 'read', 'write'] as const).filter((p) => k.permissions[p]).map((p) => ({ list: '瀏覽', read: '讀取', write: '管理' })[p]).join(' · ')}
                        {k.allow_sensitive && <div className="badge warn">含敏感檔</div>}
                      </td>
                      <td title={k.scope.covers}>
                        <Icon name={k.scope.kind === 'file' ? 'file' : 'folder'} /> {k.scope.path ?? '（已刪除）'}
                      </td>
                      <td>
                        <span className={`badge ${k.status === 'active' ? 'ok' : 'muted'}`}>{STATUS[k.status]}</span>
                      </td>
                      <td>{formatDate(k.created_at)}</td>
                      <td>{k.expires_at ? formatDate(k.expires_at) : '不過期'}</td>
                      <td>{formatDate(k.last_used_at)}</td>
                      <td>
                        {k.status !== 'revoked' && (
                          <button className="btn danger sm" onClick={() => setConfirmRevoke(k)}>
                            撤銷
                          </button>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}
      {confirmRevoke && (
        <Modal title="撤銷 API Key" onClose={() => setConfirmRevoke(null)}>
          <p>
            撤銷「{confirmRevoke.name}」後立即失效，使用中的程式會收到 401。此動作無法復原。
          </p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirmRevoke(null)} data-autofocus>取消</button>
            <button
              className="btn danger"
              onClick={async () => {
                try {
                  await api.revokeKey(confirmRevoke.id);
                } catch (e) {
                  setError(errorText(e));
                }
                setConfirmRevoke(null);
                load();
              }}
            >
              撤銷
            </button>
          </div>
        </Modal>
      )}
    </Modal>
  );
}

function CreateKey({ onCancel, onCreated }: { onCancel: () => void; onCreated: (secret: string) => void }) {
  const [name, setName] = useState('');
  const [perm, setPerm] = useState({ list: true, read: true, write: false });
  const [scope, setScope] = useState<{ id: string; path: string; kind: string }>({ id: 'root', path: '/', kind: 'dir' });
  const [expires, setExpires] = useState('');
  const [sensitive, setSensitive] = useState(false);
  const [err, setErr] = useState('');
  const [busy, setBusy] = useState(false);
  return (
    <form
      className="create-key"
      onSubmit={async (e) => {
        e.preventDefault();
        if (!name.trim()) return setErr('請輸入名稱');
        if (!perm.list && !perm.read && !perm.write) return setErr('至少勾選一項權限');
        setBusy(true);
        try {
          const r = await api.createKey({
            name: name.trim(),
            permissions: perm,
            scope_node_id: scope.id,
            expires_at: expires ? new Date(expires + 'T23:59:59').toISOString() : null,
            allow_sensitive: sensitive,
          });
          onCreated(r.key);
        } catch (e) {
          setErr(errorText(e));
          setBusy(false);
        }
      }}
    >
      <label className="field">
        名稱
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="例如：備份腳本" maxLength={100} data-autofocus />
      </label>
      <fieldset className="field">
        <legend>權限</legend>
        <label><input type="checkbox" checked={perm.list} onChange={(e) => setPerm({ ...perm, list: e.target.checked })} /> 瀏覽與搜尋</label>
        <label><input type="checkbox" checked={perm.read} onChange={(e) => setPerm({ ...perm, read: e.target.checked })} /> 預覽與下載</label>
        <label><input type="checkbox" checked={perm.write} onChange={(e) => setPerm({ ...perm, write: e.target.checked })} /> 上傳與管理檔案</label>
      </fieldset>
      <div className="field">
        授權範圍：<strong>{scope.path}</strong>
        <span className="muted small">
          {scope.kind === 'dir' ? '包含所有子項目' : '僅此檔案'}
        </span>
        <NodePicker value={scope.id} allowFiles onChange={(n) => setScope(n)} />
      </div>
      <label className="field">
        到期日（留空為不過期）
        <input type="date" value={expires} min={new Date().toISOString().slice(0, 10)} onChange={(e) => setExpires(e.target.value)} />
      </label>
      <label className="check">
        <input type="checkbox" checked={sensitive} onChange={(e) => setSensitive(e.target.checked)} /> 允許存取敏感檔案
      </label>
      {err && <p className="form-error" role="alert">{err}</p>}
      <div className="modal-actions">
        <button type="button" className="btn" onClick={onCancel}>取消</button>
        <button className="btn primary" type="submit" disabled={busy}>建立</button>
      </div>
    </form>
  );
}

function NewKey({ secret, onDone }: { secret: string; onDone: () => void }) {
  const [copied, setCopied] = useState(false);
  const origin = window.location.origin;
  return (
    <div className="new-key">
      <div className="notice warn">請保存此 Key，關閉後將無法再次查看。</div>
      <div className="secret-box">
        <code>{secret}</code>
        <button
          className="btn sm"
          onClick={async () => {
            await navigator.clipboard.writeText(secret).catch(() => {});
            setCopied(true);
          }}
        >
          <Icon name="copy" /> {copied ? '已複製' : '複製'}
        </button>
      </div>
      <details className="usage-example">
      <summary>API 使用範例</summary>
      <p className="muted small">請將 Key 放在 Authorization 標頭，勿加入網址。</p>
      <pre className="example">{`export MISTBOX_KEY='${secret.slice(0, 24)}…'
curl -H "Authorization: Bearer $MISTBOX_KEY" "${origin}/api/v1/fs/list?path=/"
curl -H "Authorization: Bearer $MISTBOX_KEY" -T report.pdf "${origin}/api/v1/fs/upload?path=/report.pdf"
curl -H "Authorization: Bearer $MISTBOX_KEY" -OJ "${origin}/api/v1/fs/download?path=/report.pdf"`}</pre>
      </details>
      <div className="modal-actions">
        <button className="btn primary" onClick={onDone}>我已妥善保存</button>
      </div>
    </div>
  );
}
