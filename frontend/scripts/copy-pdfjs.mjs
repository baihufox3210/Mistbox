// Copy pdf.js runtime assets (CMaps for CJK, standard fonts, wasm decoders)
// into public/ so they are served same-origin; nothing is fetched from CDNs.
import { cpSync, mkdirSync } from 'node:fs';
const src = new URL('../node_modules/pdfjs-dist/', import.meta.url);
const dst = new URL('../public/pdfjs/', import.meta.url);
mkdirSync(dst, { recursive: true });
for (const d of ['cmaps', 'standard_fonts', 'wasm']) {
  cpSync(new URL(d + '/', src), new URL(d + '/', dst), { recursive: true });
}
