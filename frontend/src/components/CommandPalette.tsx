import { useEffect, useMemo, useRef, useState } from 'react';
import { api, errorText, type NodeInfo, type RagHit } from '../api';
import { formatSize } from '../format';
import { FileIcon } from './FileIcon';
import { Icon } from './Icon';

export interface Command {
  id: string;
  label: string;
  icon?: string;
  shortcut?: string;
  keywords?: string;
  run: () => void;
}

type Mode = 'all' | 'files' | 'semantic';

interface Item {
  key: string;
  kind: 'command' | 'file' | 'semantic-go' | 'hit';
  label: string;
  sub?: string;
  icon?: React.ReactNode;
  shortcut?: string;
  run: () => void;
  snippet?: string;
}

function match(hay: string, needle: string): boolean {
  const h = hay.toLowerCase();
  return needle
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((w) => h.includes(w));
}

/**
 * VS Code-style palette.
 *   (blank prefix) commands + file names
 *   `>` commands only     `?` semantic search (local embedding model)
 */
export default function CommandPalette({
  mode,
  commands,
  onOpenFile,
  onClose,
}: {
  mode: Mode;
  commands: Command[];
  onOpenFile: (n: NodeInfo, line?: number) => void;
  onClose: () => void;
}) {
  const [text, setText] = useState(mode === 'semantic' ? '? ' : mode === 'all' ? '' : '');
  const [files, setFiles] = useState<NodeInfo[]>([]);
  const [hits, setHits] = useState<RagHit[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState('');
  const [sel, setSel] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLUListElement>(null);

  const semantic = text.startsWith('?');
  const onlyCmds = text.startsWith('>');
  const query = text.replace(/^[?>]\s*/, '').trim();
  const fileMode = mode === 'files' && !semantic && !onlyCmds;

  useEffect(() => input.current?.focus(), []);

  // File name search (debounced).
  useEffect(() => {
    if (semantic || onlyCmds || !query) {
      setFiles([]);
      return;
    }
    const t = setTimeout(() => {
      api.search(query).then((r) => setFiles(r.results.slice(0, 40)), () => setFiles([]));
    }, 150);
    return () => clearTimeout(t);
  }, [query, semantic, onlyCmds]);

  // Semantic search only on Enter (embedding a query costs CPU) — or debounced.
  const runSemantic = async () => {
    if (!query) return;
    setBusy(true);
    setErr('');
    try {
      setHits((await api.ragSearch(query, 15)).results);
    } catch (e) {
      setErr(errorText(e));
      setHits([]);
    } finally {
      setBusy(false);
    }
  };
  useEffect(() => {
    setHits(null);
    if (!semantic || query.length < 2) return;
    const t = setTimeout(runSemantic, 450);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [semantic, query]);

  const items: Item[] = useMemo(() => {
    if (semantic) {
      return (hits ?? []).map((h, i) => ({
        key: `h${i}`,
        kind: 'hit' as const,
        label: h.node.name,
        sub: `${h.node.path} · ${
          h.unit === 'page'
            ? `第 ${h.line_start} 頁`
            : `第 ${h.line_start}–${h.line_end} 行`
        } · 相似度 ${(h.score * 100).toFixed(0)}%`,
        snippet: h.text.length > 280 ? h.text.slice(0, 280) + '…' : h.text,
        icon: <FileIcon name={h.node.name} kind="file" />,
        run: () => onOpenFile(h.node, h.line_start),
      }));
    }
    const cmds: Item[] = fileMode
      ? []
      : commands
          .filter((c) => !query || match(`${c.label} ${c.keywords ?? ''} ${c.id}`, query))
          .map((c) => ({
            key: c.id,
            kind: 'command' as const,
            label: c.label,
            shortcut: c.shortcut,
            icon: <Icon name={c.icon ?? 'command'} />,
            run: c.run,
          }));
    const fs: Item[] = onlyCmds
      ? []
      : files.map((f) => ({
          key: f.id,
          kind: 'file' as const,
          label: f.name,
          sub: `${f.path ?? ''}${f.kind === 'file' ? ' · ' + formatSize(f.size) : ''}`,
          icon: <FileIcon name={f.name} kind={f.kind} />,
          run: () => onOpenFile(f),
        }));
    const sem: Item[] =
      !onlyCmds && query
        ? [
            {
              key: 'sem',
              kind: 'semantic-go' as const,
              label: `語意搜尋「${query}」`,
              sub: '用本地模型找內容相近的段落',
              icon: <Icon name="sparkle" />,
              run: () => setText(`? ${query}`),
            },
          ]
        : [];
    return fileMode ? [...fs, ...sem] : [...cmds, ...fs, ...sem];
  }, [semantic, hits, commands, files, query, onlyCmds, fileMode, onOpenFile]);

  useEffect(() => setSel(0), [text, hits, files.length]);
  useEffect(() => {
    list.current?.querySelector<HTMLElement>(`[data-i="${sel}"]`)?.scrollIntoView({ block: 'nearest' });
  }, [sel]);

  const exec = (it: Item | undefined) => {
    if (!it) return;
    if (it.kind !== 'semantic-go') onClose();
    it.run();
  };

  const placeholder =
    mode === 'files' ? '輸入檔名…（? 開頭為語意搜尋，> 開頭為指令）' : mode === 'semantic' ? '描述你要找的內容…' : '輸入指令或檔名…（? 語意搜尋，> 只顯示指令）';

  return (
    <div className="palette-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="palette" role="dialog" aria-modal="true" aria-label="指令面板" data-region="palette">
        <div className="palette-input">
          <Icon name={semantic ? 'sparkle' : onlyCmds ? 'command' : 'search'} />
          <input
            ref={input}
            value={text}
            placeholder={placeholder}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape') onClose();
              else if (e.key === 'ArrowDown') (e.preventDefault(), setSel((s) => Math.min(items.length - 1, s + 1)));
              else if (e.key === 'ArrowUp') (e.preventDefault(), setSel((s) => Math.max(0, s - 1)));
              else if (e.key === 'Enter') {
                e.preventDefault();
                if (semantic && hits === null) runSemantic();
                else exec(items[sel]);
              }
            }}
            role="combobox"
            aria-expanded="true"
            aria-controls="palette-list"
            aria-activedescendant={items[sel] ? `pal-${sel}` : undefined}
          />
          {busy && <span className="spinner tiny" />}
        </div>
        {err && <div className="palette-err">{err}</div>}
        <ul className="palette-list" id="palette-list" role="listbox" ref={list}>
          {items.map((it, i) => (
            <li
              key={it.key}
              id={`pal-${i}`}
              data-i={i}
              role="option"
              aria-selected={i === sel}
              className={`palette-item ${i === sel ? 'sel' : ''} kind-${it.kind}`}
              onMouseMove={() => setSel(i)}
              onClick={() => exec(it)}
            >
              <span className="pi-icon">{it.icon}</span>
              <span className="pi-main">
                <span className="pi-label">{it.label}</span>
                {it.sub && <span className="pi-sub">{it.sub}</span>}
                {it.snippet && <pre className="pi-snippet">{it.snippet}</pre>}
              </span>
              {it.shortcut && <kbd>{it.shortcut}</kbd>}
            </li>
          ))}
          {!items.length && (
            <li className="palette-empty">
              {semantic ? (query.length < 2 ? '輸入至少 2 個字' : busy || hits === null ? '搜尋中…' : '沒有相近的內容') : '沒有符合的項目'}
            </li>
          )}
        </ul>
        <div className="palette-foot">
          <span><kbd>↑</kbd><kbd>↓</kbd> 選擇</span>
          <span><kbd>Enter</kbd> 執行</span>
          <span><kbd>?</kbd> 語意搜尋</span>
          <span><kbd>&gt;</kbd> 指令</span>
          <span><kbd>Esc</kbd> 關閉</span>
        </div>
      </div>
    </div>
  );
}
