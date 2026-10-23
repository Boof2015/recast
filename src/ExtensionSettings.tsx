import { useEffect, useRef, useState } from 'react';
import { ArrowLeft, Download, FolderOpen, Package, RefreshCw, X } from 'lucide-react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open } from '@tauri-apps/plugin-dialog';
import { formatBytes } from './model';
import { ActivityBar } from './ActivityBar';

type PackageInfo = {
  id: string; name: string; version: string; description: string; publisher: string;
  platform: string; minimumAppVersion: string; installedBytes: number;
};
type Installed = { key: string; package: PackageInfo; source: string };
type Offer = { package: PackageInfo; sha256: string; downloadBytes: number; url: string };
type LocalReview = { package: PackageInfo; sha256: string; downloadBytes: number; path: string };

export function ExtensionSettings({ onClose }: { onClose: () => void }) {
  const [installed, setInstalled] = useState<Installed[]>([]);
  const [available, setAvailable] = useState<Offer[] | null>(null);
  const [review, setReview] = useState<LocalReview | null>(null);
  const [removing, setRemoving] = useState<string | null>(null);
  const [working, setWorking] = useState('');
  const [error, setError] = useState('');
  const [message, setMessage] = useState('');
  const [cancelling, setCancelling] = useState(false);
  const active = useRef(false);
  const alive = useRef(true);
  const back = useRef<HTMLButtonElement>(null);

  async function loadInstalled() {
    if (isTauri()) {
      const list = await invoke<Installed[]>('extensions_list');
      if (alive.current) setInstalled(list);
    }
  }
  useEffect(() => {
    alive.current = true;
    back.current?.focus();
    void loadInstalled().catch(error => { if (alive.current) setError(String(error)); });
    return () => { alive.current = false; };
  }, []);
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let stop: (() => void) | undefined;
    // Another window can finish an install while this page is open.
    void getCurrentWindow().listen('recast:extensions-changed', () => {
      if (!active.current) void loadInstalled().catch(error => setError(String(error)));
    }).then(unlisten => { if (disposed) unlisten(); else stop = unlisten; });
    return () => { disposed = true; stop?.(); };
  }, []);
  useEffect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || active.current) return;
      if (review) setReview(null);
      else if (removing) setRemoving(null);
      else onClose();
    };
    document.addEventListener('keydown', escape);
    return () => document.removeEventListener('keydown', escape);
  }, [onClose, review, removing]);

  async function run(label: string, action: () => Promise<void>) {
    if (active.current) return;
    if (!isTauri()) { setError('Extension installation is available in the desktop app.'); return; }
    active.current = true; setWorking(label); setError(''); setMessage(''); setCancelling(false);
    try { await action(); }
    catch (error) { if (alive.current) setError(String(error) === 'Cancelled.' ? '' : String(error)); }
    finally { active.current = false; if (alive.current) { setWorking(''); setCancelling(false); } }
  }
  async function choosePackage() {
    await run('Reading package…', async () => {
      const path = await open({ directory: false, multiple: false, title: 'Install a Recast extension', filters: [{ name: 'Recast extension', extensions: ['recast-extension'] }] });
      if (typeof path !== 'string') return;
      const details = await invoke<Omit<LocalReview, 'path'>>('extensions_inspect', { path });
      setReview({ ...details, path }); setRemoving(null);
    });
  }
  async function installLocal() {
    if (!review) return;
    await run('Installing…', async () => {
      await invoke('extensions_install_local', { path: review.path, sha256: review.sha256 });
      setMessage(`${review.package.name} installed.`); setReview(null); await loadInstalled();
    });
  }
  async function install(offer: Offer) {
    await run(`Installing ${offer.package.name}…`, async () => {
      await invoke('extensions_install', { sha256: offer.sha256 });
      setMessage(`${offer.package.name} installed.`); await loadInstalled();
    });
  }
  const visibleOffers = available?.filter(offer => !installed.some(item => item.key === offer.sha256));
  return <main className="extension-settings" aria-label="Settings">
    <nav className="settings-navigation" aria-label="Settings sections">
      <button ref={back} className="quiet-button settings-back" onClick={onClose} disabled={!!working}><ArrowLeft size={16} />Back to batch</button>
      <h1>Settings</h1>
      <div className="settings-section"><Package size={17} />Extensions</div>
    </nav>
    <section className="extensions-content">
      <header className="extensions-heading"><div><h2>Extensions</h2><p>Add more conversion formats.</p></div><button className="quiet-button" onClick={() => void choosePackage()} disabled={!!working}><FolderOpen size={15} />Install from file</button></header>
      {error && <div className="extension-feedback" role="alert"><span>{error}</span><button className="icon-button" aria-label="Dismiss error" onClick={() => setError('')}><X size={14} /></button></div>}
      <div role="status" className="extension-status">{working ? <><div className="extension-operation"><span>{cancelling ? 'Cancelling…' : working}</span><ActivityBar label={cancelling ? 'Cancelling…' : working} /></div><button className="quiet-button" disabled={cancelling} onClick={() => { setCancelling(true); void invoke('extensions_cancel').catch(error => setError(String(error))); }}>Cancel</button></> : message}</div>
      <div className="extensions-scroll">
        {review && <section className="extension-review" aria-label="Review local extension">
          <div className="extension-title"><h3>{review.package.name}</h3><span>{review.package.version}</span></div>
          <p>{review.package.description}</p>
          <p className="extension-meta">{review.package.publisher} · Local file · {formatBytes(review.package.installedBytes)} when installed</p>
          <p className="extension-trust">Local publisher identity isn’t verified. Only install packages you trust.</p>
          <div className="extension-actions"><button className="quiet-button" disabled={!!working} onClick={() => setReview(null)}>Cancel</button><button className="primary-button" disabled={!!working} onClick={() => void installLocal()}>Install</button></div>
        </section>}
        <section className="extension-group" aria-label="Installed extensions">
          <h3>Installed</h3>
          {installed.length === 0 ? <p className="extension-empty">No extensions installed.</p> : installed.map(item => <article className="extension-row" key={item.key}>
            <div className="extension-details"><div className="extension-title"><h4>{item.package.name}</h4><span>{item.package.version}</span></div><p>{item.package.description}</p><p className="extension-meta">{item.source === 'official' ? 'Recast' : `${item.package.publisher} · Local file`} · {formatBytes(item.package.installedBytes)}</p></div>
            {removing === item.key ? <div className="extension-actions"><button className="quiet-button" disabled={!!working} onClick={() => setRemoving(null)}>Keep</button><button className="quiet-button" disabled={!!working} onClick={() => void run('Removing…', async () => { await invoke('extensions_remove', { key: item.key }); setRemoving(null); await loadInstalled(); setMessage(`${item.package.name} removed.`); })}>Confirm removal</button></div> : <button className="quiet-button" disabled={!!working} onClick={() => { setRemoving(item.key); setReview(null); }}>Remove</button>}
          </article>)}
        </section>
        <section className="extension-group" aria-label="Available extensions">
          <header className="extension-group-heading"><h3>Available</h3><button className="quiet-button" disabled={!!working} onClick={() => void run('Checking for extensions…', async () => { setAvailable(await invoke<Offer[]>('extensions_catalog')); })}><RefreshCw size={13} />Check for extensions</button></header>
          {available === null ? <p className="extension-empty">Check for published extensions from Recast.</p> : visibleOffers?.length === 0 ? <p className="extension-empty">{installed.length ? 'No additional extensions available for this device.' : 'No extensions published for this device yet.'}</p> : visibleOffers?.map(offer => <article className="extension-row" key={offer.sha256}>
            <div className="extension-details"><div className="extension-title"><h4>{offer.package.name}</h4><span>{offer.package.version}</span></div><p>{offer.package.description}</p><p className="extension-meta">{formatBytes(offer.downloadBytes)} download · {formatBytes(offer.package.installedBytes)} when installed</p></div>
            <button className="quiet-button" disabled={!!working} onClick={() => void install(offer)}><Download size={14} />Install</button>
          </article>)}
        </section>
      </div>
    </section>
  </main>;
}
