// What the current viewer may do. The admin can do everything; someone
// holding a share link sees only the shared item and, unless the link allows
// editing, cannot change anything. The server enforces all of this — the UI
// just hides what would be refused.
import { createContext, useContext } from 'react';

export interface Caps {
  /** Logged-in administrator (trash, settings, keys, sharing, semantic search…). */
  admin: boolean;
  /** May create, upload, rename, move, delete and edit. */
  write: boolean;
  /** Label for "/" in breadcrumbs and the sidebar. */
  rootName: string;
}

export const ADMIN_CAPS: Caps = { admin: true, write: true, rootName: '我的雲端' };

export const CapsContext = createContext<Caps>(ADMIN_CAPS);
export const useCaps = () => useContext(CapsContext);
