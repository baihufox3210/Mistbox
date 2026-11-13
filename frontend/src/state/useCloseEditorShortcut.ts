import { useEffect } from 'react';
import type { DriveModel } from './useDrive';

/** Mounted only in IDE. Browser-reserved Ctrl+W may never reach the page. */
export function useCloseEditorShortcut({ active, dialog, closeTab }: Pick<DriveModel, 'active' | 'dialog' | 'closeTab'>) {
  useEffect(() => {
    const close = (e: KeyboardEvent) => {
      const key = e.key.toLowerCase();
      const primary = (e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && key === 'w';
      const fallback = e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey && key === 'w';
      if ((!primary && !fallback) || e.isComposing || e.defaultPrevented) return;
      e.preventDefault();
      e.stopPropagation();
      if (!e.repeat && !dialog && active) closeTab(active);
    };
    window.addEventListener('keydown', close, true);
    return () => window.removeEventListener('keydown', close, true);
  }, [active, dialog, closeTab]);
}
