import ReactMarkdown, { defaultUrlTransform, type Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import rehypeKatex from 'rehype-katex';
import rehypeHighlight from 'rehype-highlight';
import { relativeUrl } from '../api';
import 'katex/dist/katex.min.css';

const ABSOLUTE = /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i;

function decode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

interface Props {
  source: string;
  /** File the Markdown lives in; relative links/images resolve against its folder. */
  fileId?: string;
  onOpenPath?: (relPath: string) => void;
}

/**
 * Safe Markdown renderer:
 * - raw HTML is never rendered (react-markdown default, no rehype-raw);
 * - dangerous URL schemes are stripped by `defaultUrlTransform`;
 * - external images are not loaded (privacy) — shown as a placeholder link;
 * - relative images go through the authenticated `/relative` endpoint, which
 *   re-checks permission on the target file.
 */
/** Drop a trailing ` # comment` (outside quotes). */
function stripComment(v: string): string {
  let quote = '';
  for (let i = 0; i < v.length; i++) {
    const c = v[i];
    if (quote) {
      if (c === quote) quote = '';
    } else if (c === '"' || c === "'") quote = c;
    else if (c === '#' && (i === 0 || /\s/.test(v[i - 1]))) return v.slice(0, i);
  }
  return v;
}

/** `--- key: value … ---` at the very top (note tags); returned separately so it is not rendered as text. */
export function splitFrontMatter(source: string): { meta: [string, string][]; body: string } {
  const m = /^\uFEFF?---\r?\n([\s\S]*?)\r?\n(?:---|\.\.\.)[ \t]*(?:\r?\n|$)/.exec(source);
  if (!m) return { meta: [], body: source };
  const meta: [string, string][] = [];
  for (const line of m[1].split(/\r?\n/)) {
    const item = /^\s*-\s+(.*)$/.exec(line);
    if (item && meta.length) {
      const last = meta[meta.length - 1];
      const v = stripComment(item[1]).trim();
      last[1] = last[1] ? `${last[1]}, ${v}` : v;
      continue;
    }
    const kv = /^([^:#]+):\s*(.*)$/.exec(line);
    if (kv) meta.push([kv[1].trim(), stripComment(kv[2]).trim().replace(/^\[(.*)\]$/, '$1')]);
  }
  return { meta, body: source.slice(m[0].length) };
}

export default function Markdown({ source: raw, fileId, onOpenPath }: Props) {
  const { meta, body: source } = splitFrontMatter(raw);
  const components: Components = {
    img({ src, alt, title }) {
      const s = typeof src === 'string' ? src : '';
      if (!s) return <span className="md-img-blocked">［圖片網址無效］</span>;
      if (ABSOLUTE.test(s)) {
        return (
          <span className="md-img-blocked" title="外部資源預設不自動載入，以免洩漏瀏覽資訊">
            🖼 外部圖片未載入：
            <a href={s} target="_blank" rel="noopener noreferrer nofollow">
              {alt || s}
            </a>
          </span>
        );
      }
      if (!fileId) return <span className="md-img-blocked">{alt}</span>;
      return <img src={relativeUrl(fileId, decode(s.split(/[?#]/)[0]))} alt={alt ?? ''} title={title} loading="lazy" />;
    },
    a({ href, children, title }) {
      const h = typeof href === 'string' ? href : '';
      if (!h) return <span>{children}</span>;
      if (h.startsWith('#')) return <a href={h} title={title}>{children}</a>;
      if (ABSOLUTE.test(h)) {
        return (
          <a href={h} title={title} target="_blank" rel="noopener noreferrer nofollow">
            {children}
          </a>
        );
      }
      return (
        <a
          href="#"
          title={title ?? decode(h)}
          className="md-internal-link"
          onClick={(e) => {
            e.preventDefault();
            onOpenPath?.(decode(h.split('#')[0]));
          }}
        >
          {children}
        </a>
      );
    },
  };
  return (
    <div className="markdown-body">
      {meta.length > 0 && (
        <div className="md-frontmatter" aria-label="筆記標註">
          {meta.map(([k, v]) => (
            <span key={k} className="md-fm-row">
              <span className="md-fm-key">{k}</span>
              {v
                .split(',')
                .map((x) => x.trim().replace(/^["']|["']$/g, ''))
                .filter(Boolean)
                .map((x) => (
                  <code key={x} className="md-fm-val">{x}</code>
                ))}
            </span>
          ))}
        </div>
      )}
      <ReactMarkdown
        remarkPlugins={[remarkGfm, [remarkMath, { singleDollarTextMath: true }]]}
        rehypePlugins={[
          [rehypeKatex, { throwOnError: false, strict: 'ignore', trust: false, output: 'htmlAndMathml' }],
          [rehypeHighlight, { detect: false, plainText: ['text', 'txt', 'plain'] }],
        ]}
        urlTransform={defaultUrlTransform}
        components={components}
      >
        {source}
      </ReactMarkdown>
    </div>
  );
}
