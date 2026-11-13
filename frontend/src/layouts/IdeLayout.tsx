// IDE layout: explorer tree (folders *and* files) on the left with actions and
// uploads docked at its bottom; the whole right side is tabs + editor/preview.
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { NodeInfo } from '../api';
import { ContextMenu, type MenuState } from '../components/ContextMenu';
import { FileIcon } from '../components/FileIcon';
import { Icon } from '../components/Icon';
import { MOD } from '../components/Shell';
import { UploadPanel } from '../components/Uploads';
import { languageFor } from '../fileTypes';
import type { DriveModel } from '../state/useDrive';
import { DND_TYPE, dropMove, emptyMenu, nodeMenu, ragLabel, useRagStatus, useTabHooks, useTree } from './shared';

const Preview = lazy(() => import('../preview/Preview'));

interface Row {
  node: NodeInfo;
  depth: number;
}

export default function IdeLayout({ m, sideOpen, setSideOpen, targetRef }: {
  m: DriveModel;
  sideOpen: boolean;
  setSideOpen: (o: boolean) => void;
  targetRef: React.MutableRefObject<() => string>;
}) {
  const tree = useTree(m.version);
  const [selected, setSelected] = useState<string | null>(null);
  const [focus, setFocus] = useState<string | null>(null);
  const [menu, setMenu] = useState<MenuState | null>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const [width, setWidth] = useState(Number(localStorage.getItem('mistbox.ideSideW')) || 280);
  const box = useRef<HTMLDivElement>(null);
  const rag = useRagStatus();

  const rows = useMemo(() => {
    const out: Row[] = [];
    const walk = (id: string, depth: number) => {
      const list = tree.kids[id];
      if (!Array.isArray(list)) return;
      for (const n of list) {
        out.push({ node: n, depth });
        if (n.kind === 'dir' && tree.expanded.has(n.id)) walk(n.id, depth + 1);
      }
    };
    walk('root', 0);
    return out;
  }, [tree.kids, tree.expanded]);

  const byId = useMemo(() => new Map(rows.map((r) => [r.node.id, r.node])), [rows]);
  const selNode = selected ? byId.get(selected) : undefined;
  const target = useCallback(() => {
    if (!selNode) return 'root';
    return selNode.kind === 'dir' ? selNode.id : (selNode.parent_id ?? 'root');
  }, [selNode]);
  useEffect(() => {
    targetRef.current = target;
  }, [target, targetRef]);

  // Reveal the active tab in the tree.
  useEffect(() => {
    if (m.active) setSelected(m.active);
  }, [m.active]);

  useEffect(() => localStorage.setItem('mistbox.ideSideW', String(width)), [width]);

  const activate = (n: NodeInfo) => {
    setSelected(n.id);
    if (n.kind === 'dir') tree.toggle(n.id);
    else {
      m.openFile(n);
      setSideOpen(false);
    }
  };

  const onKey = (e: React.KeyboardEvent) => {
    const i = rows.findIndex((r) => r.node.id === (focus ?? selected));
    const r = rows[i];
    const move = (j: number) => {
      const t = rows[Math.max(0, Math.min(rows.length - 1, j))];
      if (!t) return;
      setFocus(t.node.id);
      setSelected(t.node.id);
      box.current?.querySelector<HTMLElement>(`[data-id="${t.node.id}"]`)?.focus();
    };
    switch (e.key) {
      case 'ArrowDown':
        move(i + 1);
        break;
      case 'ArrowUp':
        move(i - 1);
        break;
      case 'ArrowRight':
        if (r?.node.kind === 'dir') tree.toggle(r.node.id, true);
        break;
      case 'ArrowLeft':
        if (r?.node.kind === 'dir' && tree.expanded.has(r.node.id)) tree.toggle(r.node.id, false);
        else if (r?.node.parent_id) move(rows.findIndex((x) => x.node.id === r.node.parent_id));
        break;
      case 'Enter':
        if (r) activate(r.node);
        break;
      case 'F2':
        if (r) m.actions.rename(r.node);
        break;
      case 'Delete':
        if (r) m.actions.remove([r.node]);
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  const onDrop = async (e: React.DragEvent, dest: string) => {
    e.preventDefault();
    e.stopPropagation();
    setDropTarget(null);
    const internal = e.dataTransfer.getData(DND_TYPE);
    if (internal) return dropMove(m, [internal], dest);
    m.actions.uploadDrop(dest, e.dataTransfer);
    tree.toggle(dest, true);
  };

  const drag = (e: React.PointerEvent) => {
    e.preventDefault();
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const mv = (ev: PointerEvent) => setWidth(Math.max(180, Math.min(640, ev.clientX)));
    const up = () => (el.removeEventListener('pointermove', mv), el.removeEventListener('pointerup', up));
    el.addEventListener('pointermove', mv);
    el.addEventListener('pointerup', up);
  };

  const activeTab = m.tabs.find((t) => t.node.id === m.active);
  const hooks = useTabHooks(m, activeTab, true);
  const rootList = tree.kids.root;

  return (
    <div className="ide" style={{ '--ide-side-w': `${width}px` } as React.CSSProperties} data-region="ide">
      <aside className={`explorer-pane ${sideOpen ? 'open' : ''}`} data-region="explorer">
        <div className="pane-title">
          <span>檔案總管</span>
          <span className="spacer" />
          <button className="btn ghost icon-only xs" onClick={() => m.setDialog({ t: 'trash' })} title="垃圾桶" aria-label="垃圾桶">
            <Icon name="trash" size={14} />
          </button>
          <button className="btn ghost icon-only xs" onClick={tree.collapseAll} title="全部收合" aria-label="全部收合">
            <Icon name="collapse" size={14} />
          </button>
        </div>
        <div
          className={`tree-scroll ${dropTarget === 'root' ? 'drop' : ''}`}
          ref={box}
          role="tree"
          aria-label="檔案與資料夾"
          tabIndex={rows.length ? -1 : 0}
          onKeyDown={onKey}
          onDoubleClick={(e) => e.target === e.currentTarget && m.actions.newFile(target())}
          onContextMenu={(e) => {
            if (e.target !== e.currentTarget) return;
            e.preventDefault();
            setMenu({ x: e.clientX, y: e.clientY, items: emptyMenu(m, 'root', [{ label: '全部收合', icon: 'collapse', action: tree.collapseAll }]) });
          }}
          onDragOver={(e) => {
            if (e.target === e.currentTarget) (e.preventDefault(), setDropTarget('root'));
          }}
          onDragLeave={() => setDropTarget(null)}
          onDrop={(e) => onDrop(e, 'root')}
        >
          {rootList === 'loading' && <div className="state small"><span className="spinner tiny" /> 載入中…</div>}
          {rootList === 'error' && (
            <div className="state error small">
              無法載入 <button className="btn sm" onClick={m.bump}>重試</button>
            </div>
          )}
          {Array.isArray(rootList) && rootList.length === 0 && (
            <div className="tree-empty" onDoubleClick={() => m.actions.newFile('root')}>
              <p>雲端是空的</p>
              <p className="muted small">雙擊這裡新增檔案，或把檔案拖進來上傳。</p>
            </div>
          )}
          {rows.map(({ node: n, depth }) => {
            const open = n.kind === 'dir' && tree.expanded.has(n.id);
            const list = tree.kids[n.id];
            const dirty = m.tabs.some((t) => t.node.id === n.id && t.dirty);
            return (
              <div
                key={n.id}
                data-id={n.id}
                role="treeitem"
                aria-level={depth + 1}
                aria-expanded={n.kind === 'dir' ? open : undefined}
                aria-selected={selected === n.id}
                tabIndex={(focus ?? selected ?? rows[0]?.node.id) === n.id ? 0 : -1}
                className={`tree-row ${selected === n.id ? 'selected' : ''} ${m.active === n.id ? 'current' : ''} ${dropTarget === n.id ? 'drop' : ''} kind-${n.kind}`}
                style={{ paddingLeft: 8 + depth * 14 }}
                draggable
                onDragStart={(e) => e.dataTransfer.setData(DND_TYPE, n.id)}
                onDragOver={(e) => {
                  if (n.kind !== 'dir') return;
                  e.preventDefault();
                  e.stopPropagation();
                  setDropTarget(n.id);
                }}
                onDrop={(e) => n.kind === 'dir' && onDrop(e, n.id)}
                onClick={() => activate(n)}
                onFocus={() => setFocus(n.id)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  setSelected(n.id);
                  setMenu({ x: e.clientX, y: e.clientY, items: nodeMenu(m, n, [n], () => activate(n)) });
                }}
                title={n.name}
              >
                <span className="twisty">{n.kind === 'dir' && <Icon name="chevron" size={13} className={open ? 'rot90' : ''} />}</span>
                <FileIcon name={n.name} kind={n.kind} open={open} size={17} />
                <span className="tree-name">{n.name}</span>
                {n.sensitive && <Icon name="lock" size={12} className="sens" />}
                {dirty && <span className="dirty-dot" title="未儲存">●</span>}
                {open && list === 'loading' && <span className="spinner tiny" />}
              </div>
            );
          })}
          {Array.isArray(rootList) && rootList.length > 0 && (
            <div
              className="tree-tail"
              title="雙擊新增檔案，右鍵更多操作"
              onDoubleClick={() => m.actions.newFile(target())}
              onClick={() => setSelected(null)}
              onContextMenu={(e) => {
                e.preventDefault();
                setMenu({ x: e.clientX, y: e.clientY, items: emptyMenu(m, target(), [{ label: '全部收合', icon: 'collapse', action: tree.collapseAll }]) });
              }}
              onDragOver={(e) => (e.preventDefault(), setDropTarget('root'))}
              onDrop={(e) => onDrop(e, 'root')}
            />
          )}
        </div>
        <UploadPanel u={m.uploads} embedded />
        <div className="explorer-actions" role="toolbar" aria-label="檔案操作" data-region="explorer-actions">
          <button className="btn ghost sm" onClick={() => m.actions.newFile(target())} title="新增檔案（或雙擊空白處）">
            <Icon name="filePlus" /> <span>檔案</span>
          </button>
          <button className="btn ghost sm" onClick={() => m.actions.newFolder(target())} title="新增資料夾">
            <Icon name="folderPlus" /> <span>資料夾</span>
          </button>
          <button className="btn ghost sm" onClick={() => m.actions.upload(target())} title="上傳檔案（也可拖放）">
            <Icon name="upload" /> <span>上傳</span>
          </button>
          <button className="btn ghost icon-only sm" onClick={() => m.actions.uploadFolder(target())} title="上傳資料夾" aria-label="上傳資料夾">
            <Icon name="folder" />
          </button>
          <button className="btn ghost icon-only sm" onClick={m.bump} title="重新整理" aria-label="重新整理">
            <Icon name="refresh" />
          </button>
        </div>
      </aside>
      {sideOpen && <div className="scrim mobile-only" onClick={() => setSideOpen(false)} />}
      <div className="resizer" role="separator" aria-orientation="vertical" aria-label="調整檔案總管寬度" onPointerDown={drag} />

      <section className="editor-pane" data-region="editor">
        {m.tabs.length > 0 && (
          <div className="tabs" role="tablist" aria-label="開啟的檔案">
            {m.tabs.map((t) => (
              <div key={t.node.id} className={`tab ${t.node.id === m.active ? 'on' : ''}`} role="presentation" onMouseDown={(e) => e.button === 1 && (e.preventDefault(), m.closeTab(t.node.id))}>
                <button role="tab" aria-selected={t.node.id === m.active} onClick={() => m.setActive(t.node.id)} title={t.node.path ?? t.node.name}>
                  <FileIcon name={t.node.name} kind="file" size={15} />
                  {t.node.name}
                </button>
                <button className="tab-x" aria-label={`關閉 ${t.node.name}`} onClick={() => m.closeTab(t.node.id)}>
                  {t.dirty ? <span className="dirty-dot">●</span> : <Icon name="close" size={12} />}
                </button>
              </div>
            ))}
          </div>
        )}
        {activeTab ? (
          <Suspense fallback={<div className="state"><span className="spinner" /> 載入編輯器…</div>}>
            <Preview key={activeTab.node.id + ':' + (activeTab.rev ?? 0)} node={activeTab.node} onOpenPath={m.actions.openPath} editor={hooks} />
          </Suspense>
        ) : (
          <Welcome m={m} target={target} />
        )}
      </section>

      <footer className="statusbar" data-region="statusbar">
        <span className="sb-item" title="目前檔案">
          {activeTab ? (activeTab.node.path ?? activeTab.node.name) : selNode?.path ?? '/'}
        </span>
        {activeTab?.dirty && <span className="sb-item warn">● 未儲存（{MOD}+S）</span>}
        <span className="spacer" />
        {activeTab && <span className="sb-item">{languageFor(activeTab.node.name)}</span>}
        <button className="sb-item sb-btn" onClick={() => m.setDialog({ t: 'settings' })} title="語意搜尋狀態">
          <Icon name="sparkle" size={12} /> {ragLabel(rag)}
        </button>
        <button className="sb-item sb-btn" onClick={() => m.setPaletteMode('all')}>
          <Icon name="command" size={12} /> {MOD}+Shift+P
        </button>
      </footer>
      {menu && <ContextMenu menu={menu} onClose={() => setMenu(null)} />}
    </div>
  );
}

function Welcome({ m, target }: { m: DriveModel; target: () => string }) {
  return (
    <div className="welcome" onDoubleClick={() => m.actions.newFile(target())}>
      <h2>Mistbox</h2>
      <p className="muted">在左側選擇檔案開啟；文字與程式碼可直接編輯並以 {MOD}+S 儲存。</p>
      <div className="welcome-grid">
        <button className="welcome-act" onClick={() => m.actions.newFile(target())}>
          <Icon name="filePlus" /> 新增檔案 <kbd>雙擊空白處</kbd>
        </button>
        <button className="welcome-act" onClick={() => m.actions.upload(target())}>
          <Icon name="upload" /> 上傳檔案 <kbd>拖放</kbd>
        </button>
        <button className="welcome-act" onClick={() => m.setPaletteMode('all')}>
          <Icon name="command" /> 指令面板 <kbd>{MOD}+Shift+P</kbd>
        </button>
        <button className="welcome-act" onClick={() => m.setPaletteMode('files')}>
          <Icon name="search" /> 依檔名開啟 <kbd>{MOD}+P</kbd>
        </button>
        <button className="welcome-act" onClick={() => m.setPaletteMode('semantic')}>
          <Icon name="sparkle" /> 語意搜尋 <kbd>{MOD}+Shift+F</kbd>
        </button>
        <button className="welcome-act" onClick={() => m.setDialog({ t: 'settings' })}>
          <Icon name="gear" /> 主題、背景與自訂 CSS
        </button>
      </div>
    </div>
  );
}
