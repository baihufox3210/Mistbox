import { previewKind } from '../fileTypes';

/** Warm only the viewer relevant to the file the user is about to open. */
export function preloadPreview(name: string) {
  const kind = previewKind(name);
  const modules: Promise<unknown>[] = [import('./Preview')];
  if (kind === 'markdown') modules.push(import('./Markdown'), import('./CodeView'));
  else if (kind === 'pdf') modules.push(import('./PdfView'));
  else if (kind === 'latex') modules.push(import('./LatexView'));
  else if (kind === 'code' || kind === 'unknown') modules.push(import('./CodeView'));
  void Promise.all(modules).catch(() => {}); // Normal render retains retry/error handling.
}
