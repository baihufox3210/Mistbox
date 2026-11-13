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
export default function Markdown({ source, fileId, onOpenPath }: Props) {
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
