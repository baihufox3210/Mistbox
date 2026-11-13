// IDE editor area split into groups (like VS Code): each group has its own
// tabs; groups sit side by side or stacked and can be resized. Tabs move by
// dragging them onto another group (centre = join, edges = split there);
// files dragged from the explorer open the same way.
//
// Groups are rendered as a flat, absolutely positioned list keyed by group id,
// so splitting or merging never remounts an open editor.
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api, type NodeInfo } from '../api';
import { FileIcon } from '../components/FileIcon';
import { Icon } from '../components/Icon';
import { MOD } from '../components/Shell';
import type { DriveModel } from '../state/useDrive';
import {
  focusGroup,
  groupOf,
  groupOrder,
  initialGrid,
  joinAll,
  moveTab,
  reconcile,
  removeGroup,
  setSizes,
  split,
  type Edge,
  type GNode,
  type Grid,
} from '../state/editorGrid';
import { useCaps } from '../caps';
import { DND_TYPE, useTabHooks } from './shared';

const DiagnosePanel = lazy(() => import('../components/DiagnosePanel'));
const Preview = lazy(() => import('../preview/Preview'));

export const TAB_DND = 'application/x-mistbox-tab';
/** Palette / shortcut actions, sent as `mistbox:editor` events. */
export type GridAction = 'split-right' | 'split-down' | 'join-all' | 'focus-next' | 'close-group';

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}
interface Sash {
  split: string;
  i: number;
  dir: 'row' | 'col';
  at: number;
  r: Rect;
}

function computeLayout(root: GNode) {
  const groups: (Rect & { id: string })[] = [];
  const sashes: Sash[] = [];
  const walk = (n: GNode, r: Rect) => {
    if (n.t === 'g') return void groups.push({ id: n.id, ...r });
    let off = 0;
    n.kids.forEach((k, i) => {
      const s = n.sizes[i];
      const kr = n.dir === 'row' ? { x: r.x + off * r.w, y: r.y, w: s * r.w, h: r.h } : { x: r.x, y: r.y + off * r.h, w: r.w, h: s * r.h };
      if (i > 0) sashes.push({ split: n.id, i, dir: n.dir, at: n.dir === 'row' ? kr.x : kr.y, r });
      walk(k, kr);
      off += s;
    });
  };
  walk(root, { x: 0, y: 0, w: 1, h: 1 });
  return { groups, sashes };
}

function findSplit(n: GNode, id: string): Extract<GNode, { t: 's' }> | null {
  if (n.t === 'g') return null;
  if (n.id === id) return n;
  for (const k of n.kids) {
    const f = findSplit(k, id);
    if (f) return f;
  }
  return null;
}

const pct = (v: number) => `${(v * 100).toFixed(4)}%`;

/** Which part of a group the pointer is over: an edge (split) or the centre. */
function dropZone(e: React.DragEvent, el: HTMLElement): Edge | 'center' {
  const r = el.getBoundingClientRect();
  const fx = (e.clientX - r.left) / r.width;
  const fy = (e.clientY - r.top) / r.height;
  const d = Math.min(fx, 1 - fx, fy, 1 - fy);
  if (d > 0.22) return 'center';
  if (d === fx) return 'left';
  if (d === 1 - fx) return 'right';
  return d === fy ? 'top' : 'bottom';
}

/** Last layout, so switching to the Drive layout and back keeps the groups (memory only). */
let lastGrid: Grid | null = null;

export default function EditorGrid({
  m,
  nodeFor,
  empty,
}: {
  m: DriveModel;
  /** Explorer lookup for files dragged in from the tree. */
  nodeFor: (id: string) => NodeInfo | undefined;
  /** Shown when nothing is open at all. */
  empty: React.ReactNode;
}) {
  const [grid, setGrid] = useState<Grid>(() => lastGrid ?? initialGrid());
  const gridRef = useRef(grid);
  gridRef.current = grid;
  lastGrid = grid;
  const prevActive = useRef<string | null>(null);
  const place = useRef<{ group: string; edge?: Edge; tab?: string } | null>(null);
  const box = useRef<HTMLDivElement>(null);
  const [drop, setDrop] = useState<{ group: string; zone: Edge | 'center' } | null>(null);

  // `m` is rebuilt on every render; callbacks read the latest through a ref.
  const mRef = useRef(m);
  mRef.current = m;

  const openKey = m.tabs.map((t) => t.node.id).join('\n');
  useEffect(() => {
    const open = openKey ? openKey.split('\n') : [];
    const r = reconcile(gridRef.current, open, m.active, prevActive.current, place.current);
    place.current = null;
    prevActive.current = r.active;
    gridRef.current = r.grid;
    setGrid(r.grid);
    if (r.active !== m.active) mRef.current.setActive(r.active);
  }, [openKey, m.active]);

  const apply = useCallback((g: Grid, active?: string | null) => {
    gridRef.current = g;
    setGrid(g);
    if (active !== undefined) {
      prevActive.current = active;
      mRef.current.setActive(active);
    }
  }, []);

  // A drag that ends anywhere clears the drop highlight.
  useEffect(() => {
    const clear = () => setDrop(null);
    window.addEventListener('dragend', clear);
    window.addEventListener('drop', clear);
    return () => (window.removeEventListener('dragend', clear), window.removeEventListener('drop', clear));
  }, []);

  const run = useCallback(
    (a: GridAction) => {
      const g = gridRef.current;
      const gid = g.focused;
      const gr = g.groups[gid];
      if (a === 'split-right' || a === 'split-down') {
        const edge = a === 'split-right' ? 'right' : 'bottom';
        // Move the active tab into the new group when the group has others;
        // a group's only tab stays put and the new group starts empty.
        const tab = gr.active && gr.tabs.length > 1 ? gr.active : undefined;
        const r = split(g, gid, edge, tab);
        apply(r.grid, tab ?? null);
      } else if (a === 'join-all') {
        const j = joinAll(g);
        apply(j, j.groups[j.focused].active);
      } else if (a === 'focus-next') {
        const order = groupOrder(g.root);
        const next = order[(order.indexOf(gid) + 1) % order.length];
        apply(focusGroup(g, next), g.groups[next].active);
      } else if (a === 'close-group') {
        if (groupOrder(g.root).length <= 1 && gr.tabs.length === 0) return;
        // Closing its tabs removes the group; an empty one is dropped directly.
        if (gr.tabs.length) mRef.current.closeTabs(gr.tabs);
        else {
          const j = removeGroup(g, gid);
          apply(j, j.groups[j.focused]?.active ?? null);
        }
      }
    },
    [apply],
  );

  // Shortcuts: Ctrl+\ split right, Ctrl+Alt+\ split down, Ctrl+1…9 focus group.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      if (e.key === '\\') {
        e.preventDefault();
        run(e.altKey ? 'split-down' : 'split-right');
      } else if (/^[1-9]$/.test(e.key) && !e.shiftKey && !e.altKey) {
        const g = gridRef.current;
        const id = groupOrder(g.root)[Number(e.key) - 1];
        if (!id) return;
        e.preventDefault();
        apply(focusGroup(g, id), g.groups[id].active);
      }
    };
    const onCmd = (e: Event) => run((e as CustomEvent<GridAction>).detail);
    // Open two places side by side (duplicate-code comparison).
    const onCompare = async (e: Event) => {
      const { a, b } = (e as CustomEvent<{ a: { id: string; line_start: number }; b: { id: string; line_start: number } }>).detail;
      try {
        const [na, nb] = await Promise.all([api.info(a.id), api.info(b.id)]);
        const g = gridRef.current;
        const ga = groupOf(g, na.id) ?? g.focused;
        if (na.id !== nb.id) {
          if (groupOf(g, nb.id)) apply(split(g, ga, 'right', nb.id).grid);
          else place.current = { group: ga, edge: 'right', tab: nb.id };
        }
        mRef.current.openFile(na, a.line_start);
        if (na.id !== nb.id) mRef.current.openFile(nb, b.line_start);
      } catch {
        /* file gone */
      }
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('mistbox:editor', onCmd);
    window.addEventListener('mistbox:compare', onCompare);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('mistbox:editor', onCmd);
      window.removeEventListener('mistbox:compare', onCompare);
    };
  }, [run, apply]);

  const onDrop = async (e: React.DragEvent, gid: string, zone: Edge | 'center', index?: number) => {
    e.preventDefault();
    e.stopPropagation();
    setDrop(null);
    const g = gridRef.current;
    const tab = e.dataTransfer.getData(TAB_DND) || e.dataTransfer.getData(DND_TYPE);
    if (!tab) return;
    if (groupOf(g, tab)) {
      // Unsaved text survives the move (state/drafts.ts).
      if (zone === 'center') apply(moveTab(g, tab, gid, index), tab);
      else apply(split(g, gid, zone, tab).grid, tab);
      return;
    }
    // A file from the explorer: open it at the drop target.
    const n = nodeFor(tab) ?? (await api.info(tab).catch(() => undefined));
    if (!n || n.kind !== 'file') return;
    place.current = { group: gid, edge: zone === 'center' ? undefined : zone };
    mRef.current.openFile(n);
  };

  const layout = useMemo(() => computeLayout(grid.root), [grid.root]);
  const multi = layout.groups.length > 1;

  const sashDrag = (e: React.PointerEvent, s: Sash) => {
    e.preventDefault();
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const rect = box.current!.getBoundingClientRect();
    const mv = (ev: PointerEvent) => {
      const g = gridRef.current;
      const sp = findSplit(g.root, s.split);
      if (!sp) return;
      const f = s.dir === 'row' ? ((ev.clientX - rect.left) / rect.width - s.r.x) / s.r.w : ((ev.clientY - rect.top) / rect.height - s.r.y) / s.r.h;
      const before = sp.sizes.slice(0, s.i - 1).reduce((a, b) => a + b, 0);
      const pair = sp.sizes[s.i - 1] + sp.sizes[s.i];
      // Keep groups usable: at least 240 px wide / 120 px tall.
      const parentPx = s.dir === 'row' ? s.r.w * rect.width : s.r.h * rect.height;
      const min = Math.min((s.dir === 'row' ? 240 : 120) / parentPx, pair / 2);
      const a = Math.max(min, Math.min(pair - min, f - before));
      const sizes = [...sp.sizes];
      sizes[s.i - 1] = a;
      sizes[s.i] = pair - a;
      apply(setSizes(g, s.split, sizes));
    };
    const up = () => (el.removeEventListener('pointermove', mv), el.removeEventListener('pointerup', up));
    el.addEventListener('pointermove', mv);
    el.addEventListener('pointerup', up);
  };

  if (m.tabs.length === 0 && !multi) return <>{empty}</>;

  return (
    <div className={`egrid ${multi ? 'multi' : ''}`} ref={box} data-region="editor-grid">
      {layout.groups.map((r) => (
        <EditorGroup
          key={r.id}
          m={m}
          gid={r.id}
          rect={r}
          grid={grid}
          multi={multi}
          drop={drop?.group === r.id ? drop.zone : null}
          setDrop={setDrop}
          onDrop={onDrop}
          onFocus={() => {
            if (gridRef.current.focused !== r.id) apply(focusGroup(gridRef.current, r.id), gridRef.current.groups[r.id].active);
          }}
          onAction={run}
        />
      ))}
      {layout.sashes.map((s) => (
        <div
          key={`${s.split}:${s.i}`}
          className={`eg-sash ${s.dir}`}
          role="separator"
          aria-orientation={s.dir === 'row' ? 'vertical' : 'horizontal'}
          aria-label="調整編輯器群組大小"
          style={
            s.dir === 'row'
              ? { left: pct(s.at), top: pct(s.r.y), height: pct(s.r.h) }
              : { top: pct(s.at), left: pct(s.r.x), width: pct(s.r.w) }
          }
          onPointerDown={(e) => sashDrag(e, s)}
        />
      ))}
    </div>
  );
}

function EditorGroup({
  m,
  gid,
  rect,
  grid,
  multi,
  drop,
  setDrop,
  onDrop,
  onFocus,
  onAction,
}: {
  m: DriveModel;
  gid: string;
  rect: Rect;
  grid: Grid;
  multi: boolean;
  drop: Edge | 'center' | null;
  setDrop: (d: { group: string; zone: Edge | 'center' } | null) => void;
  onDrop: (e: React.DragEvent, gid: string, zone: Edge | 'center', index?: number) => void;
  onFocus: () => void;
  onAction: (a: GridAction) => void;
}) {
  const gr = grid.groups[gid];
  const focused = grid.focused === gid;
  const tabs = gr.tabs.map((id) => m.tabs.find((t) => t.node.id === id)).filter((t) => !!t);
  const activeTab = tabs.find((t) => t.node.id === gr.active);
  const caps = useCaps();
  const hooks = useTabHooks(m, activeTab?.kind === 'diagnose' ? undefined : activeTab, true, caps.admin);
  const body = useRef<HTMLDivElement>(null);
  const accepts = (e: React.DragEvent) => e.dataTransfer.types.includes(TAB_DND) || e.dataTransfer.types.includes(DND_TYPE);

  return (
    <section
      className={`egroup ${focused ? 'focused' : ''}`}
      style={{ left: pct(rect.x), top: pct(rect.y), width: pct(rect.w), height: pct(rect.h) }}
      onPointerDownCapture={onFocus}
      onFocusCapture={onFocus}
      aria-label={multi ? `編輯器群組 ${groupOrder(grid.root).indexOf(gid) + 1}` : undefined}
      data-group={gid}
    >
      <div
        className="tabs"
        role="tablist"
        aria-label="開啟的檔案"
        onDragOver={(e) => {
          if (!accepts(e)) return;
          e.preventDefault();
          setDrop({ group: gid, zone: 'center' });
        }}
        onDrop={(e) => onDrop(e, gid, 'center')}
      >
        {tabs.map((t, i) => (
          <div
            key={t.node.id}
            className={`tab ${t.node.id === gr.active ? 'on' : ''}`}
            role="presentation"
            draggable
            onDragStart={(e) => {
              e.dataTransfer.setData(TAB_DND, t.node.id);
              e.dataTransfer.effectAllowed = 'move';
            }}
            onDragOver={(e) => {
              if (!accepts(e)) return;
              e.preventDefault();
              e.stopPropagation();
              setDrop({ group: gid, zone: 'center' });
            }}
            onDrop={(e) => onDrop(e, gid, 'center', i)}
            onMouseDown={(e) => e.button === 1 && (e.preventDefault(), m.closeTab(t.node.id))}
          >
            <button role="tab" aria-selected={t.node.id === gr.active} onClick={() => m.setActive(t.node.id)} title={t.node.path ?? t.node.name}>
              <>{t.kind === 'diagnose' ? <Icon name="diagnose" size={15} /> : <FileIcon name={t.node.name} kind="file" size={15} />}</>
              {t.node.name}
            </button>
            <button className="tab-x" aria-label={`關閉 ${t.node.name}`} title={`關閉檔案（${MOD}+W / Alt+W）`} onClick={() => m.closeTab(t.node.id)}>
              {t.dirty ? <span className="dirty-dot">●</span> : <Icon name="close" size={12} />}
            </button>
          </div>
        ))}
        <span className="tabs-fill" />
        <div className="group-actions">
          <button className="btn ghost icon-only xs" onClick={() => onAction('split-right')} title={`向右分割（${MOD}+\\）`} aria-label="向右分割">
            <Icon name="splitRight" size={15} />
          </button>
          <button className="btn ghost icon-only xs" onClick={() => onAction('split-down')} title={`向下分割（${MOD}+Alt+\\）`} aria-label="向下分割">
            <Icon name="splitDown" size={15} />
          </button>
          {multi && (
            <button className="btn ghost icon-only xs" onClick={() => onAction('close-group')} title="關閉這個群組（含其中的分頁）" aria-label="關閉群組">
              <Icon name="close" size={14} />
            </button>
          )}
        </div>
      </div>
      <div
        className="egroup-body"
        ref={body}
        onDragOver={(e) => {
          if (!accepts(e)) return;
          e.preventDefault();
          const z = dropZone(e, body.current!);
          if (z !== drop) setDrop({ group: gid, zone: z });
        }}
        onDragLeave={(e) => !e.currentTarget.contains(e.relatedTarget as Node) && setDrop(null)}
        onDrop={(e) => onDrop(e, gid, dropZone(e, body.current!))}
      >
        {activeTab ? (
          <Suspense fallback={<div className="state"><span className="spinner" /> 載入編輯器…</div>}>
            {activeTab.kind === 'diagnose' ? <DiagnosePanel m={m} /> : <Preview key={activeTab.node.id + ':' + (activeTab.rev ?? 0)} node={activeTab.node} onOpenPath={m.actions.openPath} editor={hooks} />}
          </Suspense>
        ) : (
          <div className="egroup-empty">
            <p>空的編輯器群組</p>
            <button className="btn ghost sm" onClick={() => m.setPaletteMode('files')}><Icon name="search" />開啟檔案</button>
          </div>
        )}
        {drop && <div className={`eg-drop ${drop}`} />}
      </div>
    </section>
  );
}
