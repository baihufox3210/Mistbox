// Map file names to preview kinds and CodeMirror languages. Storage never
// depends on this: every file is saved and downloadable regardless of type.

export type PreviewKind = 'markdown' | 'latex' | 'code' | 'image' | 'pdf' | 'audio' | 'video' | 'binary' | 'unknown';

export function ext(name: string): string {
  const i = name.lastIndexOf('.');
  return i > 0 ? name.slice(i + 1).toLowerCase() : '';
}

/** extension -> language id understood by CodeView */
export const CODE_LANG: Record<string, string> = {
  c: 'c', h: 'c', cpp: 'cpp', cc: 'cpp', cxx: 'cpp', hpp: 'cpp', hh: 'cpp', hxx: 'cpp', ino: 'cpp',
  cu: 'cpp', cuh: 'cpp', pio: 'pio',
  java: 'java', kt: 'java', scala: 'java',
  py: 'python', pyw: 'python', pyi: 'python',
  rs: 'rust',
  js: 'javascript', mjs: 'javascript', cjs: 'javascript', jsx: 'jsx',
  ts: 'typescript', mts: 'typescript', cts: 'typescript', tsx: 'tsx',
  json: 'json', jsonc: 'json', webmanifest: 'json',
  yaml: 'yaml', yml: 'yaml',
  toml: 'toml',
  sh: 'shell', bash: 'shell', zsh: 'shell', ksh: 'shell',
  sql: 'sql',
  tex: 'latex', sty: 'latex', cls: 'latex', bib: 'latex',
  md: 'markdown', markdown: 'markdown',
  html: 'html', htm: 'html', xml: 'html', svg: 'html', css: 'css', go: 'cpp', cs: 'java', swift: 'java',
  txt: 'text', log: 'text', csv: 'text', tsv: 'text', ini: 'text', cfg: 'text', conf: 'text', env: 'text',
};

/** Names without extension that are well-known text formats. */
const TEXT_NAMES: Record<string, string> = {
  makefile: 'shell', dockerfile: 'shell', '.bashrc': 'shell', '.zshrc': 'shell', '.profile': 'shell',
  '.gitignore': 'text', '.dockerignore': 'text', license: 'text', readme: 'text', '.editorconfig': 'text',
};

const IMAGE = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'avif', 'bmp', 'ico', 'svg']);
const AUDIO = new Set(['mp3', 'wav', 'ogg', 'oga', 'm4a', 'aac', 'flac', 'opus', 'weba']);
const VIDEO = new Set(['mp4', 'webm', 'ogv', 'mov', 'm4v']);
const BINARY = new Set([
  'zip', '7z', 'rar', 'gz', 'tgz', 'bz2', 'xz', 'zst', 'tar', 'iso', 'dmg', 'exe', 'dll', 'so', 'dylib', 'bin',
  'o', 'a', 'class', 'jar', 'war', 'doc', 'docx', 'xls', 'xlsx', 'ppt', 'pptx', 'odt', 'ods', 'odp', 'psd',
  'ttf', 'otf', 'woff', 'woff2', 'sqlite', 'db', 'pyc', 'wasm', 'heic', 'tif', 'tiff', 'mkv', 'avi', 'wmv',
]);

export function previewKind(name: string): PreviewKind {
  const e = ext(name);
  if (e === 'md' || e === 'markdown') return 'markdown';
  if (e === 'tex') return 'latex';
  if (e === 'pdf') return 'pdf';
  if (IMAGE.has(e)) return 'image';
  if (AUDIO.has(e)) return 'audio';
  if (VIDEO.has(e)) return 'video';
  if (BINARY.has(e)) return 'binary';
  if (CODE_LANG[e] || TEXT_NAMES[name.toLowerCase()]) return 'code';
  return 'unknown'; // try text; server reports binary if it is
}

export function languageFor(name: string): string {
  return CODE_LANG[ext(name)] ?? TEXT_NAMES[name.toLowerCase()] ?? 'text';
}

export function typeLabel(n: { kind: string; name: string; mime: string | null }): string {
  if (n.kind === 'dir') return '資料夾';
  const e = ext(n.name);
  return e ? e.toUpperCase() : n.mime && n.mime !== 'application/octet-stream' ? n.mime : '檔案';
}
