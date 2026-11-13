// Shared model for both layouts (Drive / IDE): navigation, tabs with dirty
// state, dialogs, uploads and every file operation.
import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ApiError, contentUrl, errorText, type Listing, type NodeInfo } from '../api';
import { filesFromDrop, useUploads, type PendingFile } from '../components/Uploads';
import { discardDraft } from './drafts';

export interface Tab {
  node: NodeInfo;
  /** Line to reveal (from semantic search). */
  line?: number;
  dirty?: boolean;
  /** Bumped to force a reload of the tab content. */
  rev?: number;
}

export type Dialog =
  | { t: 'mkdir'; parent: string; parentPath: string }
  | { t: 'newfile'; parent: string; parentPath: string }
  | { t: 'rename'; item: NodeInfo }
  | { t: 'delete'; items: NodeInfo[] }
  | { t: 'move'; items: NodeInfo[] }
  | { t: 'keys' }
  | { t: 'password' }
  | { t: 'settings'; section?: string }
  | { t: 'confirm-close'; id: string }
  | { t: 'trash' }
  | { t: 'share'; item: NodeInfo }
  | { t: 'shares' }
  | { t: 'diagnose' }
  | null;

function readUrl() {
  const p = new URLSearchParams(window.location.search);
  return { dir: p.get('dir') || 'root', file: p.get('file') };
}

export function useDrive(opts: { admin?: boolean } = {}) {
  const admin = opts.admin ?? true;
  const [dirId, setDirId] = useState(readUrl().dir);
  const [listing, setListing] = useState<Listing | null>(null);
  const [listErr, setListErr] = useState<unknown>(null);
  const [loading, setLoading] = useState(true);
  /** Bumped after any mutation so trees / listings refresh. */
  const [version, setVersion] = useState(0);
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [dialog, setDialog] = useState<Dialog>(null);
  const [toastMsg, setToastState] = useState('');
  const [toastAction, setToastAction] = useState<{ label: string; run: () => void } | null>(null);
  const [paletteMode, setPaletteMode] = useState<null | 'all' | 'files' | 'semantic'>(null);
  const uploadTarget = useRef<string>('root');
  const fileInput = useRef<HTMLInputElement>(null);
  const dirInput = useRef<HTMLInputElement>(null);
  const savers = useRef(new Map<string, () => Promise<void>>());

  const toast = useCallback((msg: string, action?: { label: string; run: () => void }) => {
    setToastState(msg);
    setToastAction(action ?? null);
  }, []);
  useEffect(() => {
    if (!toastMsg) return;
    const t = setTimeout(() => (setToastState(''), setToastAction(null)), toastAction ? 8000 : 4000);
    return () => clearTimeout(t);
  }, [toastMsg, toastAction]);

  const bump = useCallback(() => setVersion((v) => v + 1), []);

  const load = useCallback(async (id: string, quiet = false) => {
    if (!quiet) setLoading(true);
    setListErr(null);
    try {
      setListing(await api.children(id));
    } catch (e) {
      setListErr(e);
      setListing(null);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load(dirId, version > 0);
  }, [dirId, version, load]);

  const syncUrl = useCallback((dir: string, file: string | null, push: boolean) => {
    const p = new URLSearchParams();
    if (dir !== 'root') p.set('dir', dir);
    if (file) p.set('file', file);
    // Keep the fragment: on share pages it holds the link secret.
    const url = `${window.location.pathname}${p.toString() ? '?' + p : ''}${window.location.hash}`;
    if (push) window.history.pushState(null, '', url);
    else window.history.replaceState(null, '', url);
  }, []);

  const activeRef = useRef(active);
  activeRef.current = active;

  const navigate = useCallback(
    (id: string) => {
      setDirId(id);
      syncUrl(id, activeRef.current, true);
    },
    [syncUrl],
  );

  const openFile = useCallback(
    (n: NodeInfo, line?: number) => {
      if (n.kind === 'dir') return navigate(n.id);
      setTabs((t) =>
        t.some((x) => x.node.id === n.id)
          ? t.map((x) => (x.node.id === n.id ? { ...x, node: n, line: line ?? x.line } : x))
          : [...t, { node: n, line }],
      );
      setActive(n.id);
      syncUrl(dirId, n.id, false);
    },
    [dirId, navigate, syncUrl],
  );

  useEffect(() => {
    const onPop = () => setDirId(readUrl().dir);
    window.addEventListener('popstate', onPop);
    const f = readUrl().file;
    if (f) api.info(f).then((n) => openFile(n), () => {});
    return () => window.removeEventListener('popstate', onPop);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const closeTab = useCallback(
    (id: string, force = false) => {
      const tab = tabs.find((t) => t.node.id === id);
      if (tab?.dirty && !force) return setDialog({ t: 'confirm-close', id });
      savers.current.delete(id);
      discardDraft(id);
      const i = tabs.findIndex((x) => x.node.id === id);
      const next = tabs.filter((x) => x.node.id !== id);
      setTabs(next);
      if (activeRef.current === id) {
        const n = next[Math.min(i, next.length - 1)];
        setActive(n?.node.id ?? null);
        syncUrl(dirId, n?.node.id ?? null, false);
      }
    },
    [tabs, dirId, syncUrl],
  );

  const closeAll = useCallback(() => {
    if (tabs.some((t) => t.dirty)) return toast('有未儲存的分頁，請先儲存或個別關閉');
    tabs.forEach((t) => discardDraft(t.node.id));
    setTabs([]);
    setActive(null);
    syncUrl(dirId, null, false);
  }, [tabs, dirId, syncUrl, toast]);

  /** Close several tabs at once (an editor group); refuses if any is unsaved. */
  const closeTabs = useCallback(
    (ids: string[]) => {
      const set = new Set(ids);
      if (tabs.some((t) => set.has(t.node.id) && t.dirty)) return toast('有未儲存的分頁，請先儲存或個別關閉');
      ids.forEach((id) => (savers.current.delete(id), discardDraft(id)));
      setTabs(tabs.filter((t) => !set.has(t.node.id)));
      if (activeRef.current && set.has(activeRef.current)) {
        setActive(null);
        syncUrl(dirId, null, false);
      }
    },
    [tabs, dirId, syncUrl, toast],
  );

  const setDirty = useCallback((id: string, dirty: boolean) => {
    setTabs((t) => (t.some((x) => x.node.id === id && !!x.dirty !== dirty) ? t.map((x) => (x.node.id === id ? { ...x, dirty } : x)) : t));
  }, []);

  const updateNode = useCallback((n: NodeInfo) => {
    setTabs((t) => t.map((x) => (x.node.id === n.id ? { ...x, node: n } : x)));
  }, []);

  /** Editors register a save function so Ctrl+S / the palette can trigger it. */
  const registerSaver = useCallback((id: string, fn: (() => Promise<void>) | null) => {
    if (fn) savers.current.set(id, fn);
    else savers.current.delete(id);
  }, []);
  const saveActive = useCallback(async () => {
    const fn = activeRef.current && savers.current.get(activeRef.current);
    if (fn) await fn();
  }, []);

  const uploads = useUploads(
    useCallback(() => {
      bump();
    }, [bump]),
  );

  const pathOf = useCallback(
    async (id: string) => (id === 'root' ? '/' : ((await api.info(id).catch(() => null))?.path ?? '/')),
    [],
  );

  const actions = {
    newFolder: async (parent: string) => setDialog({ t: 'mkdir', parent, parentPath: await pathOf(parent) }),
    newFile: async (parent: string) => setDialog({ t: 'newfile', parent, parentPath: await pathOf(parent) }),
    upload: (parent: string) => {
      uploadTarget.current = parent;
      fileInput.current?.click();
    },
    uploadFolder: (parent: string) => {
      uploadTarget.current = parent;
      dirInput.current?.click();
    },
    uploadFiles: async (parent: string, files: PendingFile[]) => {
      if (!files.length) return;
      uploads.enqueue(files, parent, await pathOf(parent));
    },
    /** Drop from the OS: files (with folder structure) plus empty folders. */
    uploadDrop: async (parent: string, dt: DataTransfer) => {
      const { files, emptyDirs } = await filesFromDrop(dt);
      const base = await pathOf(parent);
      for (const d of emptyDirs) {
        await api.mkdirPath(`${base === '/' ? '' : base}/${d}`).catch((e) => toast(errorText(e)));
      }
      if (emptyDirs.length) bump();
      if (files.length) uploads.enqueue(files, parent, base);
    },
    rename: (item: NodeInfo) => setDialog({ t: 'rename', item }),
    move: (items: NodeInfo[]) => items.length && setDialog({ t: 'move', items }),
    remove: (items: NodeInfo[]) => items.length && setDialog({ t: 'delete', items }),
    download: (n: NodeInfo) => {
      if (n.kind !== 'file') return;
      const a = document.createElement('a');
      a.href = contentUrl(n.id);
      a.download = n.name;
      a.click();
    },
    copyPath: async (n: NodeInfo) => {
      const p = n.path ?? (await api.info(n.id)).path ?? n.name;
      await navigator.clipboard.writeText(p).catch(() => {});
      toast(`已複製路徑：${p}`);
    },
    openPath: async (from: NodeInfo, rel: string) => {
      try {
        const n = await api.resolve(from.id, rel);
        openFile(n);
      } catch (e) {
        toast(e instanceof ApiError && e.status === 404 ? `找不到連結的檔案：${rel}` : errorText(e));
      }
    },
    createFile: async (parent: string, name: string) => {
      const n = await api.createFile(parent, name);
      bump();
      openFile(n);
      return n;
    },
    createFolder: async (parent: string, name: string) => {
      const n = await api.mkdir(parent, name);
      bump();
      return n;
    },
    afterDelete: (ids: string[], permanent = false) => {
      const set = new Set(ids);
      ids.forEach(discardDraft);
      setTabs((t) => t.filter((x) => !set.has(x.node.id)));
      if (activeRef.current && set.has(activeRef.current)) setActive(null);
      bump();
      if (!permanent && !admin) toast(`已將 ${ids.length} 個項目移到垃圾桶（分享者可以還原）`);
      else if (!permanent)
        toast(`已將 ${ids.length} 個項目移到垃圾桶`, {
          label: '復原',
          run: async () => {
            for (const id of ids) await api.restore(id).catch(() => {});
            bump();
            toast('已復原');
          },
        });
    },
    afterRename: (n: NodeInfo) => {
      updateNode(n);
      bump();
    },
  };

  return {
    dirId,
    navigate,
    listing,
    listErr,
    loading,
    reload: () => load(dirId),
    version,
    bump,
    tabs,
    active,
    setActive: (id: string | null) => {
      setActive(id);
      syncUrl(dirId, id, false);
    },
    openFile,
    closeTab,
    closeAll,
    closeTabs,
    setDirty,
    updateNode,
    registerSaver,
    saveActive,
    dialog,
    setDialog,
    toastMsg,
    toastAction,
    toast,
    paletteMode,
    setPaletteMode,
    uploads,
    actions,
    uploadTarget,
    fileInput,
    dirInput,
  };
}

export type DriveModel = ReturnType<typeof useDrive>;
