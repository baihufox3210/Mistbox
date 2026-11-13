import { describe, expect, it, vi } from 'vitest';
import { render, fireEvent, cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';
import Markdown from './Markdown';

afterEach(cleanup);

const doc = String.raw`# 標題：中文 Markdown

段落含有**粗體**與中文標點，以及行內公式 $E = mc^2$ 和 $\alpha_i$。

| 欄位 | 數值 |
| ---- | ---: |
| 甲   | 1    |
| 乙   | 2    |

- 第一層
  - 第二層
    1. 第三層有序
- [x] 已完成
- [ ] 待辦

> 引用文字

$$
\int_0^1 x^2\,dx = \frac{1}{3}
$$

\`\`\`python
def f(x):
    return "$not math$"
\`\`\`

\`\`\`c
int main(void) { return 0; }
\`\`\`

行內程式碼 \`$PATH\` 不是公式。

![本地圖](<./img/圖 1.png>)
![remote](https://example.com/tracker.png)

[相對文件](../docs/讀我.md) [外部](https://example.com) [危險](javascript:alert(1))

<script>alert('xss')</script>
<img src=x onerror="alert(1)">
`;

function setup(onOpenPath = vi.fn()) {
  const r = render(<Markdown source={doc.replaceAll('\\`', '`')} fileId="file-123" onOpenPath={onOpenPath} />);
  return { ...r, onOpenPath };
}

describe('Markdown preview', () => {
  it('renders headings, tables, nested & task lists, quotes', () => {
    const { container } = setup();
    expect(container.querySelector('h1')?.textContent).toBe('標題：中文 Markdown');
    expect(container.querySelectorAll('table tbody tr')).toHaveLength(2);
    expect(container.querySelector('th:last-child')?.getAttribute('style') ?? container.querySelector('th:last-child')?.getAttribute('align')).toMatch(/right/);
    expect(container.querySelector('ul ul ol li')?.textContent).toBe('第三層有序');
    const boxes = container.querySelectorAll<HTMLInputElement>('input[type=checkbox]');
    expect(boxes).toHaveLength(2);
    expect(boxes[0].checked).toBe(true);
    expect(container.querySelector('blockquote')?.textContent).toContain('引用文字');
  });

  it('renders inline and block math with KaTeX, but not inside code', () => {
    const { container } = setup();
    expect(container.querySelectorAll('.katex').length).toBeGreaterThanOrEqual(3);
    expect(container.querySelector('.katex-display')).not.toBeNull();
    const pyCode = container.querySelector('code.language-python')!;
    expect(pyCode.textContent).toContain('"$not math$"');
    expect(pyCode.querySelector('.katex')).toBeNull();
    const inline = Array.from(container.querySelectorAll('p code')).find((c) => c.textContent === '$PATH');
    expect(inline).toBeTruthy();
  });

  it('highlights fenced code by language', () => {
    const { container } = setup();
    const py = container.querySelector('code.language-python')!;
    expect(py.classList.contains('hljs')).toBe(true);
    expect(py.querySelector('.hljs-keyword')?.textContent).toBe('def');
    const c = container.querySelector('code.language-c')!;
    expect(c.querySelector('.hljs-type, .hljs-keyword')).not.toBeNull();
  });

  it('routes relative images through the authenticated endpoint and blocks external ones', () => {
    const { container } = setup();
    const imgs = container.querySelectorAll('img');
    expect(imgs).toHaveLength(1);
    const src = imgs[0].getAttribute('src')!;
    expect(src.startsWith('/api/v1/nodes/file-123/relative?path=')).toBe(true);
    expect(decodeURIComponent(src.split('path=')[1])).toBe('./img/圖 1.png');
    expect(container.textContent).toContain('外部圖片未載入');
    expect(container.innerHTML).not.toContain('src="https://example.com/tracker.png"');
  });

  it('never renders raw HTML or javascript: URLs', () => {
    const { container } = setup();
    expect(container.querySelector('script')).toBeNull();
    expect(container.querySelector('[onerror]')).toBeNull();
    expect(container.innerHTML).not.toMatch(/javascript:/i);
    const ext = Array.from(container.querySelectorAll('a')).find((a) => a.textContent === '外部')!;
    expect(ext.getAttribute('rel')).toContain('noopener');
    expect(ext.getAttribute('target')).toBe('_blank');
  });

  it('opens relative document links inside the app', () => {
    const { container, onOpenPath } = setup();
    const link = Array.from(container.querySelectorAll('a')).find((a) => a.textContent === '相對文件')!;
    fireEvent.click(link);
    expect(onOpenPath).toHaveBeenCalledWith('../docs/讀我.md');
  });
});

import { splitFrontMatter } from './Markdown';

describe('front matter', () => {
  it('is split off and parsed', () => {
    const r = splitFrontMatter('---\nrules: [omp-race, bit-precedence]\nsymbols:\n  - dma_x\n  - pio_y\n---\n# Title\n');
    expect(r.body).toBe('# Title\n');
    expect(r.meta).toEqual([
      ['rules', 'omp-race, bit-precedence'],
      ['symbols', 'dma_x, pio_y'],
    ]);
    expect(splitFrontMatter('# no\n---\n').meta).toEqual([]);
    expect(splitFrontMatter('---\nrules: [a, b]   # 規則\n---\n').meta).toEqual([['rules', 'a, b']]);
  });
});
