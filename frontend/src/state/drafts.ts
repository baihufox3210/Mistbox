// Unsaved editor text, kept outside React so it survives an editor being
// unmounted (switching tabs, moving a tab to another editor group).
// In memory only: never persisted to storage.

export interface Draft {
  text: string;
  /** Version the draft is based on (sent with the save as If-Match). */
  sha: string;
}

const drafts = new Map<string, Draft>();

export const getDraft = (id: string) => drafts.get(id);
export const putDraft = (id: string, d: Draft) => void drafts.set(id, d);
export const discardDraft = (id: string) => void drafts.delete(id);
