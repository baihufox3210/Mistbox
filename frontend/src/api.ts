// Thin client for /api/v1. The session lives in an HttpOnly cookie; the CSRF
// token is kept only in memory (never localStorage).

export interface NodeInfo {
  id: string;
  parent_id: string | null;
  name: string;
  kind: 'dir' | 'file';
  size: number;
  mime: string | null;
  sha256: string | null;
  sensitive: boolean;
  created_at: string;
  modified_at: string;
  path?: string;
  stats?: { files: number; dirs: number; bytes: number };
}

export interface Listing {
  node: NodeInfo;
  breadcrumbs: { id: string; name: string }[];
  children: NodeInfo[];
}

export interface TextPreview {
  encoding: string;
  detected: string;
  valid_utf8: boolean;
  had_errors: boolean;
  binary: boolean;
  truncated: boolean;
  size: number;
  limit: number;
  text: string;
  encodings: string[];
}

export interface ApiKeyInfo {
  id: string;
  name: string;
  prefix: string;
  permissions: { list: boolean; read: boolean; write: boolean };
  allow_sensitive: boolean;
  scope: { id: string; kind: string | null; path: string | null; covers: string };
  status: 'active' | 'revoked' | 'expired' | 'scope_deleted';
  created_at: string;
  expires_at: string | null;
  last_used_at: string | null;
  revoked_at: string | null;
}

export interface TrashItem extends NodeInfo {
  trashed_at: string;
  purge_at: string | null;
}

export type SharePermission = 'view' | 'edit';

export interface ShareInfo {
  id: string;
  node: { id: string; name: string; kind: 'dir' | 'file'; path: string } | null;
  permission: SharePermission;
  label: string;
  status: 'active' | 'expired' | 'unavailable';
  created_at: string;
  expires_at: string | null;
  last_used_at: string | null;
  use_count: number;
}

/** What a link holder gets from `POST /share/open`. */
export interface OpenedShare {
  id: string;
  permission: SharePermission;
  label: string;
  expires_at: string | null;
  root: NodeInfo;
}

export interface UiSettings {
  background_url: string;
  background_dim: number;
  background_blur: number;
  theme_css_url: string | null;
}

export interface RagStatus {
  enabled: boolean;
  model?: string;
  model_description?: string;
  state?: 'loading' | 'ready' | 'indexing' | 'error';
  error?: string | null;
  indexed_files?: number;
  chunks?: number;
  pending?: number;
  skipped?: number;
  failed?: number;
  /** The model is unloaded after a few idle minutes and reloaded on demand. */
  model_loaded?: boolean;
  /** PDF text extraction (sandboxed worker) is configured. */
  pdf?: boolean;
}

export interface RagHit {
  node: NodeInfo;
  score: number;
  seq: number;
  line_start: number;
  line_end: number;
  /** What line_start/line_end count: lines, or pages for PDFs. */
  unit: 'line' | 'page';
  text: string;
}

export class ApiError extends Error {
  constructor(
    public status: number,
    public code: string,
    message: string,
    public details: Record<string, unknown> = {},
  ) {
    super(message);
  }
}

let csrf = '';
export const setCsrf = (t: string) => {
  csrf = t;
};

/**
 * Share-link mode: every request carries `X-Share-Token` (kept in memory,
 * read from the URL fragment). Browser-initiated GETs (img/video/download)
 * name the share with `?share=<id>` and use the HttpOnly cookie set by
 * `api.shareOpen()`.
 */
let share: { id: string; token: string } | null = null;
export const setShare = (id: string, secret: string) => {
  share = { id, token: `mbs_${id}_${secret}` };
};
export const inShareMode = () => share !== null;

function authHeaders(method: string): Record<string, string> {
  if (share) return { 'X-Share-Token': share.token };
  return method !== 'GET' && method !== 'HEAD' ? { 'X-CSRF-Token': csrf } : {};
}

/** Append `share=<id>` for URLs the browser fetches by itself. */
function withShare(url: string): string {
  if (!share) return url;
  return url + (url.includes('?') ? '&' : '?') + 'share=' + share.id;
}

export const UNAUTHORIZED_EVENT = 'mistbox:unauthorized';

async function toError(res: Response): Promise<ApiError> {
  let body: { error?: { code?: string; message?: string } & Record<string, unknown> } = {};
  try {
    body = await res.json();
  } catch {
    /* non-JSON */
  }
  const e = body.error ?? {};
  const { code, message, ...details } = e;
  return new ApiError(res.status, code ?? 'http_' + res.status, message ?? `HTTP ${res.status}`, details);
}

async function raw(method: string, url: string, body?: unknown): Promise<Response> {
  const headers: Record<string, string> = authHeaders(method);
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  let res: Response;
  try {
    res = await fetch(url, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      credentials: 'same-origin',
    });
  } catch {
    throw new ApiError(0, 'network', '無法連線到伺服器');
  }
  if (!res.ok) {
    const err = await toError(res);
    if (res.status === 401 && !share && !url.endsWith('/auth/login') && !url.endsWith('/auth/me')) {
      window.dispatchEvent(new Event(UNAUTHORIZED_EVENT));
    }
    throw err;
  }
  return res;
}

export type Severity = 'error' | 'warning' | 'perf' | 'info';

export interface CodeRule {
  id: string;
  severity: Severity;
  category: string;
  title: string;
  hint: string;
}

export interface CodeFinding {
  node_id: string;
  rule: string;
  severity: Severity;
  line: number;
  col: number;
  line_end: number;
  message: string;
  symbol: string | null;
  path?: string;
  name?: string;
}

export interface CodeFindings {
  node: NodeInfo;
  /** For a file: index state (results match the content when `sha256` equals the file's). */
  index: { status: 'indexed' | 'skipped' | 'failed'; sha256: string; lang: string | null; detail: string | null; lines: number } | null;
  findings: CodeFinding[];
}

export interface DuplicateSide {
  id: string;
  name: string;
  path: string;
  line_start: number;
  line_end: number;
}

export interface CodeDuplicate {
  a: DuplicateSide;
  b: DuplicateSide;
  tokens: number;
}

export interface CodeStatus {
  state: string;
  files: number;
  pending: number;
  by_lang: Record<string, number>;
  findings: Partial<Record<Severity, number>>;
}

async function json<T>(method: string, url: string, body?: unknown): Promise<T> {
  const res = await raw(method, url, body);
  if (res.status === 204) return undefined as T;
  return res.json() as Promise<T>;
}

const q = encodeURIComponent;
const V = '/api/v1';

export const api = {
  me: () => json<{ username: string; csrf_token: string }>('GET', `${V}/auth/me`),
  login: (username: string, password: string) =>
    json<{ username: string; csrf_token: string }>('POST', `${V}/auth/login`, { username, password }),
  logout: () => json<void>('POST', `${V}/auth/logout`),
  changePassword: (current_password: string, new_password: string) =>
    json<{ username: string; csrf_token: string }>('POST', `${V}/auth/password`, { current_password, new_password }),

  info: (id: string, stats = false) => json<NodeInfo>('GET', `${V}/nodes/${q(id)}${stats ? '?stats=1' : ''}`),
  children: (id: string) => json<Listing>('GET', `${V}/nodes/${q(id)}/children`),
  mkdir: (parent: string, name: string) => json<NodeInfo>('POST', `${V}/nodes/${q(parent)}/children`, { name }),
  update: (id: string, patch: { name?: string; parent_id?: string; sensitive?: boolean }) =>
    json<NodeInfo>('PATCH', `${V}/nodes/${q(id)}`, patch),
  /** Move to the recycle bin (or delete permanently). */
  remove: (id: string, permanent = false) =>
    json<{ deleted: string; permanent: boolean }>('DELETE', `${V}/nodes/${q(id)}${permanent ? '?permanent=1' : ''}`),
  trash: () => json<{ items: TrashItem[]; retention_days: number }>('GET', `${V}/trash`),
  restore: (id: string) =>
    json<{ node: NodeInfo; restored_to_root: boolean }>('POST', `${V}/trash/${q(id)}/restore`),
  emptyTrash: () => json<{ deleted: number }>('DELETE', `${V}/trash`),
  /** mkdir -p relative to the caller's root. */
  mkdirPath: (path: string) => json<NodeInfo>('POST', `${V}/fs/mkdir?path=${q(path)}`),
  search: (term: string) => json<{ results: NodeInfo[] }>('GET', `${V}/search?q=${q(term)}&limit=200`),
  text: (id: string, encoding?: string, partial = false) =>
    json<TextPreview>(
      'GET',
      `${V}/nodes/${q(id)}/text?${encoding ? `encoding=${q(encoding)}&` : ''}${partial ? 'partial=1' : ''}`,
    ),
  resolve: (id: string, path: string) => json<NodeInfo>('GET', `${V}/nodes/${q(id)}/resolve?path=${q(path)}`),
  bytes: async (url: string) => (await raw('GET', url)).arrayBuffer(),

  latexCompile: (id: string) =>
    json<{ ok: true; cached: boolean; engine: string; files: string[]; missing: string[]; pdf_url: string }>(
      'POST',
      `${V}/nodes/${q(id)}/latex`,
    ),

  /** Create an empty file (upload with an empty body; never overwrites). */
  createFile: (parent: string, name: string) =>
    json<NodeInfo>('POST', `${V}/nodes/${q(parent)}/files?name=${q(name)}&on_conflict=fail`),
  /** Replace file content; `sha` must be the version being edited (else 412). */
  putContent: async (id: string, text: string, sha: string): Promise<NodeInfo> => {
    let res: Response;
    try {
      res = await fetch(`${V}/nodes/${q(id)}/content`, {
        method: 'PUT',
        headers: { ...authHeaders('PUT'), 'If-Match': `"${sha}"`, 'Content-Type': 'application/octet-stream' },
        body: new TextEncoder().encode(text),
        credentials: 'same-origin',
      });
    } catch {
      throw new ApiError(0, 'network', '無法連線到伺服器');
    }
    if (!res.ok) throw await toError(res);
    return res.json();
  },

  settings: () => json<UiSettings>('GET', `${V}/settings`),
  saveSettings: (s: Partial<UiSettings>) => json<UiSettings>('PUT', `${V}/settings`, s),

  ragStatus: () => json<RagStatus>('GET', `${V}/rag/status`),
  codeStatus: () => json<CodeStatus>('GET', `${V}/code/status`),
  codeRules: () => json<{ rules: CodeRule[] }>('GET', `${V}/code/rules`),
  codeFindings: (nodeId?: string, severity?: Severity) =>
    json<CodeFindings>('GET', `${V}/code/findings?${new URLSearchParams({ ...(nodeId ? { node_id: nodeId } : {}), ...(severity ? { severity } : {}) })}`),
  codeDuplicates: (nodeId?: string) =>
    json<{ node: NodeInfo; duplicates: CodeDuplicate[] }>('GET', `${V}/code/duplicates${nodeId ? `?node_id=${q(nodeId)}` : ''}`),
  codeReindex: () => json<{ ok: boolean }>('POST', `${V}/code/reindex`),
  ragSearch: (query: string, k = 12) =>
    json<{ results: RagHit[] }>('GET', `${V}/rag/search?q=${q(query)}&k=${k}`),
  ragReindex: () => json<{ ok: boolean }>('POST', `${V}/rag/reindex`),

  shares: (nodeId?: string) =>
    json<{ shares: ShareInfo[] }>('GET', `${V}/shares${nodeId ? `?node_id=${q(nodeId)}` : ''}`),
  /** The returned `link` (relative, contains the secret) is only available now. */
  createShare: (body: { node_id: string; permission: SharePermission; expires_at: string | null; label?: string }) =>
    json<{ link: string; share: ShareInfo }>('POST', `${V}/shares`, body),
  updateShare: (id: string, patch: { permission?: SharePermission; expires_at?: string | null; label?: string }) =>
    json<ShareInfo>('PATCH', `${V}/shares/${q(id)}`, patch),
  deleteShare: (id: string) => json<void>('DELETE', `${V}/shares/${q(id)}`),
  shareOpen: () => json<OpenedShare>('POST', `${V}/share/open`),

  keys: () => json<{ keys: ApiKeyInfo[] }>('GET', `${V}/keys`),
  createKey: (body: {
    name: string;
    permissions: { list: boolean; read: boolean; write: boolean };
    scope_node_id: string;
    expires_at: string | null;
    allow_sensitive: boolean;
  }) => json<{ key: string; api_key: ApiKeyInfo }>('POST', `${V}/keys`, body),
  revokeKey: (id: string) => json<ApiKeyInfo>('DELETE', `${V}/keys/${q(id)}`),
};

export const contentUrl = (id: string, inline = false) => withShare(`${V}/nodes/${q(id)}/content${inline ? '?inline=1' : ''}`);
export const relativeUrl = (id: string, path: string) => withShare(`${V}/nodes/${q(id)}/relative?path=${q(path)}`);

export type ConflictPolicy = 'fail' | 'rename';

/** Upload one file with progress (XHR gives upload progress events; fetch does not). */
export function uploadFile(
  dirId: string,
  file: File,
  opts: { relPath?: string; onConflict?: ConflictPolicy; onProgress?: (loaded: number, total: number) => void },
): { promise: Promise<NodeInfo>; abort: () => void } {
  const xhr = new XMLHttpRequest();
  const params = new URLSearchParams();
  if (opts.relPath) params.set('path', opts.relPath);
  else params.set('name', file.name);
  params.set('on_conflict', opts.onConflict ?? 'fail');
  if (file.lastModified) params.set('mtime', String(Math.floor(file.lastModified / 1000)));
  const promise = new Promise<NodeInfo>((resolve, reject) => {
    xhr.open('POST', `${V}/nodes/${q(dirId)}/files?${params}`);
    for (const [k, v] of Object.entries(authHeaders('POST'))) xhr.setRequestHeader(k, v);
    xhr.setRequestHeader('Content-Type', 'application/octet-stream');
    xhr.upload.onprogress = (e) => opts.onProgress?.(e.loaded, e.lengthComputable ? e.total : file.size);
    xhr.onload = () => {
      let body: { error?: { code: string; message: string } & Record<string, unknown> } & Partial<NodeInfo> = {};
      try {
        body = JSON.parse(xhr.responseText);
      } catch {
        /* ignore */
      }
      if (xhr.status >= 200 && xhr.status < 300) resolve(body as NodeInfo);
      else {
        if (xhr.status === 401 && !share) window.dispatchEvent(new Event(UNAUTHORIZED_EVENT));
        const e = body.error ?? { code: 'http_' + xhr.status, message: `HTTP ${xhr.status}` };
        reject(new ApiError(xhr.status, e.code, e.message));
      }
    };
    xhr.onerror = () => reject(new ApiError(0, 'network', '網路錯誤，上傳中斷'));
    xhr.onabort = () => reject(new ApiError(0, 'aborted', '已取消'));
    xhr.send(file);
  });
  return { promise, abort: () => xhr.abort() };
}

export function errorText(e: unknown): string {
  if (e instanceof ApiError) {
    if (e.status === 403) return `沒有權限：${e.message}`;
    return e.message;
  }
  return e instanceof Error ? e.message : String(e);
}
