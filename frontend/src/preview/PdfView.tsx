import { useEffect, useRef, useState } from 'react';
import * as pdfjs from 'pdfjs-dist';
import type { PDFDocumentProxy } from 'pdfjs-dist';
import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url';

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

/** Renders PDF bytes with pdf.js onto canvases (no browser plugin, no scripts in the PDF). */
export default function PdfView({ data }: { data: ArrayBuffer }) {
  const [doc, setDoc] = useState<PDFDocumentProxy | null>(null);
  const [error, setError] = useState('');
  const [scale, setScale] = useState(1.2);

  useEffect(() => {
    let cancelled = false;
    setDoc(null);
    setError('');
    const task = pdfjs.getDocument({
      data: new Uint8Array(data.slice(0)),
      cMapUrl: '/pdfjs/cmaps/',
      cMapPacked: true,
      standardFontDataUrl: '/pdfjs/standard_fonts/',
      wasmUrl: '/pdfjs/wasm/',
      enableXfa: false,
    });
    task.promise.then(
      (d) => !cancelled && setDoc(d),
      (e) => !cancelled && setError(String(e?.message ?? e)),
    );
    return () => {
      cancelled = true;
      task.destroy();
    };
  }, [data]);

  if (error) return <div className="state error">PDF 無法顯示：{error}</div>;
  if (!doc) return <div className="state"><span className="spinner" /> 載入 PDF…</div>;
  return (
    <div className="pdf-view">
      <div className="viewer-toolbar">
        <span className="muted">{doc.numPages} 頁</span>
        <span className="spacer" />
        <button className="btn ghost sm" onClick={() => setScale((s) => Math.max(0.5, s - 0.2))} aria-label="縮小">−</button>
        <span className="muted">{Math.round(scale * 100)}%</span>
        <button className="btn ghost sm" onClick={() => setScale((s) => Math.min(3, s + 0.2))} aria-label="放大">＋</button>
      </div>
      <div className="pdf-pages">
        {Array.from({ length: doc.numPages }, (_, i) => (
          <PdfPage key={i} doc={doc} n={i + 1} scale={scale} />
        ))}
      </div>
    </div>
  );
}

function PdfPage({ doc, n, scale }: { doc: PDFDocumentProxy; n: number; scale: number }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [visible, setVisible] = useState(n <= 2);
  const [size, setSize] = useState<{ w: number; h: number } | null>(null);

  useEffect(() => {
    const el = canvas.current;
    if (!el || visible) return;
    const io = new IntersectionObserver((es) => es.some((e) => e.isIntersecting) && setVisible(true), { rootMargin: '600px' });
    io.observe(el);
    return () => io.disconnect();
  }, [visible]);

  useEffect(() => {
    let cancelled = false;
    let task: { cancel: () => void; promise: Promise<void> } | null = null;
    doc.getPage(n).then((page) => {
      if (cancelled) return;
      const vp = page.getViewport({ scale });
      setSize({ w: vp.width, h: vp.height });
      if (!visible || !canvas.current) return;
      const ratio = window.devicePixelRatio || 1;
      const c = canvas.current;
      c.width = Math.floor(vp.width * ratio);
      c.height = Math.floor(vp.height * ratio);
      task = page.render({ canvas: c, viewport: page.getViewport({ scale: scale * ratio }) });
      task.promise.catch(() => {});
    });
    return () => {
      cancelled = true;
      task?.cancel();
    };
  }, [doc, n, scale, visible]);

  return (
    <canvas
      ref={canvas}
      className="pdf-page"
      aria-label={`第 ${n} 頁`}
      style={size ? { width: size.w, height: size.h } : { width: 600, height: 800 }}
    />
  );
}
