import { ext, previewKind } from '../fileTypes';

/**
 * Line icons for folders and files. A file is an outlined page with a small
 * glyph for its category; colours come from CSS variables (`--icon-line`,
 * `--icon-folder`, `--icon-<category>`) so themes can restyle them. Strokes
 * keep the same pixel width at every size.
 */
function category(name: string): string {
  const e = ext(name);
  const kind = previewKind(name);
  if (kind === 'markdown') return 'md';
  if (kind === 'latex') return 'tex';
  if (kind === 'pdf') return 'pdf';
  if (kind === 'image') return 'image';
  if (kind === 'audio') return 'audio';
  if (kind === 'video') return 'video';
  if (['zip', '7z', 'rar', 'gz', 'tgz', 'xz', 'bz2', 'zst', 'tar'].includes(e)) return 'archive';
  if (['doc', 'docx', 'odt', 'rtf'].includes(e)) return 'doc';
  if (['xls', 'xlsx', 'ods', 'csv', 'tsv'].includes(e)) return 'sheet';
  if (['ppt', 'pptx', 'odp', 'key'].includes(e)) return 'slide';
  if (['txt', 'log', ''].includes(e) && kind !== 'binary') return 'text';
  if (kind === 'code') return 'code';
  return 'file';
}

/** Glyphs drawn inside the page (page spans x 5–19, y 2.5–21.5). */
const GLYPH: Record<string, string> = {
  text: 'M8.5 11.5h7M8.5 14.5h7M8.5 17.5h4.5',
  doc: 'M8.5 11.5h7M8.5 14.5h7M8.5 17.5h4.5',
  md: 'M8.5 17.5v-5l2 2.5 2-2.5v5M15.5 12.5v5M14 16l1.5 1.5L17 16',
  code: 'M10.5 12 8.5 14.5l2 2.5M13.5 12l2 2.5-2 2.5',
  tex: 'M15.5 12h-6.5l3 2.75-3 2.75h6.5',
  pdf: 'M8.5 12h7M8.5 15h7M8.5 18h7',
  image: 'M8.5 17.5l2.5-3 2 2 1.25-1.25 1.75 2.25zM14 10.75a.75.75 0 1 0 0 .01',
  audio: 'M12.5 16.5v-5.5l3-.75M11 18a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3z',
  video: 'M10.5 11.5v6l4.75-3z',
  archive: 'M12 5v1.5M12 8v1.5M12 11v1.5M10.75 14h2.5v3h-2.5z',
  sheet: 'M8.5 11.5h7v6h-7zM8.5 14.5h7M12 11.5v6',
  slide: 'M8.5 11.5h7v4.5h-7zM12 16v2M10.5 18h3',
  file: '',
};

const PAGE = 'M6.5 2.5h7.5l5 5v12.5a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 5 20V4a1.5 1.5 0 0 1 1.5-1.5zM14 2.5V6a1.5 1.5 0 0 0 1.5 1.5H19';
const FOLDER = 'M3 18V6a1.5 1.5 0 0 1 1.5-1.5h4.4l2 2h8.6A1.5 1.5 0 0 1 21 8v10a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 18zM3 9h18';
const FOLDER_OPEN =
  'M3 18V6a1.5 1.5 0 0 1 1.5-1.5h4.4l2 2h7.6A1.5 1.5 0 0 1 20 8v2M3 18l2.3-6.9A1.5 1.5 0 0 1 6.7 10H21l-2.4 8.4a1.5 1.5 0 0 1-1.4 1.1H4.5A1.5 1.5 0 0 1 3 18z';

export function FileIcon({ name, kind, size = 18, open }: { name: string; kind: 'dir' | 'file'; size?: number; open?: boolean }) {
  // Constant on-screen line weight: a touch heavier only for large icons.
  const sw = size >= 40 ? 1.4 : 1.35;
  const common = {
    width: size,
    height: size,
    viewBox: '0 0 24 24',
    fill: 'none',
    strokeLinecap: 'round' as const,
    strokeLinejoin: 'round' as const,
    'aria-hidden': true,
  };
  if (kind === 'dir') {
    return (
      <svg className="ficon ficon-folder" {...common}>
        <path d={open ? FOLDER_OPEN : FOLDER} stroke="var(--icon-folder)" strokeWidth={sw} vectorEffect="non-scaling-stroke" />
      </svg>
    );
  }
  const c = category(name);
  return (
    <svg className={`ficon ficon-${c}`} data-cat={c} {...common}>
      <path d={PAGE} stroke="var(--icon-line)" strokeWidth={sw} vectorEffect="non-scaling-stroke" />
      {GLYPH[c] && size >= 14 && (
        <path d={GLYPH[c]} stroke={`var(--icon-${c}, var(--icon-file))`} strokeWidth={sw} vectorEffect="non-scaling-stroke" />
      )}
    </svg>
  );
}
