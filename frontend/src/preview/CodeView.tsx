import { useEffect, useRef, useState } from 'react';
import { EditorState, Compartment, StateEffect, StateField, RangeSet, type Extension } from '@codemirror/state';
import {
  EditorView,
  lineNumbers,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  drawSelection,
  keymap,
  Decoration,
  gutter,
  GutterMarker,
  ViewPlugin,
  type DecorationSet,
} from '@codemirror/view';
import { syntaxHighlighting, defaultHighlightStyle, HighlightStyle, StreamLanguage, foldGutter, bracketMatching, indentOnInput } from '@codemirror/language';
import { tags as t } from '@lezer/highlight';
import { search, searchKeymap, openSearchPanel, highlightSelectionMatches } from '@codemirror/search';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { cpp } from '@codemirror/lang-cpp';
import { java } from '@codemirror/lang-java';
import { python } from '@codemirror/lang-python';
import { rust } from '@codemirror/lang-rust';
import { javascript } from '@codemirror/lang-javascript';
import { json } from '@codemirror/lang-json';
import { yaml } from '@codemirror/lang-yaml';
import { sql } from '@codemirror/lang-sql';
import { markdown } from '@codemirror/lang-markdown';
import { toml } from '@codemirror/legacy-modes/mode/toml';
import { shell } from '@codemirror/legacy-modes/mode/shell';
import { stex } from '@codemirror/legacy-modes/mode/stex';
import { xml } from '@codemirror/legacy-modes/mode/xml';
import { css } from '@codemirror/legacy-modes/mode/css';
import { Icon } from '../components/Icon';

export function languageExtension(lang: string): Extension {
  switch (lang) {
    case 'c':
    case 'cpp':
      return cpp();
    case 'java':
      return java();
    case 'python':
      return python();
    case 'rust':
      return rust();
    case 'javascript':
      return javascript();
    case 'jsx':
      return javascript({ jsx: true });
    case 'typescript':
      return javascript({ typescript: true });
    case 'tsx':
      return javascript({ typescript: true, jsx: true });
    case 'json':
      return json();
    case 'yaml':
      return yaml();
    case 'sql':
      return sql();
    case 'markdown':
      return markdown();
    case 'toml':
      return StreamLanguage.define(toml);
    case 'shell':
      return StreamLanguage.define(shell);
    case 'latex':
      return StreamLanguage.define(stex);
    case 'html':
      return StreamLanguage.define(xml);
    case 'css':
      return StreamLanguage.define(css);
    case 'pio':
      return StreamLanguage.define(pioMode);
    default:
      return [];
  }
}

const LABEL: Record<string, string> = {
  c: 'C', cpp: 'C++', java: 'Java', python: 'Python', rust: 'Rust', javascript: 'JavaScript', jsx: 'JSX',
  typescript: 'TypeScript', tsx: 'TSX', json: 'JSON', yaml: 'YAML', sql: 'SQL', markdown: 'Markdown',
  toml: 'TOML', shell: 'Shell', latex: 'LaTeX 原始碼', html: 'HTML/XML（純顯示，不執行）', css: 'CSS', text: '純文字',
  pio: 'PIO 組合語言',
};

/** RP2040 / RP2350 PIO assembly (`.pio`). */
const pioMode = {
  name: 'pio',
  token(stream: import('@codemirror/language').StringStream): string | null {
    if (stream.eatSpace()) return null;
    if (stream.match(';') || stream.match('//')) {
      stream.skipToEnd();
      return 'comment';
    }
    if (stream.match(/^\.[a-z_]+/) || stream.match(/^%.*/)) return 'meta';
    if (stream.match(/^(0x[0-9a-f]+|0b[01]+|\d+)/i)) return 'number';
    if (stream.match(/^(jmp|wait|in|out|push|pull|mov|irq|set|nop)\b/i)) return 'keyword';
    if (stream.match(/^(side|opt|block|noblock|iffull|ifempty|rel|pins|pindirs|x|y|null|isr|osr|exec|pc|status|gpio|pin|public|clear|nowait)\b/i)) return 'atom';
    if (stream.match(/^[A-Za-z_]\w*:/)) return 'labelName';
    if (stream.match(/^[A-Za-z_]\w*/)) return 'variableName';
    stream.next();
    return null;
  },
};

/** A static-analysis finding shown in the editor. */
export interface Diagnostic {
  line: number;
  severity: 'error' | 'warning' | 'perf' | 'info';
  message: string;
  title?: string;
}

const SEV_LABEL: Record<Diagnostic['severity'], string> = { error: '錯誤', warning: '警告', perf: '可優化', info: '建議' };

class DiagMarker extends GutterMarker {
  constructor(
    readonly severity: Diagnostic['severity'],
    readonly text: string,
  ) {
    super();
  }
  eq(o: DiagMarker) {
    return o.severity === this.severity && o.text === this.text;
  }
  toDOM() {
    const el = document.createElement('span');
    el.className = `cm-diag-dot ${this.severity}`;
    el.title = this.text;
    el.setAttribute('aria-label', this.text);
    return el;
  }
}

const setDiags = StateEffect.define<Diagnostic[]>();
const RANK = { error: 0, warning: 1, perf: 2, info: 3 };
/** Line highlights + gutter markers; positions follow edits until refreshed. */
const diagField = StateField.define<{ lines: DecorationSet; marks: RangeSet<GutterMarker> }>({
  create: () => ({ lines: Decoration.none, marks: RangeSet.empty }),
  update(v, tr) {
    for (const e of tr.effects) {
      if (!e.is(setDiags)) continue;
      const doc = tr.state.doc;
      const byLine = new Map<number, Diagnostic[]>();
      for (const d of e.value) {
        if (d.line < 1 || d.line > doc.lines) continue;
        byLine.set(d.line, [...(byLine.get(d.line) ?? []), d]);
      }
      const lines = [];
      const marks = [];
      for (const [ln, ds] of [...byLine].sort((a, b) => a[0] - b[0])) {
        ds.sort((a, b) => RANK[a.severity] - RANK[b.severity]);
        const from = doc.line(ln).from;
        const text = ds.map((d) => `${SEV_LABEL[d.severity]}：${d.title ? d.title + '—' : ''}${d.message}`).join('\n');
        lines.push(Decoration.line({ class: `cm-diag-line ${ds[0].severity}`, attributes: { title: text } }).range(from));
        marks.push(new DiagMarker(ds[0].severity, text).range(from));
      }
      return { lines: Decoration.set(lines), marks: RangeSet.of(marks) };
    }
    return tr.docChanged ? { lines: v.lines.map(tr.changes), marks: v.marks.map(tr.changes) } : v;
  },
  provide: (f) => EditorView.decorations.from(f, (v) => v.lines),
});
/**
 * The line-number gutter stays put while the text scrolls sideways under it.
 * With a translucent theme (background image) the text would show through,
 * so clip away exactly the part of the content that is behind the gutter.
 */
const clipUnderGutter = ViewPlugin.fromClass(
  class {
    constructor(readonly view: EditorView) {
      this.sync = this.sync.bind(this);
      view.scrollDOM.addEventListener('scroll', this.sync, { passive: true });
      this.sync();
    }
    sync() {
      const s = this.view.scrollDOM.scrollLeft;
      const g = (this.view.dom.querySelector('.cm-gutters') as HTMLElement | null)?.offsetWidth ?? 0;
      this.view.dom.style.setProperty('--cm-scroll-left', `${s}px`);
      this.view.dom.style.setProperty('--cm-gutter-w', `${g}px`);
    }
    update() {
      // Gutter width changes with the number of digits.
      this.sync();
    }
    destroy() {
      this.view.scrollDOM.removeEventListener('scroll', this.sync);
    }
  },
);

const diagGutter = gutter({ class: 'cm-diag-gutter', markers: (v) => v.state.field(diagField).marks });

const theme = EditorView.theme({
  '&': { height: '100%', fontSize: 'var(--code-font-size, 13.5px)', backgroundColor: 'var(--code-bg)', color: 'var(--text)' },
  '.cm-scroller': { fontFamily: 'var(--mono)', lineHeight: '1.6' },
  '.cm-content': { caretColor: 'var(--accent-500)' },
  '.cm-cursor': { borderLeftColor: 'var(--accent-500)' },
  '.cm-gutters': { backgroundColor: 'var(--code-gutter-bg)', color: 'var(--text-3)', borderRight: '1px solid var(--line)' },
  '.cm-activeLine': { backgroundColor: 'var(--code-active-line)' },
  '.cm-activeLineGutter': { backgroundColor: 'var(--code-active-line)', color: 'var(--text)' },
  '&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection': { backgroundColor: 'var(--code-selection) !important' },
  '.cm-panels': { backgroundColor: 'var(--panel)', color: 'var(--text)', borderBottom: '1px solid var(--line)' },
  '.cm-panels input, .cm-panels button': { color: 'var(--text)', backgroundColor: 'var(--surface)', border: '1px solid var(--line-strong)', borderRadius: '4px' },
  '.cm-searchMatch': { backgroundColor: 'var(--code-search-match)' },
  '.cm-searchMatch-selected': { backgroundColor: 'var(--code-search-current)' },
  '.cm-foldPlaceholder': { backgroundColor: 'var(--panel)', border: '1px solid var(--line)', color: 'var(--text-2)' },
  '.cm-line.cm-reveal': { backgroundColor: 'var(--code-reveal)' },
  '&.cm-focused': { outline: 'none' },
});

/** Token colours come from CSS variables (--syn-*), so light/dark and custom.css can restyle them. */
const highlight = HighlightStyle.define([
  { tag: [t.keyword, t.controlKeyword, t.moduleKeyword, t.operatorKeyword, t.definitionKeyword, t.modifier], color: 'var(--syn-keyword)' },
  { tag: [t.string, t.special(t.string), t.regexp, t.character], color: 'var(--syn-string)' },
  { tag: [t.number, t.bool, t.null, t.atom], color: 'var(--syn-number)' },
  { tag: [t.comment, t.lineComment, t.blockComment, t.docComment], color: 'var(--syn-comment)', fontStyle: 'italic' },
  { tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName], color: 'var(--syn-function)' },
  { tag: [t.typeName, t.className, t.namespace, t.standard(t.typeName)], color: 'var(--syn-type)' },
  { tag: [t.propertyName, t.attributeName], color: 'var(--syn-property)' },
  { tag: [t.variableName, t.labelName], color: 'var(--syn-variable)' },
  { tag: [t.definition(t.variableName), t.definition(t.propertyName)], color: 'var(--syn-definition)' },
  { tag: [t.operator, t.punctuation, t.bracket], color: 'var(--syn-punct)' },
  { tag: [t.meta, t.processingInstruction, t.annotation], color: 'var(--syn-meta)' },
  { tag: [t.tagName], color: 'var(--syn-tag)' },
  { tag: t.heading, color: 'var(--syn-heading)', fontWeight: '700' },
  { tag: t.emphasis, fontStyle: 'italic' },
  { tag: t.strong, fontWeight: '700' },
  { tag: t.link, color: 'var(--syn-link)', textDecoration: 'underline' },
  { tag: t.invalid, color: 'var(--danger)' },
]);

interface Props {
  text: string;
  /** Synchronise externally replaced text for scratch editors. */
  controlled?: boolean;
  lang: string;
  /** Extra toolbar content (encoding selector, save button…). */
  toolbar?: React.ReactNode;
  editable?: boolean;
  onChange?: (text: string) => void;
  onSave?: () => void;
  /** 1-based line to scroll to and highlight. */
  line?: number;
  /** Static-analysis findings to mark in the gutter. */
  diagnostics?: Diagnostic[];
  /** Start with line wrapping on (prose, e.g. Markdown). */
  defaultWrap?: boolean;
}

/**
 * Code viewer / editor: highlighting, line numbers, search (Ctrl/Cmd+F), copy,
 * wrap, optional editing with Ctrl/Cmd+S. The document is created once per
 * mount (remount with a React key to load new text). CRLF files keep CRLF.
 */
export default function CodeView({ text, controlled = false, lang, toolbar, editable = false, onChange, onSave, line, diagnostics, defaultWrap = false }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const comps = useRef({ wrap: new Compartment(), ro: new Compartment(), lang: new Compartment() });
  const cb = useRef({ onChange, onSave });
  cb.current = { onChange, onSave };
  const [wrap, setWrap] = useState(defaultWrap);
  const [copied, setCopied] = useState(false);
  const crlf = text.includes('\r\n');

  useEffect(() => {
    if (!host.current) return;
    const v = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: text,
        extensions: [
          clipUnderGutter,
          diagField,
          diagGutter,
          lineNumbers(),
          foldGutter(),
          highlightSpecialChars(),
          history(),
          drawSelection(),
          highlightActiveLine(),
          highlightActiveLineGutter(),
          highlightSelectionMatches(),
          bracketMatching(),
          indentOnInput(),
          revealField,
          crlf ? EditorState.lineSeparator.of('\r\n') : [],
          comps.current.ro.of([EditorState.readOnly.of(!editable)]),
          EditorState.tabSize.of(4),
          syntaxHighlighting(highlight),
          syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
          search({ top: true }),
          keymap.of([
            { key: 'Mod-s', preventDefault: true, run: () => (cb.current.onSave?.(), true) },
            indentWithTab,
            ...searchKeymap,
            ...historyKeymap,
            ...defaultKeymap,
          ]),
          comps.current.lang.of(languageExtension(lang)),
          comps.current.wrap.of(defaultWrap ? EditorView.lineWrapping : []),
          theme,
          EditorView.updateListener.of((u) => {
            if (u.docChanged) cb.current.onChange?.(u.state.doc.toString());
          }),
          EditorView.contentAttributes.of({ 'aria-label': editable ? '程式碼編輯器' : '程式碼內容（唯讀）' }),
        ],
      }),
    });
    view.current = v;
    return () => {
      v.destroy();
      view.current = null;
    };
    // The document is intentionally created once per mount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    view.current?.dispatch({ effects: comps.current.ro.reconfigure([EditorState.readOnly.of(!editable)]) });
  }, [editable]);
  useEffect(() => {
    view.current?.dispatch({ effects: comps.current.lang.reconfigure(languageExtension(lang)) });
  }, [lang]);
  useEffect(() => {
    view.current?.dispatch({ effects: comps.current.wrap.reconfigure(wrap ? EditorView.lineWrapping : []) });
  }, [wrap]);
  useEffect(() => {
    view.current?.dispatch({ effects: setDiags.of(diagnostics ?? []) });
  }, [diagnostics]);
  useEffect(() => {
    const v = view.current;
    if (!v || !line) return;
    const ln = v.state.doc.line(Math.min(Math.max(1, line), v.state.doc.lines));
    v.dispatch({ selection: { anchor: ln.from }, effects: [EditorView.scrollIntoView(ln.from, { y: 'center' }), setReveal.of(ln.from)] });
  }, [line]);

  useEffect(() => {
    const v = view.current;
    if (controlled && v && v.state.doc.toString() !== text) {
      v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: text } });
    }
  }, [text, controlled]);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(view.current?.state.doc.toString() ?? text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard denied */
    }
  };

  return (
    <div className="code-view">
      <div className="viewer-toolbar">
        <span className="badge">{LABEL[lang] ?? lang}</span>
        {crlf && <span className="badge muted" title="保留 Windows 換行">CRLF</span>}
        {toolbar}
        <span className="spacer" />
        <button
          className="btn ghost sm"
          onClick={() => view.current && (view.current.focus(), openSearchPanel(view.current))}
          title="搜尋（Ctrl+F）"
        >
          <Icon name="search" /> <span className="btn-label">搜尋</span>
        </button>
        <button className={`btn ghost sm ${wrap ? 'active' : ''}`} onClick={() => setWrap((w) => !w)} aria-pressed={wrap} title="自動換行">
          <Icon name="wrap" /> <span className="btn-label">自動換行</span>
        </button>
        <button className="btn ghost sm" onClick={copy} title="複製全部">
          <Icon name="copy" /> <span className="btn-label">{copied ? '已複製' : '複製'}</span>
        </button>
      </div>
      <div className="code-host" ref={host} />
    </div>
  );
}

// Highlight a revealed line (semantic-search jump) until the next edit.
const setReveal = StateEffect.define<number>();
const revealField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(deco, tr) {
    for (const e of tr.effects) if (e.is(setReveal)) return Decoration.set([Decoration.line({ class: 'cm-reveal' }).range(e.value)]);
    return tr.docChanged ? Decoration.none : deco.map(tr.changes);
  },
  provide: (f) => EditorView.decorations.from(f),
});
