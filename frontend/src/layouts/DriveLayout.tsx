// Drive layout (file-manager style): folders and files in separate sections
// with big icons / image thumbnails, or a detailed list; previews open in a
// full-screen overlay.
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api, contentUrl, type NodeInfo } from '../api';
import { ContextMenu, type MenuState } from '../components/ContextMenu';
import { ErrorState } from '../components/ErrorState';
import { FileIcon } from '../components/FileIcon';
import { Icon } from '../components/Icon';
import { UploadPanel } from '../components/Uploads';
import { previewKind, typeLabel } from '../fileTypes';
import { formatDate, formatSize } from '../format';
import type { DriveModel } from '../state/useDrive';
import type { Layout } from '../theme';
import { DND_TYPE, dropMove, emptyMenu, nodeMenu, ragLabel, setAsBackground, useRagStatus, useTabHooks, useTree } from './shared';

const Preview = lazy(() => import('../preview/Preview'));

type SortKey = 'name' | 'modified' | 'size' | 'type';

export default function DriveLayout({
  m,
  sideOpen,
  setSideOpen,
  targetRef,
  setLayout,
}: {
  m: DriveModel;
  sideOpen: boolean;
  setSideOpen: (o: boolean) => void;
  targetRef: React.MutableRefObject<() => string>;
  setLayout: (l: Layout) => void;
}) {
  const [view, setView] = useState<'grid' | 'list'>((localStorage.getItem('mistbox.driveView') as 'grid' | 'list') || 'grid');
  const [sort, setSort] = useState<{ key: SortKey; asc: boolean }>({ key: 'name', asc: true });
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [cursor, setCursor] = useState(0);
  const [menu, setMenu] = useState<MenuState | null>(null);
  const [newMenu, setNewMenu] = useState<MenuState | null>(null);
  const [dragOver, setDragOver] = useState<string | null>(null);
  const [stats, setStats] = useState<{ files: number; bytes: number } | null>(null);
  const tree = useTree(m.version);
  const rag = useRagStatus();
  const main = useRef<HTMLDivElement>(null);

  useEffect(() => localStorage.setItem('mistbox.driveView', view), [view]);
  useEffect(() => {
    setSelected(new Set());
    setCursor(0);
  }, [m.dirId]);
  useEffect(() => {
    targetRef.current = () => m.dirId;
  }, [m.dirId, targetRef]);
  useEffect(() => {
    api.info('root', true).then((n) => n.stats && setStats({ files: n.stats.files, bytes: n.stats.bytes }), () => {});
  }, [m.version]);
  // Expand the sidebar tree along the current path.
  useEffect(() => {
    if (m.listing) tree.expandAll(m.listing.breadcrumbs.map((b) => b.id));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [m.listing]);

  const items = useMemo(() => {
    const list = [...(m.listing?.children ?? [])];
    const d = sort.asc ? 1 : -1;
    list.sort((a, b) => {
      let r = 0;
      if (sort.key === 'size') r = a.size - b.size;
      else if (sort.key === 'modified') r = a.modified_at.localeCompare(b.modified_at);
      else if (sort.key === 'type') r = typeLabel(a).localeCompare(typeLabel(b));
      return (r || a.name.localeCompare(b.name, 'zh-Hant', { numeric: true })) * d;
    });
    return [...list.filter((n) => n.kind === 'dir'), ...list.filter((n) => n.kind === 'file')];
  }, [m.listing, sort]);
  const folders = items.filter((n) => n.kind === 'dir');
  const files = items.filter((n) => n.kind === 'file');
  const selItems = items.filter((n) => selected.has(n.id));

  const open = useCallback((n: NodeInfo) => (n.kind === 'dir' ? m.navigate(n.id) : m.openFile(n)), [m]);

  const pick = (i: number, e?: { ctrlKey?: boolean; metaKey?: boolean; shiftKey?: boolean }) => {
    const n = items[i];
    if (!n) return;
    setCursor(i);
    setSelected((s) => {
      if (e?.shiftKey) return new Set(items.slice(Math.min(cursor, i), Math.max(cursor, i) + 1).map((x) => x.id));
      if (e?.ctrlKey || e?.metaKey) {
        const ns = new Set(s);
        if (ns.has(n.id)) ns.delete(n.id);
        else ns.add(n.id);
        return ns;
      }
      return new Set([n.id]);
    });
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (m.active) return;
    const cols = view === 'grid' ? Math.max(1, Math.floor((main.current?.clientWidth ?? 800) / 196)) : 1;
    const go = (j: number) => {
      const k = Math.max(0, Math.min(items.length - 1, j));
      pick(k, { shiftKey: e.shiftKey });
      main.current?.querySelector<HTMLElement>(`[data-i="${k}"]`)?.focus();
    };
    switch (e.key) {
      case 'ArrowRight':
        go(cursor + (view === 'grid' ? 1 : 0));
        break;
      case 'ArrowLeft':
        go(cursor - (view === 'grid' ? 1 : 0));
        break;
      case 'ArrowDown':
        go(cursor + cols);
        break;
      case 'ArrowUp':
        go(cursor - cols);
        break;
      case 'Enter':
        if (items[cursor]) open(items[cursor]);
        break;
      case 'Backspace': {
        const bc = m.listing?.breadcrumbs ?? [];
        if (bc.length > 1) m.navigate(bc[bc.length - 2].id);
        break;
      }
      case 'Delete':
        m.actions.remove(selItems);
        break;
      case 'F2':
        if (selItems.length === 1) m.actions.rename(selItems[0]);
        break;
      case 'a':
        if (!(e.ctrlKey || e.metaKey)) return;
        setSelected(new Set(items.map((x) => x.id)));
        break;
      case 'Escape':
        setSelected(new Set());
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  const drop = async (e: React.DragEvent, dest: string) => {
    e.preventDefault();
    e.stopPropagation();
    setDragOver(null);
    const internal = e.dataTransfer.getData(DND_TYPE);
    if (internal) {
      const ids = selected.has(internal) ? [...selected] : [internal];
      return dropMove(m, ids, dest);
    }
    m.actions.uploadDrop(dest, e.dataTransfer);
  };

  const itemProps = (n: NodeInfo, i: number) => ({
    'data-i': i,
    tabIndex: i === cursor ? 0 : -1,
    'aria-selected': selected.has(n.id),
    draggable: true,
    onDragStart: (e: React.DragEvent) => e.dataTransfer.setData(DND_TYPE, n.id),
    onDragOver: (e: React.DragEvent) => {
      if (n.kind !== 'dir') return;
      e.preventDefault();
      e.stopPropagation();
      setDragOver(n.id);
    },
    onDragLeave: () => setDragOver((d) => (d === n.id ? null : d)),
    onDrop: (e: React.DragEvent) => n.kind === 'dir' && drop(e, n.id),
    onClick: (e: React.MouseEvent) => pick(i, e),
    onDoubleClick: (e: React.MouseEvent) => (e.stopPropagation(), open(n)),
    onFocus: () => setCursor(i),
    onContextMenu: (e: React.MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (!selected.has(n.id)) setSelected(new Set([n.id]));
      const sel = selected.has(n.id) ? selItems : [n];
      setMenu({ x: e.clientX, y: e.clientY, items: nodeMenu(m, n, sel, () => open(n)) });
    },
  });

  const crumbs = m.listing?.breadcrumbs ?? [];
  const activeTab = m.tabs.find((t) => t.node.id === m.active);

  return (
    <div className="drive" data-region="drive">
      <aside className={`drive-side ${sideOpen ? 'open' : ''}`} data-region="sidebar">
        <button
          className="new-btn"
          onClick={(e) => {
            const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
            setNewMenu({ x: r.left, y: r.bottom + 6, items: emptyMenu(m, m.dirId) });
          }}
        >
          <Icon name="plus" size={20} /> 新增
        </button>
        <nav className="side-tree" aria-label="資料夾">
          <SideNode m={m} tree={tree} node={{ id: 'root', name: '我的雲端' }} depth={0} onDrop={drop} dragOver={dragOver} setDragOver={setDragOver} onPick={() => setSideOpen(false)} />
          <button className="side-row side-trash" onClick={() => (m.setDialog({ t: 'trash' }), setSideOpen(false))}>
            <span className="twisty" />
            <Icon name="trash" size={17} />
            <span className="tree-name">垃圾桶</span>
          </button>
        </nav>
        <div className="side-foot">
          {stats && (
            <div className="side-stat">
              <Icon name="folder" size={14} /> {stats.files.toLocaleString()} 個檔案 · {formatSize(stats.bytes)}
            </div>
          )}
          <button className="side-stat link" onClick={() => m.setDialog({ t: 'settings' })}>
            <Icon name="sparkle" size={14} /> {ragLabel(rag)}
          </button>
        </div>
      </aside>
      {sideOpen && <div className="scrim mobile-only" onClick={() => setSideOpen(false)} />}

      <section
        className={`drive-main ${dragOver === m.dirId ? 'drop' : ''}`}
        ref={main}
        onKeyDown={onKey}
        onDragOver={(e) => {
          e.preventDefault();
          if (dragOver !== m.dirId) setDragOver(m.dirId);
        }}
        onDragLeave={(e) => !e.currentTarget.contains(e.relatedTarget as Node) && setDragOver(null)}
        onDrop={(e) => drop(e, m.dirId)}
        onContextMenu={(e) => {
          e.preventDefault();
          setMenu({ x: e.clientX, y: e.clientY, items: emptyMenu(m, m.dirId) });
        }}
        onDoubleClick={(e) => {
          const t = e.target as HTMLElement;
          if (!t.closest('.item, button, a, input, th')) m.actions.newFile(m.dirId);
        }}
        onClick={(e) => {
          if (!(e.target as HTMLElement).closest('.item')) setSelected(new Set());
        }}
        data-region="drive-main"
      >
        <div className="drive-head">
          <nav className="drive-crumbs" aria-label="路徑">
            {crumbs.map((b, i) => (
              <span key={b.id} className="crumb-wrap">
                {i > 0 && <Icon name="chevron" size={16} className="crumb-sep" />}
                {i === crumbs.length - 1 ? (
                  <span className="crumb current" aria-current="page">{i === 0 ? '我的雲端' : b.name}</span>
                ) : (
                  <button
                    className="crumb"
                    onClick={() => m.navigate(b.id)}
                    onDragOver={(e) => (e.preventDefault(), e.stopPropagation())}
                    onDrop={(e) => drop(e, b.id)}
                  >
                    {i === 0 ? '我的雲端' : b.name}
                  </button>
                )}
              </span>
            ))}
          </nav>
          <div className="drive-tools">
            {selItems.length > 0 ? (
              <div className="sel-bar">
                <span>已選取 {selItems.length} 項</span>
                {selItems.length === 1 && selItems[0].kind === 'file' && (
                  <button className="btn ghost icon-only" onClick={() => m.actions.download(selItems[0])} title="下載" aria-label="下載">
                    <Icon name="download" />
                  </button>
                )}
                {selItems.length === 1 && (
                  <button className="btn ghost icon-only" onClick={() => m.actions.rename(selItems[0])} title="重新命名" aria-label="重新命名">
                    <Icon name="edit" />
                  </button>
                )}
                <button className="btn ghost icon-only" onClick={() => m.actions.move(selItems)} title="移動" aria-label="移動">
                  <Icon name="move" />
                </button>
                <button className="btn ghost icon-only danger-ghost" onClick={() => m.actions.remove(selItems)} title="刪除" aria-label="刪除">
                  <Icon name="trash" />
                </button>
                <button className="btn ghost icon-only" onClick={() => setSelected(new Set())} title="取消選取" aria-label="取消選取">
                  <Icon name="close" />
                </button>
              </div>
            ) : (
              <label className="sort">
                排序
                <select
                  value={`${sort.key}:${sort.asc ? 'asc' : 'desc'}`}
                  onChange={(e) => {
                    const [key, dir] = e.target.value.split(':');
                    setSort({ key: key as SortKey, asc: dir === 'asc' });
                  }}
                >
                  <option value="name:asc">名稱 A→Z</option>
                  <option value="name:desc">名稱 Z→A</option>
                  <option value="modified:desc">最近修改</option>
                  <option value="modified:asc">最早修改</option>
                  <option value="size:desc">最大</option>
                  <option value="size:asc">最小</option>
                  <option value="type:asc">類型</option>
                </select>
              </label>
            )}
            <div className="segmented" role="group" aria-label="檢視">
              <button className={view === 'grid' ? 'on' : ''} aria-pressed={view === 'grid'} onClick={() => setView('grid')} title="格狀">
                <Icon name="grid" />
              </button>
              <button className={view === 'list' ? 'on' : ''} aria-pressed={view === 'list'} onClick={() => setView('list')} title="清單">
                <Icon name="list" />
              </button>
            </div>
          </div>
        </div>

        <div className="drive-body">
          {m.loading && !m.listing ? (
            <div className="state"><span className="spinner" /> 載入中…</div>
          ) : m.listErr ? (
            <ErrorState error={m.listErr} retry={m.reload} />
          ) : items.length === 0 ? (
            <div className="state empty">
              <FileIcon name="" kind="dir" size={72} />
              <p>這個資料夾是空的</p>
              <p className="muted">把檔案或資料夾拖放到這裡上傳、按右鍵新增，或雙擊空白處建立檔案。</p>
            </div>
          ) : view === 'grid' ? (
            <>
              {folders.length > 0 && (
                <>
                  <h3 className="section-title">資料夾</h3>
                  <div className="folder-grid" role="listbox" aria-label="資料夾" aria-multiselectable="true">
                    {folders.map((n) => {
                      const i = items.indexOf(n);
                      return (
                        <div key={n.id} role="option" className={`item folder-card ${selected.has(n.id) ? 'selected' : ''} ${dragOver === n.id ? 'drop' : ''}`} {...itemProps(n, i)}>
                          <FileIcon name={n.name} kind="dir" size={24} />
                          <span className="item-name" title={n.name}>{n.name}</span>
                          <button className="item-more" tabIndex={-1} aria-label="更多" onClick={(e) => (e.stopPropagation(), itemProps(n, i).onContextMenu(e))}>
                            ⋮
                          </button>
                        </div>
                      );
                    })}
                  </div>
                </>
              )}
              {files.length > 0 && (
                <>
                  <h3 className="section-title">檔案</h3>
                  <div className="file-grid" role="listbox" aria-label="檔案" aria-multiselectable="true">
                    {files.map((n) => {
                      const i = items.indexOf(n);
                      const img = previewKind(n.name) === 'image' && n.size < 25 * 1024 * 1024;
                      return (
                        <div key={n.id} role="option" className={`item file-card ${selected.has(n.id) ? 'selected' : ''}`} {...itemProps(n, i)}>
                          <div className="thumb">
                            {img ? <img src={contentUrl(n.id, true)} alt="" loading="lazy" draggable={false} /> : <FileIcon name={n.name} kind="file" size={56} />}
                          </div>
                          <div className="file-meta">
                            <FileIcon name={n.name} kind="file" size={16} />
                            <span className="item-name" title={n.name}>{n.name}</span>
                            {n.sensitive && <Icon name="lock" size={12} className="sens" />}
                          </div>
                          <div className="file-sub muted small">
                            {formatSize(n.size)} · {formatDate(n.modified_at)}
                          </div>
                        </div>
                      );
                    })}
                  </div>
                </>
              )}
            </>
          ) : (
            <table className="files drive-list" role="grid" aria-multiselectable="true">
              <thead>
                <tr>
                  <th className="col-name">名稱</th>
                  <th className="col-modified">修改時間</th>
                  <th className="col-size">大小</th>
                  <th className="col-type">類型</th>
                </tr>
              </thead>
              <tbody>
                {items.map((n, i) => (
                  <tr key={n.id} className={`item ${selected.has(n.id) ? 'selected' : ''} ${dragOver === n.id ? 'drop' : ''}`} {...itemProps(n, i)}>
                    <td className="col-name">
                      <span className="name-cell">
                        <FileIcon name={n.name} kind={n.kind} size={20} />
                        <span className="fname">{n.name}</span>
                        {n.sensitive && <span className="badge warn">敏感</span>}
                      </span>
                    </td>
                    <td className="col-modified">{formatDate(n.modified_at)}</td>
                    <td className="col-size">{n.kind === 'dir' ? '—' : formatSize(n.size)}</td>
                    <td className="col-type">{typeLabel(n)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
        {dragOver === m.dirId && <div className="drop-hint">放開以上傳到「{m.listing?.node.path ?? '/'}」</div>}
      </section>

      <UploadPanel u={m.uploads} />
      {activeTab && (
        <Overlay
          m={m}
          files={files}
          onIde={() => setLayout('ide')}
          tab={activeTab}
        />
      )}
      {menu && <ContextMenu menu={menu} onClose={() => setMenu(null)} />}
      {newMenu && <ContextMenu menu={newMenu} onClose={() => setNewMenu(null)} />}
    </div>
  );
}

function SideNode({
  m,
  tree,
  node,
  depth,
  onDrop,
  dragOver,
  setDragOver,
  onPick,
}: {
  m: DriveModel;
  tree: ReturnType<typeof useTree>;
  node: { id: string; name: string };
  depth: number;
  onDrop: (e: React.DragEvent, dest: string) => void;
  dragOver: string | null;
  setDragOver: (s: string | null) => void;
  onPick: () => void;
}) {
  const open = tree.expanded.has(node.id);
  const list = tree.kids[node.id];
  const dirs = Array.isArray(list) ? list.filter((n) => n.kind === 'dir') : [];
  const leaf = Array.isArray(list) && dirs.length === 0;
  return (
    <div role="treeitem" aria-expanded={leaf ? undefined : open} aria-selected={m.dirId === node.id}>
      <div
        className={`side-row ${m.dirId === node.id ? 'current' : ''} ${dragOver === 'side:' + node.id ? 'drop' : ''}`}
        style={{ paddingLeft: 10 + depth * 14 }}
        onClick={() => (m.navigate(node.id), onPick())}
        onDragOver={(e) => (e.preventDefault(), e.stopPropagation(), setDragOver('side:' + node.id))}
        onDragLeave={() => setDragOver(null)}
        onDrop={(e) => onDrop(e, node.id)}
        tabIndex={0}
        onKeyDown={(e) => {
          if (e.key === 'Enter') m.navigate(node.id);
          if (e.key === 'ArrowRight') tree.toggle(node.id, true);
          if (e.key === 'ArrowLeft') tree.toggle(node.id, false);
        }}
      >
        <span
          className="twisty"
          onClick={(e) => {
            e.stopPropagation();
            tree.toggle(node.id);
          }}
        >
          {!leaf && <Icon name="chevron" size={13} className={open ? 'rot90' : ''} />}
        </span>
        <FileIcon name={node.name} kind="dir" open={open && !leaf} size={18} />
        <span className="tree-name">{node.name}</span>
      </div>
      {open &&
        dirs.map((d) => (
          <SideNode key={d.id} m={m} tree={tree} node={d} depth={depth + 1} onDrop={onDrop} dragOver={dragOver} setDragOver={setDragOver} onPick={onPick} />
        ))}
    </div>
  );
}

/** Full-screen preview with ←/→ to step through files in the folder. */
function Overlay({ m, files, tab, onIde }: { m: DriveModel; files: NodeInfo[]; tab: import('../state/useDrive').Tab; onIde: () => void }) {
  const hooks = useTabHooks(m, tab, false);
  const n = tab.node;
  const idx = files.findIndex((f) => f.id === n.id);
  const step = useCallback(
    (d: number) => {
      if (idx < 0 || tab.dirty) return;
      const next = files[(idx + d + files.length) % files.length];
      if (next && next.id !== n.id) {
        m.closeTab(n.id);
        m.openFile(next);
      }
    },
    [idx, files, n.id, m, tab.dirty],
  );
  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      const typing = (e.target as HTMLElement).closest('input, textarea, select, .cm-editor');
      if (typing) return;
      if (e.key === 'Escape') m.closeTab(n.id);
      if (e.key === 'ArrowRight') step(1);
      if (e.key === 'ArrowLeft') step(-1);
    };
    window.addEventListener('keydown', k);
    return () => window.removeEventListener('keydown', k);
  }, [m, n.id, step]);
  return (
    <div className="overlay" role="dialog" aria-modal="true" aria-label={`預覽 ${n.name}`} data-region="overlay">
      <header className="overlay-head">
        <button className="btn ghost icon-only" onClick={() => m.closeTab(n.id)} aria-label="關閉預覽" title="關閉（Esc）">
          <Icon name="back" />
        </button>
        <FileIcon name={n.name} kind="file" size={22} />
        <div className="overlay-title">
          <strong>{n.name}</strong>
          <span className="muted small">
            {n.path} · {formatSize(n.size)} · {formatDate(n.modified_at)}
            {tab.dirty && <span className="warn"> · ● 未儲存</span>}
          </span>
        </div>
        <span className="spacer" />
        {previewKind(n.name) === 'image' && (
          <button className="btn ghost" onClick={() => setAsBackground(m, n)} title="設為背景圖片">
            <Icon name="image" /> <span className="hide-sm">設為背景</span>
          </button>
        )}
        <button className="btn ghost" onClick={onIde} title="以 IDE 版面開啟">
          <Icon name="code" /> <span className="hide-sm">在 IDE 開啟</span>
        </button>
        <a className="btn ghost" href={contentUrl(n.id)} download title="下載">
          <Icon name="download" /> <span className="hide-sm">下載</span>
        </a>
      </header>
      <div className="overlay-body">
        {files.length > 1 && idx >= 0 && (
          <>
            <button className="overlay-nav prev" onClick={() => step(-1)} aria-label="上一個檔案" title="上一個（←）">
              <Icon name="back" size={22} />
            </button>
            <button className="overlay-nav next" onClick={() => step(1)} aria-label="下一個檔案" title="下一個（→）">
              <Icon name="chevron" size={22} />
            </button>
          </>
        )}
        <div className="overlay-content">
          <Suspense fallback={<div className="state"><span className="spinner" /> 載入預覽…</div>}>
            <Preview key={n.id} node={n} onOpenPath={m.actions.openPath} editor={hooks} bare />
          </Suspense>
        </div>
      </div>
    </div>
  );
}
