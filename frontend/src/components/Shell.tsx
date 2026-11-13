import { Logo } from './Logo';
// Chrome shared by both layouts: top bar (command centre instead of a search
// box), global shortcuts, command palette, dialogs, hidden upload inputs, toast.
import { useEffect, useMemo, useState } from 'react';
import { api } from '../api';
import { applyTheme, getThemePref, loadCustomCss, resolvedTheme, type Layout } from '../theme';
import type { DriveModel } from '../state/useDrive';
import CommandPalette, { type Command } from './CommandPalette';
import { Icon } from './Icon';
import KeysPage from './KeysPage';
import { DeleteDialog, Modal, MoveDialog, PasswordDialog, PromptDialog } from './Modal';
import SettingsDialog from './SettingsDialog';
import TrashDialog from './TrashDialog';
import SharesDialog from './SharesDialog';
import { useCaps } from '../caps';

const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform);
export const MOD = isMac ? '⌘' : 'Ctrl';

export function Topbar({
  m,
  username,
  layout,
  setLayout,
  onLogout,
  leading,
}: {
  m: DriveModel;
  username: string;
  layout: Layout;
  setLayout: (l: Layout) => void;
  onLogout: () => void;
  leading?: React.ReactNode;
}) {
  const [menu, setMenu] = useState(false);
  const [dark, setDark] = useState(resolvedTheme() === 'dark');
  useEffect(() => {
    const f = () => setDark(resolvedTheme() === 'dark');
    window.addEventListener('mistbox:theme', f);
    return () => window.removeEventListener('mistbox:theme', f);
  }, []);
  return (
    <header className="topbar" data-region="topbar">
      {leading}
      <div className="brand">
        <Logo size={24} />
        <span>Mistbox</span>
      </div>
      <button className="cmd-center" onClick={() => m.setPaletteMode('all')} title="指令面板（Ctrl+Shift+P / F1 / Ctrl+K）">
        <Icon name="search" />
        <span className="cmd-text">搜尋檔案、指令…</span>
        <kbd>{MOD}+Shift+P</kbd>
      </button>
      <div className="topbar-actions">
        <button className="btn ghost icon-only" onClick={() => (setLayout('ide'), m.openDiagnose())} title="診斷／優化（Ctrl+Shift+D）：貼上錯誤訊息或程式碼" aria-label="診斷／優化">
          <Icon name="diagnose" />
        </button>
        <button className="btn ghost icon-only" onClick={() => m.setPaletteMode('semantic')} title="語意搜尋（Ctrl+Shift+F）" aria-label="語意搜尋">
          <Icon name="sparkle" />
        </button>
        <button
          className="btn ghost icon-only"
          onClick={() => applyTheme(dark ? 'light' : 'dark')}
          title={dark ? '切換為淺色' : '切換為深色'}
          aria-label="切換主題"
        >
          <Icon name={dark ? 'sun' : 'moon'} />
        </button>
        <button
          className="btn ghost icon-only"
          onClick={() => setLayout(layout === 'drive' ? 'ide' : 'drive')}
          title={layout === 'drive' ? '切換為 IDE 版面' : '切換為雲端硬碟版面'}
          aria-label="切換版面"
        >
          <Icon name={layout === 'drive' ? 'code' : 'grid'} />
        </button>
        <div className="user-menu">
          <button className="btn ghost" onClick={() => setMenu((o) => !o)} aria-expanded={menu} aria-haspopup="menu">
            {username} ▾
          </button>
          {menu && (
            <div className="menu" role="menu" onClick={() => setMenu(false)}>
              <button role="menuitem" onClick={() => m.setDialog({ t: 'settings' })}>
                <Icon name="gear" /> 設定（主題、背景、自訂 CSS、語意搜尋）
              </button>
              <button role="menuitem" onClick={() => m.setDialog({ t: 'keys' })}>
                <Icon name="key" /> API Keys
              </button>
              <button role="menuitem" onClick={() => m.setDialog({ t: 'password' })}>
                <Icon name="lock" /> 變更密碼
              </button>
              <button role="menuitem" onClick={onLogout}>
                <Icon name="logout" /> 登出
              </button>
            </div>
          )}
        </div>
      </div>
    </header>
  );
}

export function useCommands(
  m: DriveModel,
  opts: { target: () => string; layout: Layout; setLayout: (l: Layout) => void; onLogout: () => void; extra?: Command[] },
): Command[] {
  const { target, layout, setLayout, onLogout, extra } = opts;
  const caps = useCaps();
  return useMemo(() => {
    const activeNode = m.tabs.find((t) => t.node.id === m.active && t.kind !== 'diagnose')?.node;
    const c: Command[] = [
      { id: 'new-file', label: '新增檔案', icon: 'filePlus', keywords: 'create new file 建立', run: () => m.actions.newFile(target()) },
      { id: 'new-folder', label: '新增資料夾', icon: 'folderPlus', keywords: 'mkdir create directory', run: () => m.actions.newFolder(target()) },
      { id: 'upload', label: '上傳檔案', icon: 'upload', keywords: 'upload', run: () => m.actions.upload(target()) },
      { id: 'upload-folder', label: '上傳資料夾', icon: 'folder', keywords: 'upload directory', run: () => m.actions.uploadFolder(target()) },
      { id: 'diagnose', label: '診斷／優化模式（貼上錯誤訊息或程式碼）', icon: 'diagnose', shortcut: `${MOD}+Shift+D`, keywords: 'diagnose debug error fix optimize 診斷 除錯 錯誤 優化 解決', run: () => (setLayout('ide'), m.openDiagnose()) },
      { id: 'semantic', label: '語意搜尋（本地模型）', icon: 'sparkle', shortcut: `${MOD}+Shift+F`, keywords: 'rag vector semantic 向量 相似', run: () => setTimeout(() => m.setPaletteMode('semantic')) },
      { id: 'goto-file', label: '前往檔案（依檔名）', icon: 'search', shortcut: `${MOD}+P`, keywords: 'open quick find', run: () => setTimeout(() => m.setPaletteMode('files')) },
      { id: 'save', label: '儲存目前檔案', icon: 'save', shortcut: `${MOD}+S`, keywords: 'save write', run: () => m.saveActive() },
      { id: 'layout-drive', label: '切換版面：雲端硬碟', icon: 'grid', keywords: 'layout view google drive', run: () => setLayout('drive') },
      { id: 'layout-ide', label: '切換版面：IDE', icon: 'code', keywords: 'layout view editor vscode', run: () => setLayout('ide') },
      { id: 'theme-light', label: '主題：淺色', icon: 'sun', keywords: 'theme light', run: () => applyTheme('light') },
      { id: 'theme-dark', label: '主題：深色', icon: 'moon', keywords: 'theme dark night', run: () => applyTheme('dark') },
      { id: 'theme-system', label: '主題：跟隨系統', icon: 'layout', keywords: 'theme system auto', run: () => applyTheme('system') },
      { id: 'settings', label: '開啟設定（背景圖片、自訂 CSS、語意搜尋）', icon: 'gear', keywords: 'preferences background css', run: () => m.setDialog({ t: 'settings' }) },
      { id: 'reload-css', label: '重新載入自訂 CSS（theme/custom.css）', icon: 'refresh', keywords: 'custom css theme reload', run: () => (loadCustomCss(true), m.toast('已重新載入 custom.css')) },
      { id: 'refresh', label: '重新整理檔案清單', icon: 'refresh', keywords: 'reload refresh', run: () => m.bump() },
      { id: 'trash', label: '開啟垃圾桶（還原已刪除的項目）', icon: 'trash', keywords: 'trash recycle bin restore 刪除 還原 回收', run: () => m.setDialog({ t: 'trash' }) },
      { id: 'go-root', label: '回到根目錄', icon: 'folder', keywords: 'home root', run: () => m.navigate('root') },
      { id: 'close-all', label: '關閉所有分頁', icon: 'close', keywords: 'close tabs', run: () => m.closeAll() },
      { id: 'keys', label: '管理 API Keys', icon: 'key', keywords: 'api token', run: () => m.setDialog({ t: 'keys' }) },
      { id: 'shares', label: '管理分享連結', icon: 'link', keywords: 'share link 分享 連結', run: () => m.setDialog({ t: 'shares' }) },
      { id: 'password', label: '變更密碼', icon: 'lock', keywords: 'password', run: () => m.setDialog({ t: 'password' }) },
      { id: 'logout', label: '登出', icon: 'logout', keywords: 'logout sign out', run: onLogout },
    ];
    if (layout === 'ide') {
      const grid = (detail: string) => () => window.dispatchEvent(new CustomEvent('mistbox:editor', { detail }));
      c.splice(7, 0,
        { id: 'split-right', label: '編輯器：向右分割', icon: 'splitRight', shortcut: `${MOD}+\\`, keywords: 'split editor right 分割 並排 左右', run: grid('split-right') },
        { id: 'split-down', label: '編輯器：向下分割', icon: 'splitDown', shortcut: `${MOD}+Alt+\\`, keywords: 'split editor down 分割 上下', run: grid('split-down') },
        { id: 'focus-next-group', label: '編輯器：切換到下一個群組', icon: 'layout', shortcut: `${MOD}+1…9`, keywords: 'focus next editor group 群組', run: grid('focus-next') },
        { id: 'join-groups', label: '編輯器：合併所有群組', icon: 'layout', keywords: 'join merge editor groups 合併 取消分割', run: grid('join-all') },
        { id: 'close-group', label: '編輯器：關閉目前群組', icon: 'close', keywords: 'close editor group 群組', run: grid('close-group') },
        { id: 'problems', label: '切換問題面板（靜態分析、重複程式碼）', icon: 'sevWarning', shortcut: `${MOD}+Shift+M`, keywords: 'problems lint static analysis duplicate 問題 分析 重複 優化', run: () => window.dispatchEvent(new Event('mistbox:problems')) },
      );
    }
    if (activeNode) {
      c.splice(7, 0,
        { id: 'download-active', label: `下載目前檔案：${activeNode.name}`, icon: 'download', keywords: 'download', run: () => m.actions.download(activeNode) },
        { id: 'copy-path', label: `複製路徑：${activeNode.name}`, icon: 'copy', keywords: 'copy path', run: () => m.actions.copyPath(activeNode) },
        { id: 'rename-active', label: `重新命名：${activeNode.name}`, icon: 'edit', keywords: 'rename', run: () => m.actions.rename(activeNode) },
        { id: 'share-active', label: `分享連結：${activeNode.name}`, icon: 'link', keywords: 'share link 分享', run: () => m.setDialog({ t: 'share', item: activeNode }) },
      );
    }
    // Share-link viewers: no admin features; read-only links cannot change anything.
    const adminOnly = new Set(['semantic', 'problems', 'diagnose', 'layout-drive', 'layout-ide', 'settings', 'reload-css', 'trash', 'keys', 'shares', 'share-active', 'password', 'logout']);
    const writeOnly = new Set(['new-file', 'new-folder', 'upload', 'upload-folder', 'save', 'rename-active']);
    const allowed = (x: Command) => (caps.admin || !adminOnly.has(x.id)) && (caps.write || !writeOnly.has(x.id));
    return [...(extra ?? []), ...c.filter((x) => x.id !== `layout-${layout}` && allowed(x))];
  }, [m, target, layout, setLayout, onLogout, extra, caps]);
}

/** Global shortcuts, palette, dialogs, hidden inputs and toast. */
export function ShellOverlays({
  m,
  commands,
  layout,
  setLayout,
}: {
  m: DriveModel;
  commands: Command[];
  layout: Layout;
  setLayout: (l: Layout) => void;
}) {
  const caps = useCaps();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.ctrlKey || e.metaKey;
      const k = e.key.toLowerCase();
      if ((mod && e.shiftKey && k === 'p') || e.key === 'F1' || (mod && !e.shiftKey && k === 'k')) {
        e.preventDefault();
        m.setPaletteMode('all');
      } else if (mod && !e.shiftKey && k === 'p') {
        e.preventDefault();
        m.setPaletteMode('files');
      } else if (mod && e.shiftKey && k === 'd') {
        e.preventDefault();
        if (caps.admin) (setLayout('ide'), m.openDiagnose());
      } else if (mod && e.shiftKey && k === 'f') {
        e.preventDefault();
        if (caps.admin) m.setPaletteMode('semantic');
      } else if (mod && k === 's') {
        e.preventDefault();
        m.saveActive();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [m, caps.admin]);

  // Warn before leaving with unsaved edits.
  const dirty = m.tabs.some((t) => t.dirty);
  useEffect(() => {
    if (!dirty) return;
    const h = (e: BeforeUnloadEvent) => e.preventDefault();
    window.addEventListener('beforeunload', h);
    return () => window.removeEventListener('beforeunload', h);
  }, [dirty]);

  const d = m.dialog;
  const close = () => m.setDialog(null);
  const confirmTab = d?.t === 'confirm-close' ? m.tabs.find((t) => t.node.id === d.id) : undefined;

  return (
    <>
      <input
        ref={m.fileInput}
        type="file"
        multiple
        hidden
        onChange={(e) => {
          m.actions.uploadFiles(m.uploadTarget.current, Array.from(e.target.files ?? []).map((file) => ({ file })));
          e.target.value = '';
        }}
      />
      <input
        ref={m.dirInput}
        type="file"
        hidden
        // @ts-expect-error non-standard but widely supported
        webkitdirectory=""
        onChange={(e) => {
          m.actions.uploadFiles(
            m.uploadTarget.current,
            Array.from(e.target.files ?? []).map((file) => ({ file, relPath: file.webkitRelativePath || undefined })),
          );
          e.target.value = '';
        }}
      />
      {m.paletteMode && (
        <CommandPalette
          key={m.paletteMode}
          mode={m.paletteMode}
          commands={commands}
          onOpenFile={(n, line) => m.openFile(n, line)}
          onClose={() => m.setPaletteMode(null)}
        />
      )}
      {m.toastMsg && (
        <div className="toast" role="status">
          {m.toastMsg}
          {m.toastAction && (
            <button className="toast-action" onClick={() => (m.toastAction?.run(), m.toast(''))}>
              {m.toastAction.label}
            </button>
          )}
        </div>
      )}

      {d?.t === 'newfile' && (
        <PromptDialog
          title={`新增檔案到 ${d.parentPath}`}
          label="檔名（含副檔名，例如 筆記.md、main.py）"
          initial="未命名.md"
          confirm="建立"
          onClose={close}
          onSubmit={async (name) => {
            await m.actions.createFile(d.parent, name);
          }}
        />
      )}
      {d?.t === 'mkdir' && (
        <PromptDialog
          title={`新增資料夾到 ${d.parentPath}`}
          label="資料夾名稱"
          initial="新資料夾"
          confirm="建立"
          onClose={close}
          onSubmit={async (name) => {
            await m.actions.createFolder(d.parent, name);
          }}
        />
      )}
      {d?.t === 'rename' && (
        <PromptDialog
          title="重新命名"
          label="新名稱"
          initial={d.item.name}
          confirm="重新命名"
          onClose={close}
          onSubmit={async (name) => {
            m.actions.afterRename(await api.update(d.item.id, { name }));
          }}
        />
      )}
      {d?.t === 'delete' && (
        <DeleteDialog items={d.items} onClose={close} onDone={(permanent) => m.actions.afterDelete(d.items.map((i) => i.id), permanent)} />
      )}
      {d?.t === 'trash' && <TrashDialog onClose={close} onChanged={(msg) => (m.bump(), msg && m.toast(msg))} />}
      {d?.t === 'move' && <MoveDialog items={d.items} onClose={close} onDone={m.bump} />}
      {d?.t === 'keys' && <KeysPage onClose={close} />}
      {d?.t === 'share' && <SharesDialog node={d.item} onClose={close} toast={m.toast} />}
      {d?.t === 'shares' && <SharesDialog onClose={close} toast={m.toast} />}
      {d?.t === 'password' && <PasswordDialog onClose={close} />}
      {d?.t === 'settings' && <SettingsDialog onClose={close} layout={layout} setLayout={setLayout} />}
      {d?.t === 'confirm-close' && confirmTab && (
        <Modal title="尚未儲存" onClose={close}>
          <p>「{confirmTab.node.name}」有尚未儲存的變更。</p>
          <div className="modal-actions">
            <button className="btn" onClick={close} data-autofocus>取消</button>
            <button className="btn danger" onClick={() => (close(), m.closeTab(d.id, true))}>不儲存並關閉</button>
          </div>
        </Modal>
      )}
    </>
  );
}

export { getThemePref };
