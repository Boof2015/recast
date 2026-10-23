import { useEffect, useRef, useState, type ReactNode } from 'react';
import { ChevronDown, Minus, Square, Copy, X } from 'lucide-react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

// WebView2 and WKWebView identify their host OS here. Only macOS needs space
// for traffic lights; Windows supplies its controls in this shared header.
const platform = /Windows/.test(navigator.userAgent) ? 'windows' : /Macintosh|Mac OS X/.test(navigator.userAgent) ? 'macos' : 'linux';

export function AppHeader({ children, onMenuOpen, onError }: {
  children: ReactNode;
  onMenuOpen: () => void;
  onError: (message: string) => void;
}) {
  const [maximized, setMaximized] = useState(false);
  const [focused, setFocused] = useState(true);
  const menuButton = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (platform !== 'windows' || !isTauri()) return;
    const appWindow = getCurrentWindow();
    let disposed = false;
    const stops: (() => void)[] = [];
    const retain = (stop: () => void) => { if (disposed) stop(); else stops.push(stop); };
    const updateMaximized = () => appWindow.isMaximized().then(value => { if (!disposed) setMaximized(value); });
    void updateMaximized().catch(() => {});
    void appWindow.isFocused().then(value => { if (!disposed) setFocused(value); }).catch(() => {});
    void appWindow.onResized(() => { void updateMaximized().catch(() => {}); }).then(retain).catch(() => {});
    void appWindow.onFocusChanged(({ payload }) => { if (!disposed) setFocused(payload); }).then(retain).catch(() => {});
    return () => { disposed = true; stops.forEach(stop => stop()); };
  }, []);

  useEffect(() => {
    if (platform !== 'windows') return;
    const openMenu = (event: KeyboardEvent) => {
      if ((event.key === 'F10' && !event.shiftKey) || (event.altKey && event.key.toLowerCase() === 'f')) {
        event.preventDefault();
        menuButton.current?.click();
      }
    };
    window.addEventListener('keydown', openMenu);
    return () => window.removeEventListener('keydown', openMenu);
  }, []);

  async function showMenu() {
    onMenuOpen();
    if (!isTauri()) return;
    const bounds = menuButton.current?.getBoundingClientRect();
    if (!bounds) return;
    try { await invoke('show_app_menu', { x: bounds.left, y: bounds.bottom + 4 }); }
    catch (error) { onError(`The app menu could not open. ${String(error)}`); }
  }

  async function control(action: 'minimize' | 'toggleMaximize' | 'close') {
    if (!isTauri()) return;
    try { await getCurrentWindow()[action](); }
    catch (error) { onError(`The window could not ${action === 'toggleMaximize' ? 'resize' : action}. ${String(error)}`); }
  }

  return <header className="window-bar" data-platform={platform} data-focused={focused} data-tauri-drag-region>
    {platform === 'windows' ? <button ref={menuButton} className="app-menu-button" aria-label="Recast menu" aria-haspopup="menu" title="Recast menu (Alt+F)" onClick={() => void showMenu()}>
      <span className="app-name">Recast</span><ChevronDown size={12} strokeWidth={1.7} aria-hidden="true" />
    </button> : <span className="app-name" data-tauri-drag-region>Recast</span>}
    <div className="window-drag-space" data-tauri-drag-region />
    <div className="window-actions">{children}</div>
    {platform === 'windows' && <div className="window-controls" aria-label="Window controls">
      <button className="window-control" aria-label="Minimize" title="Minimize" onClick={() => void control('minimize')}><Minus size={14} strokeWidth={1.4} aria-hidden="true" /></button>
      <button className="window-control" aria-label={maximized ? 'Restore' : 'Maximize'} title={maximized ? 'Restore' : 'Maximize'} onClick={() => void control('toggleMaximize')}>{maximized ? <Copy size={12} strokeWidth={1.4} aria-hidden="true" /> : <Square size={12} strokeWidth={1.4} aria-hidden="true" />}</button>
      <button className="window-control window-close" aria-label="Close window" title="Close" onClick={() => void control('close')}><X size={16} strokeWidth={1.4} aria-hidden="true" /></button>
    </div>}
  </header>;
}
