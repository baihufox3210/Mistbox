import { useCallback, useEffect, useRef, useState } from 'react';
import { uploadFile, type ApiError, type ConflictPolicy } from '../api';
import { formatSize } from '../format';
import { Icon } from './Icon';

export interface PendingFile {
  file: File;
  /** Path relative to the target folder, for folder uploads ("a/b/c.txt"). */
  relPath?: string;
}

export interface UploadItem {
  key: number;
  file: File;
  relPath?: string;
  dirId: string;
  dirLabel: string;
  status: 'queued' | 'uploading' | 'done' | 'error' | 'conflict' | 'skipped';
  loaded: number;
  policy: ConflictPolicy;
  error?: string;
  storedAs?: string;
}

const CONCURRENCY = 3;
let seq = 0;

export function useUploads(onUploaded: (dirId: string) => void) {
  const [items, setItems] = useState<UploadItem[]>([]);
  const aborts = useRef(new Map<number, () => void>());
  const patch = (key: number, p: Partial<UploadItem>) => setItems((list) => list.map((i) => (i.key === key ? { ...i, ...p } : i)));

  const enqueue = useCallback((files: PendingFile[], dirId: string, dirLabel: string) => {
    setItems((list) => [
      ...list,
      ...files.map((f) => ({
        key: ++seq,
        file: f.file,
        relPath: f.relPath,
        dirId,
        dirLabel,
        status: 'queued' as const,
        loaded: 0,
        policy: 'fail' as ConflictPolicy,
      })),
    ]);
  }, []);

  // Scheduler: keep up to CONCURRENCY uploads running.
  useEffect(() => {
    const running = items.filter((i) => i.status === 'uploading').length;
    const next = items.filter((i) => i.status === 'queued').slice(0, Math.max(0, CONCURRENCY - running));
    for (const it of next) {
      patch(it.key, { status: 'uploading', loaded: 0, error: undefined });
      const { promise, abort } = uploadFile(it.dirId, it.file, {
        relPath: it.relPath,
        onConflict: it.policy,
        onProgress: (loaded) => patch(it.key, { loaded }),
      });
      aborts.current.set(it.key, abort);
      promise.then(
        (n) => {
          patch(it.key, { status: 'done', loaded: it.file.size, storedAs: n.name });
          onUploaded(it.dirId);
        },
        (e: ApiError) => {
          if (e.code === 'aborted') patch(it.key, { status: 'skipped', error: '已取消' });
          else if (e.status === 409) patch(it.key, { status: 'conflict', error: e.message });
          else patch(it.key, { status: 'error', error: e.message });
        },
      ).finally(() => aborts.current.delete(it.key));
    }
  }, [items, onUploaded]);

  return {
    items,
    enqueue,
    retry: (key: number) => patch(key, { status: 'queued' }),
    keepBoth: (key: number) => patch(key, { status: 'queued', policy: 'rename' }),
    skip: (key: number) => patch(key, { status: 'skipped', error: '已略過' }),
    cancel: (key: number) => {
      const a = aborts.current.get(key);
      if (a) a();
      else patch(key, { status: 'skipped', error: '已取消' });
    },
    clearFinished: () => setItems((l) => l.filter((i) => !['done', 'skipped'].includes(i.status))),
  };
}

export type Uploads = ReturnType<typeof useUploads>;

export function UploadPanel({ u, embedded }: { u: Uploads; embedded?: boolean }) {
  const [collapsed, setCollapsed] = useState(false);
  if (!u.items.length) return null;
  const done = u.items.filter((i) => i.status === 'done').length;
  const active = u.items.filter((i) => i.status === 'uploading' || i.status === 'queued').length;
  const problems = u.items.filter((i) => i.status === 'error' || i.status === 'conflict').length;
  return (
    <section className={`upload-panel ${embedded ? 'embedded' : 'floating'}`} aria-label="上傳進度" data-region="uploads">
      <header>
        <strong>
          上傳 {done}/{u.items.length}
        </strong>
        {active > 0 && <span className="spinner tiny" />}
        {problems > 0 && <span className="badge danger">{problems} 個需處理</span>}
        <span className="spacer" />
        <button className="btn ghost sm" onClick={u.clearFinished}>清除已完成</button>
        <button className="btn ghost sm" onClick={() => setCollapsed((c) => !c)} aria-expanded={!collapsed}>
          {collapsed ? '展開' : '收合'}
        </button>
      </header>
      {!collapsed && (
        <ul>
          {u.items.map((i) => {
            const pct = i.file.size ? Math.round((i.loaded / i.file.size) * 100) : i.status === 'done' ? 100 : 0;
            return (
              <li key={i.key} className={`up-${i.status}`}>
                <div className="up-name" title={`${i.dirLabel}/${i.relPath ?? i.file.name}`}>
                  <Icon name="file" /> {i.relPath ?? i.file.name}
                  <span className="muted small"> · {formatSize(i.file.size)}</span>
                </div>
                {i.status === 'uploading' && (
                  <div className="progress" role="progressbar" aria-label={`${i.file.name} 上傳進度`} aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
                    <div style={{ width: `${pct}%` }} />
                  </div>
                )}
                <div className="up-status">
                  {i.status === 'queued' && <span className="muted">等待中</span>}
                  {i.status === 'uploading' && <span>{pct}%</span>}
                  {i.status === 'done' && <span className="ok">完成{i.storedAs && i.storedAs !== i.file.name.split('/').pop() ? `（另存為 ${i.storedAs}）` : ''}</span>}
                  {i.status === 'skipped' && <span className="muted">{i.error}</span>}
                  {i.status === 'error' && <span className="err">{i.error}</span>}
                  {i.status === 'conflict' && <span className="err">已有同名檔案</span>}
                  {i.status === 'uploading' && <button className="btn ghost sm" onClick={() => u.cancel(i.key)}>取消</button>}
                  {i.status === 'error' && <button className="btn sm" onClick={() => u.retry(i.key)}>重試</button>}
                  {i.status === 'conflict' && (
                    <>
                      <button className="btn sm" onClick={() => u.keepBoth(i.key)}>保留兩者</button>
                      <button className="btn ghost sm" onClick={() => u.skip(i.key)}>略過</button>
                    </>
                  )}
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

/** Collect files (recursively, for dropped folders) from a drop event. */
export interface Dropped {
  files: PendingFile[];
  /** Relative paths of dropped folders that contain no files (created explicitly). */
  emptyDirs: string[];
}

export async function filesFromDrop(dt: DataTransfer): Promise<Dropped> {
  const entries = Array.from(dt.items)
    .filter((i) => i.kind === 'file')
    .map((i) => i.webkitGetAsEntry?.())
    .filter((e): e is FileSystemEntry => !!e);
  if (!entries.length) return { files: Array.from(dt.files).map((file) => ({ file })), emptyDirs: [] };
  const out: PendingFile[] = [];
  const emptyDirs: string[] = [];
  const walk = async (entry: FileSystemEntry, prefix: string): Promise<void> => {
    if (entry.isFile) {
      const file = await new Promise<File>((res, rej) => (entry as FileSystemFileEntry).file(res, rej));
      out.push({ file, relPath: prefix ? `${prefix}/${file.name}` : undefined });
    } else if (entry.isDirectory) {
      const reader = (entry as FileSystemDirectoryEntry).createReader();
      const dir = prefix ? `${prefix}/${entry.name}` : entry.name;
      // readEntries returns batches; loop until empty.
      let any = false;
      for (;;) {
        const batch = await new Promise<FileSystemEntry[]>((res, rej) => reader.readEntries(res, rej));
        if (!batch.length) break;
        any = true;
        for (const e of batch) await walk(e, dir);
      }
      if (!any) emptyDirs.push(dir);
    }
  };
  for (const e of entries) await walk(e, '');
  return { files: out, emptyDirs };
}
