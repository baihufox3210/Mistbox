// Short-lived, versioned text previews. Each workspace owns its cache so
// decrypted/private text cannot carry over into another login or share.
import { createContext, useMemo, type ReactNode } from 'react';
import type { TextPreview } from '../api';

const TTL = 15_000;
const MAX_ENTRIES = 16;
const MAX_CHARS = 2 * 1024 * 1024;
interface Entry {
  promise: Promise<TextPreview>;
  data?: TextPreview;
  expires: number;
}
export class TextCache {
  private entries = new Map<string, Entry>();
  peek(key: string): TextPreview | null {
    const e = this.entries.get(key);
    if (!e || e.expires <= Date.now()) { this.entries.delete(key); return null; }
    return e.data ?? null;
  }
  load(key: string, fetch: () => Promise<TextPreview>): Promise<TextPreview> {
    const old = this.entries.get(key);
    if (old && old.expires > Date.now()) {
      this.entries.delete(key);
      this.entries.set(key, old);
      return old.promise;
    }
    const promise = fetch();
    const e: Entry = { promise, expires: Date.now() + TTL };
    e.promise = promise.then((data) => {
      // A newer request may already own this key.
      if (this.entries.get(key) === e) {
        e.data = data;
        e.expires = Date.now() + TTL;
        let chars = 0;
        for (const entry of this.entries.values()) chars += entry.data?.text.length ?? 0;
        while (this.entries.size > MAX_ENTRIES || chars > MAX_CHARS) {
          const oldest = this.entries.keys().next().value;
          if (oldest === undefined) break;
          chars -= this.entries.get(oldest)?.data?.text.length ?? 0;
          this.entries.delete(oldest);
        }
      }
      return data;
    }, (error) => {
      if (this.entries.get(key) === e) this.entries.delete(key);
      throw error;
    });
    this.entries.set(key, e);
    while (this.entries.size > MAX_ENTRIES) this.entries.delete(this.entries.keys().next().value!);
    return e.promise;
  }
}
export const TextCacheContext = createContext<TextCache | null>(null);
export function TextCacheProvider({ version, children }: { version: number; children: ReactNode }) {
  // Explicit refreshes and file mutations discard previews immediately.
  const cache = useMemo(() => new TextCache(), [version]);
  return <TextCacheContext.Provider value={cache}>{children}</TextCacheContext.Provider>;
}
