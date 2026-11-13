import { ext, previewKind } from '../fileTypes';

/** Category → colour + short label. Colours are CSS variables so themes can restyle them. */
function category(name: string): { key: string; label: string } {
  const e = ext(name);
  const kind = previewKind(name);
  if (kind === 'markdown') return { key: 'md', label: 'MD' };
  if (kind === 'latex') return { key: 'tex', label: 'TEX' };
  if (kind === 'pdf') return { key: 'pdf', label: 'PDF' };
  if (kind === 'image') return { key: 'image', label: e === 'svg' ? 'SVG' : 'IMG' };
  if (kind === 'audio') return { key: 'audio', label: '♪' };
  if (kind === 'video') return { key: 'video', label: '▶' };
  if (['zip', '7z', 'rar', 'gz', 'tgz', 'xz', 'bz2', 'zst', 'tar'].includes(e)) return { key: 'archive', label: 'ZIP' };
  if (['doc', 'docx', 'odt', 'rtf'].includes(e)) return { key: 'doc', label: 'DOC' };
  if (['xls', 'xlsx', 'ods', 'csv', 'tsv'].includes(e)) return { key: 'sheet', label: e === 'csv' ? 'CSV' : 'XLS' };
  if (['ppt', 'pptx', 'odp', 'key'].includes(e)) return { key: 'slide', label: 'PPT' };
  if (['txt', 'log', ''].includes(e) && kind !== 'binary') return { key: 'text', label: e ? e.toUpperCase() : 'TXT' };
  if (kind === 'code') return { key: 'code', label: e.slice(0, 4).toUpperCase() || '</>' };
  return { key: 'file', label: e.slice(0, 4).toUpperCase() };
}

export function FileIcon({ name, kind, size = 18, open }: { name: string; kind: 'dir' | 'file'; size?: number; open?: boolean }) {
  if (kind === 'dir') {
    return (
      <svg className="ficon ficon-folder" width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
        <path d="M2.5 6.2c0-.9.7-1.7 1.7-1.7h4.6c.5 0 .9.2 1.2.5l1.4 1.5h8.4c.9 0 1.7.8 1.7 1.7v1.3h-19z" fill="var(--icon-folder-back)" />
        <path
          d={open ? 'M2.5 9.5h19.6l-2 9.1c-.2.8-.9 1.4-1.7 1.4H4.2c-.9 0-1.7-.8-1.7-1.7z' : 'M2.5 8.5h19v9.8c0 .9-.8 1.7-1.7 1.7H4.2c-.9 0-1.7-.8-1.7-1.7z'}
          fill="var(--icon-folder)"
        />
      </svg>
    );
  }
  const c = category(name);
  const big = size >= 40;
  return (
    <svg className={`ficon ficon-${c.key}`} width={size} height={size} viewBox="0 0 24 24" aria-hidden="true" data-cat={c.key}>
      <path d="M5.5 2h9l5 5v13.3c0 .9-.8 1.7-1.7 1.7H5.5c-.9 0-1.7-.8-1.7-1.7V3.7C3.8 2.8 4.6 2 5.5 2z" fill="var(--icon-page)" stroke="var(--icon-page-line)" strokeWidth=".8" />
      <path d="M14.5 2v3.3c0 .9.8 1.7 1.7 1.7h3.3" fill="var(--icon-page-fold)" stroke="var(--icon-page-line)" strokeWidth=".8" />
      <rect x="3.8" y={big ? 13 : 12.5} width="12.4" height={big ? 6 : 7} rx="1.4" fill={`var(--icon-${c.key}, var(--icon-file))`} />
      <text x="10" y={big ? 17.4 : 17.6} textAnchor="middle" fontSize={c.label.length > 3 ? 3.3 : 4} fontWeight="700" fill="#fff" fontFamily="system-ui, sans-serif">
        {c.label}
      </text>
    </svg>
  );
}
