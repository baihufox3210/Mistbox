import { useEffect, useRef, useState } from 'react';
import { api, errorText, setCsrf, type NodeInfo } from '../api';
import { formatSize } from '../format';
import { Icon } from './Icon';
import { useCaps } from '../caps';

export function Modal({
  title,
  onClose,
  children,
  wide,
}: {
  title: string;
  onClose: () => void;
  children: React.ReactNode;
  wide?: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    const first = ref.current?.querySelector<HTMLElement>('[data-autofocus], input, select, textarea, button.primary, button');
    first?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        onClose();
      }
      if (e.key === 'Tab' && ref.current) {
        // Simple focus trap.
        const f = Array.from(
          ref.current.querySelectorAll<HTMLElement>('a[href], button:not([disabled]), input, select, textarea, [tabindex="0"]'),
        );
        if (!f.length) return;
        const [a, z] = [f[0], f[f.length - 1]];
        if (e.shiftKey && document.activeElement === a) {
          e.preventDefault();
          z.focus();
        } else if (!e.shiftKey && document.activeElement === z) {
          e.preventDefault();
          a.focus();
        }
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => {
      window.removeEventListener('keydown', onKey, true);
      prev?.focus?.();
    };
  }, [onClose]);
  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className={`modal ${wide ? 'wide' : ''}`} role="dialog" aria-modal="true" aria-label={title} ref={ref}>
        <div className="modal-head">
          <h2>{title}</h2>
          <button className="btn ghost icon-only" onClick={onClose} aria-label="關閉">
            <Icon name="close" />
          </button>
        </div>
        <div className="modal-body">{children}</div>
      </div>
    </div>
  );
}

export function PromptDialog({
  title,
  label,
  initial = '',
  confirm,
  onSubmit,
  onClose,
}: {
  title: string;
  label: string;
  initial?: string;
  confirm: string;
  onSubmit: (v: string) => Promise<void>;
  onClose: () => void;
}) {
  const [v, setV] = useState(initial);
  const [err, setErr] = useState('');
  const [busy, setBusy] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    // Select the stem (before the extension) for rename.
    const el = input.current;
    if (el) {
      const dot = initial.lastIndexOf('.');
      el.setSelectionRange(0, dot > 0 ? dot : initial.length);
    }
  }, [initial]);
  const invalid = !v.trim()
    ? '名稱不可空白'
    : /[/\\\u0000-\u001f\u007f]/.test(v)
      ? '名稱不可包含 / \\ 或控制字元'
      : v === '.' || v === '..'
        ? '名稱不可為 . 或 ..'
        : '';
  return (
    <Modal title={title} onClose={onClose}>
      <form
        onSubmit={async (e) => {
          e.preventDefault();
          if (invalid) return setErr(invalid);
          setBusy(true);
          try {
            await onSubmit(v.trim());
            onClose();
          } catch (e) {
            setErr(errorText(e));
            setBusy(false);
          }
        }}
      >
        <label className="field">
          {label}
          <input ref={input} value={v} onChange={(e) => (setV(e.target.value), setErr(''))} data-autofocus />
        </label>
        {err && <p className="form-error" role="alert">{err}</p>}
        <div className="modal-actions">
          <button type="button" className="btn" onClick={onClose}>取消</button>
          <button type="submit" className="btn primary" disabled={busy || !!invalid}>{confirm}</button>
        </div>
      </form>
    </Modal>
  );
}

/** Delete = move to the recycle bin (undoable); optionally skip the bin. */
export function DeleteDialog({
  items,
  onDone,
  onClose,
  inTrash = false,
}: {
  items: NodeInfo[];
  onDone: (permanent: boolean) => void;
  onClose: () => void;
  /** Items are already in the recycle bin → deletion is permanent. */
  inTrash?: boolean;
}) {
  const [stats, setStats] = useState<{ files: number; dirs: number; bytes: number } | null>(null);
  const [err, setErr] = useState('');
  const [busy, setBusy] = useState(false);
  const [permanent, setPermanent] = useState(inTrash);
  const { admin } = useCaps();
  useEffect(() => {
    let cancelled = false;
    Promise.all(items.map((i) => (i.kind === 'dir' ? api.info(i.id, true).then((n) => n.stats!) : Promise.resolve({ files: 1, dirs: 0, bytes: i.size }))))
      .then((all) => {
        if (!cancelled)
          setStats(all.reduce((a, s) => ({ files: a.files + s.files, dirs: a.dirs + s.dirs, bytes: a.bytes + s.bytes }), { files: 0, dirs: 0, bytes: 0 }));
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [items]);
  return (
    <Modal title={permanent ? '永久刪除' : '移到垃圾桶'} onClose={onClose}>
      <p>
        {permanent ? (
          <>
            即將<strong>永久刪除</strong>下列 {items.length} 個項目，<strong>無法復原</strong>：
          </>
        ) : (
          <>下列 {items.length} 個項目會移到垃圾桶，{admin ? '可在垃圾桶中還原' : '之後只有分享者能還原'}：</>
        )}
      </p>
      <ul className="delete-list">
        {items.slice(0, 8).map((i) => (
          <li key={i.id}>
            <Icon name={i.kind === 'dir' ? 'folder' : 'file'} /> {i.name}
          </li>
        ))}
        {items.length > 8 && <li>…以及其他 {items.length - 8} 項</li>}
      </ul>
      <p className="muted">
        {stats ? `共 ${stats.files} 個檔案、${Math.max(0, stats.dirs)} 個資料夾，${formatSize(stats.bytes)}。` : '計算內容中…'}
      </p>
      {!inTrash && admin && (
        <label className="check">
          <input type="checkbox" checked={permanent} onChange={(e) => setPermanent(e.target.checked)} /> 略過垃圾桶，直接永久刪除
        </label>
      )}
      {err && <p className="form-error" role="alert">{err}</p>}
      <div className="modal-actions">
        <button className="btn" onClick={onClose} data-autofocus>取消</button>
        <button
          className={`btn ${permanent ? 'danger' : 'primary'}`}
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            try {
              for (const i of items) await api.remove(i.id, permanent && !inTrash);
              onDone(permanent);
              onClose();
            } catch (e) {
              setErr(errorText(e));
              setBusy(false);
              onDone(permanent);
            }
          }}
        >
          <Icon name="trash" /> {permanent ? '永久刪除' : '移到垃圾桶'}
        </button>
      </div>
    </Modal>
  );
}

/** Folder picker (lazy tree). Optionally allows picking files too. */
export function NodePicker({
  value,
  onChange,
  allowFiles,
  disabledIds = [],
}: {
  value: string;
  onChange: (n: { id: string; name: string; kind: string; path: string }) => void;
  allowFiles?: boolean;
  disabledIds?: string[];
}) {
  return (
    <div className="picker" role="tree" aria-label="選擇位置">
      <PickerNode id="root" name="我的雲端" kind="dir" path="/" depth={0} value={value} onChange={onChange} allowFiles={allowFiles} disabledIds={disabledIds} initiallyOpen />
    </div>
  );
}

function PickerNode(props: {
  id: string;
  name: string;
  kind: string;
  path: string;
  depth: number;
  value: string;
  onChange: (n: { id: string; name: string; kind: string; path: string }) => void;
  allowFiles?: boolean;
  disabledIds: string[];
  initiallyOpen?: boolean;
}) {
  const { id, name, kind, path, depth, value, onChange, allowFiles, disabledIds } = props;
  const [open, setOpen] = useState(!!props.initiallyOpen);
  const [kids, setKids] = useState<NodeInfo[] | null>(null);
  const disabled = disabledIds.includes(id);
  useEffect(() => {
    if (open && kids === null && kind === 'dir') api.children(id).then((l) => setKids(l.children), () => setKids([]));
  }, [open, kids, id, kind]);
  const visibleKids = (kids ?? []).filter((k) => allowFiles || k.kind === 'dir');
  return (
    <div role="treeitem" aria-expanded={kind === 'dir' ? open : undefined} aria-selected={value === id}>
      <div className={`picker-row ${value === id ? 'selected' : ''} ${disabled ? 'disabled' : ''}`} style={{ paddingLeft: 8 + depth * 16 }}>
        {kind === 'dir' ? (
          <button className="twisty" onClick={() => setOpen((o) => !o)} aria-label={open ? '收合' : '展開'}>
            <Icon name="chevron" className={open ? 'rot90' : ''} size={14} />
          </button>
        ) : (
          <span className="twisty" />
        )}
        <button className="picker-label" disabled={disabled} onClick={() => onChange({ id, name, kind, path })}>
          <Icon name={kind === 'dir' ? 'folder' : 'file'} /> {name}
        </button>
      </div>
      {open &&
        visibleKids.map((k) => (
          <PickerNode
            key={k.id}
            id={k.id}
            name={k.name}
            kind={k.kind}
            path={(path === '/' ? '' : path) + '/' + k.name}
            depth={depth + 1}
            value={value}
            onChange={onChange}
            allowFiles={allowFiles}
            disabledIds={disabledIds}
          />
        ))}
    </div>
  );
}

export function MoveDialog({ items, onDone, onClose }: { items: NodeInfo[]; onDone: () => void; onClose: () => void }) {
  const [dest, setDest] = useState<{ id: string; path: string } | null>(null);
  const [err, setErr] = useState('');
  const [busy, setBusy] = useState(false);
  return (
    <Modal title={`移動 ${items.length} 個項目`} onClose={onClose}>
      <p className="muted">選擇目標資料夾（不能移入自己或子資料夾）：</p>
      <NodePicker value={dest?.id ?? ''} onChange={(n) => setDest(n)} disabledIds={items.filter((i) => i.kind === 'dir').map((i) => i.id)} />
      <p>目標：<strong>{dest?.path ?? '尚未選擇'}</strong></p>
      {err && <p className="form-error" role="alert">{err}</p>}
      <div className="modal-actions">
        <button className="btn" onClick={onClose}>取消</button>
        <button
          className="btn primary"
          disabled={!dest || busy}
          onClick={async () => {
            if (!dest) return;
            setBusy(true);
            try {
              for (const i of items) await api.update(i.id, { parent_id: dest.id });
              onDone();
              onClose();
            } catch (e) {
              setErr(errorText(e));
              setBusy(false);
              onDone();
            }
          }}
        >
          移動到這裡
        </button>
      </div>
    </Modal>
  );
}

export function PasswordDialog({ onClose }: { onClose: () => void }) {
  const [cur, setCur] = useState('');
  const [next, setNext] = useState('');
  const [again, setAgain] = useState('');
  const [msg, setMsg] = useState('');
  const [ok, setOk] = useState(false);
  return (
    <Modal title="變更密碼" onClose={onClose}>
      <form
        onSubmit={async (e) => {
          e.preventDefault();
          if (next !== again) return setMsg('兩次輸入的新密碼不同');
          if (next.length < 10) return setMsg('新密碼至少 10 個字元');
          try {
            const r = await api.changePassword(cur, next);
            setCsrf(r.csrf_token);
            setOk(true);
            setMsg('密碼已更新，其他裝置的登入已全部登出。');
          } catch (e) {
            setMsg(errorText(e));
          }
        }}
      >
        <label className="field">目前密碼<input type="password" autoComplete="current-password" value={cur} onChange={(e) => setCur(e.target.value)} /></label>
        <label className="field">新密碼（至少 10 字元）<input type="password" autoComplete="new-password" value={next} onChange={(e) => setNext(e.target.value)} /></label>
        <label className="field">再次輸入新密碼<input type="password" autoComplete="new-password" value={again} onChange={(e) => setAgain(e.target.value)} /></label>
        {msg && <p className={ok ? 'form-ok' : 'form-error'} role="status">{msg}</p>}
        <div className="modal-actions">
          <button type="button" className="btn" onClick={onClose}>關閉</button>
          {!ok && <button className="btn primary" type="submit">更新密碼</button>}
        </div>
      </form>
    </Modal>
  );
}
