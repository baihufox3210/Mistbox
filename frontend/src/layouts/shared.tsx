import { useCallback, useEffect, useMemo, useState } from 'react';
import { api, contentUrl, errorText, type NodeInfo, type RagStatus } from '../api';
import type { MenuItem } from '../components/ContextMenu';
import { previewKind } from '../fileTypes';
import { applyBackground } from '../theme';
import type { DriveModel, Tab } from '../state/useDrive';
import type { EditorHooks } from '../preview/text';
import { ADMIN_CAPS, type Caps } from '../caps';

export const DND_TYPE = 'application/x-mistbox-node';

/** Lazily loaded folder tree shared by the IDE explorer and the Drive sidebar. */
export function useTree(version: number, initiallyOpen: string[] = ['root']) {
  const [kids, setKids] = useState<Record<string, NodeInfo[] | 'loading' | 'error'>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set(initiallyOpen));
  const load = useCallback((id: string) => {
    setKids((k) => ({ ...k, [id]: Array.isArray(k[id]) ? k[id] : 'loading' }));
    api.children(id).then(
      (l) => setKids((k) => ({ ...k, [id]: l.children })),
      () => setKids((k) => ({ ...k, [id]: 'error' })),
    );
  }, []);
  const key = [...expanded].join(',');
  useEffect(() => {
    expanded.forEach(load);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [version, key, load]);
  const toggle = useCallback((id: string, open?: boolean) => {
    setExpanded((e) => {
      const n = new Set(e);
      if (open ?? !n.has(id)) n.add(id);
      else n.delete(id);
      return n;
    });
  }, []);
  const expandAll = useCallback((ids: string[]) => setExpanded((e) => new Set([...e, ...ids])), []);
  const collapseAll = useCallback(() => setExpanded(new Set(['root'])), []);
  return { kids, expanded, toggle, expandAll, collapseAll };
}

export function setAsBackground(m: DriveModel, n: NodeInfo) {
  api.saveSettings({ background_url: contentUrl(n.id, true) }).then(
    (s) => (applyBackground(s), m.toast('已設為背景，可在設定調整整體透明度與模糊')),
    (e) => m.toast(errorText(e)),
  );
}

/** Context menu for a node (shared by both layouts). */
export function nodeMenu(m: DriveModel, n: NodeInfo, selection: NodeInfo[], open: () => void, caps: Caps = ADMIN_CAPS): MenuItem[] {
  const many = selection.length > 1 && selection.some((s) => s.id === n.id) ? selection : [n];
  const items: MenuItem[] = [];
  if (n.kind === 'dir') {
    items.push({ label: '開啟', icon: 'folderOpen', action: open });
    if (caps.write)
      items.push(
        { separator: true, label: '' },
        { label: '新增檔案', icon: 'filePlus', action: () => m.actions.newFile(n.id) },
        { label: '新增資料夾', icon: 'folderPlus', action: () => m.actions.newFolder(n.id) },
        { label: '上傳檔案到這裡', icon: 'upload', action: () => m.actions.upload(n.id) },
        { label: '上傳資料夾到這裡', icon: 'folder', action: () => m.actions.uploadFolder(n.id) },
      );
  } else {
    items.push({ label: '開啟', icon: 'external', action: open }, { label: '下載', icon: 'download', action: () => m.actions.download(n) });
    if (caps.admin && previewKind(n.name) === 'image') items.push({ label: '設為背景圖片', icon: 'image', action: () => setAsBackground(m, n) });
  }
  if (caps.admin)
    items.push({ separator: true, label: '' }, { label: '分享連結…', icon: 'link', disabled: many.length > 1, action: () => m.setDialog({ t: 'share', item: n }) });
  if (caps.write)
    items.push(
      { separator: true, label: '' },
      { label: '重新命名', icon: 'edit', shortcut: 'F2', disabled: many.length > 1, action: () => m.actions.rename(n) },
      { label: many.length > 1 ? `移動 ${many.length} 個項目` : '移動到…', icon: 'move', action: () => m.actions.move(many) },
    );
  items.push({ label: '複製路徑', icon: 'copy', action: () => m.actions.copyPath(n) });
  if (caps.write)
    items.push(
      { separator: true, label: '' },
      { label: many.length > 1 ? `刪除 ${many.length} 個項目` : '刪除', icon: 'trash', shortcut: 'Del', danger: true, action: () => m.actions.remove(many) },
    );
  return items;
}

export function emptyMenu(m: DriveModel, target: string, extra: MenuItem[] = [], caps: Caps = ADMIN_CAPS): MenuItem[] {
  const write: MenuItem[] = caps.write
    ? [
        { label: '新增檔案', icon: 'filePlus', shortcut: '雙擊空白處', action: () => m.actions.newFile(target) },
        { label: '新增資料夾', icon: 'folderPlus', action: () => m.actions.newFolder(target) },
        { separator: true, label: '' },
        { label: '上傳檔案', icon: 'upload', action: () => m.actions.upload(target) },
        { label: '上傳資料夾', icon: 'folder', action: () => m.actions.uploadFolder(target) },
        { separator: true, label: '' },
      ]
    : [];
  return [...write, { label: '重新整理', icon: 'refresh', action: m.bump }, ...extra];
}

/** Move dragged node(s) into a folder. */
export async function dropMove(m: DriveModel, ids: string[], dest: string) {
  try {
    for (const id of ids) if (id !== dest) await api.update(id, { parent_id: dest });
  } catch (e) {
    m.toast(errorText(e));
  }
  m.bump();
}

/** Editor hooks for a tab, memoised so editors don't re-register every render. */
export function useTabHooks(m: DriveModel, tab: Tab | undefined, startEditing: boolean, diagnostics = false): EditorHooks | undefined {
  const id = tab?.node.id;
  const line = tab?.line;
  const { setDirty, updateNode, registerSaver, bump } = m;
  return useMemo(
    () =>
      id
        ? {
            startEditing,
            diagnostics,
            line,
            onDirty: (d: boolean) => setDirty(id, d),
            onSaved: (n: NodeInfo) => (updateNode(n), bump()),
            register: (fn: (() => Promise<void>) | null) => registerSaver(id, fn),
          }
        : undefined,
    [id, line, startEditing, diagnostics, setDirty, updateNode, registerSaver, bump],
  );
}

export function useRagStatus(enabled = true) {
  const [s, setS] = useState<RagStatus | null>(null);
  useEffect(() => {
    if (!enabled) return;
    const f = () => api.ragStatus().then(setS, () => {});
    f();
    const t = setInterval(f, 10_000);
    return () => clearInterval(t);
  }, [enabled]);
  return s;
}

export function ragLabel(s: RagStatus | null): string {
  if (!s) return '';
  if (!s.enabled) return '語意搜尋停用';
  if (s.state === 'error') return '語意搜尋：模型錯誤';
  if (s.state === 'loading') return '語意搜尋：載入模型…';
  if ((s.pending ?? 0) > 0) return `語意索引中（剩 ${s.pending}）`;
  return `語意索引 ${s.indexed_files} 檔`;
}
