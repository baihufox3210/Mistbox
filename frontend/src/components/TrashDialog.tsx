import { useCallback, useEffect, useState } from 'react';
import { api, errorText, type TrashItem } from '../api';
import { formatDate, formatSize } from '../format';
import { FileIcon } from './FileIcon';
import { Icon } from './Icon';
import { DeleteDialog, Modal } from './Modal';

/** Recycle bin: restore, delete permanently, empty. */
export default function TrashDialog({ onClose, onChanged }: { onClose: () => void; onChanged: (msg?: string) => void }) {
  const [items, setItems] = useState<TrashItem[] | null>(null);
  const [days, setDays] = useState(30);
  const [err, setErr] = useState('');
  const [confirm, setConfirm] = useState<TrashItem[] | 'all' | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const load = useCallback(() => {
    api.trash().then((r) => (setItems(r.items), setDays(r.retention_days)), (e) => setErr(errorText(e)));
  }, []);
  useEffect(load, [load]);

  const restore = async (it: TrashItem) => {
    setBusy(it.id);
    try {
      const r = await api.restore(it.id);
      onChanged(r.restored_to_root ? `原資料夾已不存在，已還原到 ${r.node.path}` : `已還原到 ${r.node.path}`);
      load();
    } catch (e) {
      setErr(errorText(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <Modal title="垃圾桶" onClose={onClose} wide>
      <div className="row between">
        <p className="muted small">
          刪除的項目會先移到這裡{days > 0 ? `，${days} 天後自動永久刪除` : '，不會自動清除'}。還原時若原位置已有同名項目，會自動加上編號，不會覆寫。
        </p>
        <button className="btn danger" disabled={!items?.length} onClick={() => setConfirm('all')}>
          <Icon name="trash" /> 清空垃圾桶
        </button>
      </div>
      {err && <p className="form-error" role="alert">{err}</p>}
      {!items && !err && <div className="state"><span className="spinner" /> 載入中…</div>}
      {items && items.length === 0 && (
        <div className="state empty">
          <Icon name="trash" size={36} />
          <p>垃圾桶是空的</p>
        </div>
      )}
      {items && items.length > 0 && (
        <div className="table-wrap">
          <table className="keys-table trash-table">
            <thead>
              <tr>
                <th>名稱</th>
                <th>原位置</th>
                <th>大小</th>
                <th>刪除時間</th>
                <th>永久刪除於</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {items.map((it) => (
                <tr key={it.id}>
                  <td>
                    <span className="name-cell">
                      <FileIcon name={it.name} kind={it.kind} size={18} /> {it.name}
                    </span>
                  </td>
                  <td className="muted">{it.path}</td>
                  <td>{it.kind === 'dir' ? `${(it as TrashItem & { stats?: { files: number } }).stats?.files ?? 0} 個檔案` : formatSize(it.size)}</td>
                  <td>{formatDate(it.trashed_at)}</td>
                  <td>{it.purge_at ? formatDate(it.purge_at) : '—'}</td>
                  <td className="actions">
                    <button className="btn sm" disabled={busy === it.id} onClick={() => restore(it)}>
                      <Icon name="refresh" /> 還原
                    </button>
                    <button className="btn sm danger-ghost" onClick={() => setConfirm([it])}>
                      永久刪除
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {confirm === 'all' && (
        <Modal title="清空垃圾桶" onClose={() => setConfirm(null)}>
          <p>
            垃圾桶中的 {items?.length ?? 0} 個項目將被<strong>永久刪除，無法復原</strong>。
          </p>
          <div className="modal-actions">
            <button className="btn" onClick={() => setConfirm(null)} data-autofocus>取消</button>
            <button
              className="btn danger"
              onClick={async () => {
                try {
                  const r = await api.emptyTrash();
                  onChanged(`已永久刪除 ${r.deleted} 個項目`);
                } catch (e) {
                  setErr(errorText(e));
                }
                setConfirm(null);
                load();
              }}
            >
              <Icon name="trash" /> 永久刪除全部
            </button>
          </div>
        </Modal>
      )}
      {Array.isArray(confirm) && (
        <DeleteDialog
          items={confirm}
          inTrash
          onClose={() => setConfirm(null)}
          onDone={() => {
            onChanged();
            load();
          }}
        />
      )}
    </Modal>
  );
}
