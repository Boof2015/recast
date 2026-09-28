import { useCallback, useEffect, useRef, useState, type ReactNode, type UIEventHandler } from 'react';
import { ArrowDownToLine, ArrowLeft, ArrowRight, Check, ChevronDown, ChevronRight, File, Film, Folder, Monitor, Moon, Plus, Sun, X } from 'lucide-react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { open } from '@tauri-apps/plugin-dialog';
import { defaultSettings, destinationsFor, formatCounts, kindLabels, kinds, unitLabels, type Appearance, type Destination, type GroupSettings, type InputFile, type MediaKind } from './model';
import { sampleFiles, type Scenario } from './fixtures';
import { CheckboxSetting, QualitySetting, SelectSetting } from './controls';
import { GroupStack } from './GroupStack';
import { useConversion } from './useConversion';

const query = new URLSearchParams(window.location.search);
const initialScenario = query.get('design');
const designMode = import.meta.env.DEV && initialScenario !== null;
const validScenarios: Scenario[] = ['empty', 'mixed', 'images', 'video'];
const startingScenario: Scenario = validScenarios.includes(initialScenario as Scenario) ? initialScenario as Scenario : 'mixed';

function initialSettings() {
  const settings = defaultSettings();
  if (designMode && startingScenario === 'mixed') {
    settings.images.target = 'webp';
    settings.audio.target = 'wav';
  }
  return settings;
}

function ScrollArea({ children, className = '', onScroll }: { children: ReactNode; className?: string; onScroll?: UIEventHandler<HTMLDivElement> }) {
  const ref = useRef<HTMLDivElement>(null);
  const [edges, setEdges] = useState({ top: false, bottom: false });
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const update = () => setEdges({ top: element.scrollTop > 3, bottom: element.scrollHeight - element.scrollTop - element.clientHeight > 3 });
    update();
    const observer = new ResizeObserver(update);
    observer.observe(element);
    if (element.firstElementChild) observer.observe(element.firstElementChild);
    element.addEventListener('scroll', update, { passive: true });
    return () => { observer.disconnect(); element.removeEventListener('scroll', update); };
  }, [children]);
  return <div className={`scroll-area ${className}`} data-top={edges.top} data-bottom={edges.bottom} ref={ref} onScroll={onScroll}>{children}</div>;
}

function FormatSettings({ kind, destination, value, onChange }: { kind: MediaKind; destination: Destination; value: GroupSettings; onChange: (change: Partial<GroupSettings>) => void }) {
  const isImage = kind === 'images';
  const isAudio = kind === 'audio' || destination.category === 'Audio only';
  const isVideo = kind === 'video' && !isAudio;
  const hasQuality = isImage && ['webp', 'jpeg', 'avif'].includes(destination.id);
  const hasLossless = isImage && ['webp', 'avif'].includes(destination.id);
  return <div className="settings-stack">
    {hasQuality && <QualitySetting value={value.quality} disabled={hasLossless && value.lossless} onChange={(quality) => onChange({ quality })} />}
    {hasLossless && <CheckboxSetting label="Lossless" checked={value.lossless} onChange={(lossless) => onChange({ lossless })} />}
    {isImage && <SelectSetting label="Resize" value={value.resize} options={['Original', '75%', '50%', '25%']} onChange={(resize) => onChange({ resize })} />}
    {isAudio && ['mp3', 'm4a', 'opus', 'ogg'].includes(destination.id) && <SelectSetting label="Bitrate" value={value.bitrate} options={['128 kbps', '192 kbps', '256 kbps', '320 kbps']} onChange={(bitrate) => onChange({ bitrate })} />}
    {isAudio && <p className="settings-note">Keep the original sample rate and channels.</p>}
    {isVideo && destination.id !== 'gif' && <>
      <div className="setting-row"><span>Video codec</span><span className="setting-value">{destination.id === 'webm' ? 'VP9' : 'H.264'}</span></div>
      <SelectSetting label="Resolution" value={value.resolution} options={['Original', '1080p', '720p', '480p']} onChange={(resolution) => onChange({ resolution })} />
      <SelectSetting label="Frame rate" value={value.frameRate} options={['Original', '24 fps', '30 fps', '60 fps']} onChange={(frameRate) => onChange({ frameRate })} />
      <div className="setting-row"><span>Audio codec</span><span className="setting-value">{destination.id === 'webm' ? 'Opus' : 'AAC'}</span></div>
    </>}
    {isVideo && destination.id === 'gif' && <div className="range-placeholder"><Film size={24} strokeWidth={1.4} /><p>Preview & source range</p><span>To be explored in the next design pass</span></div>}
    {destination.id !== 'gif' && destination.id !== 'bmp' && <CheckboxSetting label="Keep metadata" checked={value.metadata} onChange={(metadata) => onChange({ metadata })} />}
  </div>;
}

function DestinationBrowser({ formats, onChoose }: { formats: Destination[]; onChoose: (format: string) => void }) {
  const categories = [...new Set(formats.map((format) => format.category))];
  return <div className="destination-browser">
    <h2>Convert to</h2>
    {formats.length === 0 && <p className="settings-note">These files don’t share an available output yet. This build converts PNG and JPEG images to WebP.</p>}
    {categories.map((category) => <section className="destination-category" key={category}>
      <h3>{category}</h3>
      <div className="destination-list">{formats.filter((format) => format.category === category).map((format) => <button key={format.id} className="destination-option" onClick={() => onChoose(format.id)}>
        <span><strong>{format.label}</strong><small>{format.description}</small></span><ArrowRight size={16} />
      </button>)}</div>
    </section>)}
  </div>;
}

export default function App() {
  const [files, setFiles] = useState<InputFile[]>(() => designMode ? sampleFiles(startingScenario) : []);
  const [settings, setSettings] = useState(initialSettings);
  const [selected, setSelected] = useState<MediaKind>(startingScenario === 'video' && designMode ? 'video' : 'images');
  const [expanded, setExpanded] = useState<MediaKind[]>([]);
  const [scenario, setScenario] = useState<Scenario>(startingScenario);
  const [notice, setNotice] = useState('');
  const [dragging, setDragging] = useState(false);
  const [inspecting, setInspecting] = useState(false);
  const inspectionCount = useRef(0);
  const [outputFolder, setOutputFolder] = useState<string | null>(null);
  const conversion = useConversion();
  const { job, busy } = conversion;
  const [appearance, setAppearance] = useState<Appearance>(() => {
    try { const saved = localStorage.getItem('recast.appearance'); return saved === 'light' || saved === 'dark' ? saved : 'system'; }
    catch { return 'system'; }
  });
  const [appearanceOpen, setAppearanceOpen] = useState(false);
  const appearanceRef = useRef<HTMLDivElement>(null);
  const workspaceRef = useRef<HTMLElement>(null);
  const [groupSizes, setGroupSizes] = useState<Partial<Record<MediaKind, number>>>({});
  const updateGroupSizes = useCallback((sizes: Partial<Record<MediaKind, number>>) => {
    setGroupSizes((current) => kinds.every((kind) => current[kind] === sizes[kind]) ? current : sizes);
  }, []);
  const syncGroupScroll: UIEventHandler<HTMLDivElement> = (event) => {
    const source = event.currentTarget;
    workspaceRef.current?.querySelectorAll<HTMLDivElement>('.group-scroll').forEach((element) => {
      if (element !== source && Math.abs(element.scrollTop - source.scrollTop) > 1) element.scrollTop = source.scrollTop;
    });
  };

  const presentKinds = kinds.filter((kind) => files.some((file) => file.kind === kind));
  const previewCatalog = designMode && files.every(file => !file.path);
  const activeKind = presentKinds.includes(selected) ? selected : presentKinds[0] ?? 'images';
  const activeFiles = files.filter((file) => file.kind === activeKind);
  const formats = destinationsFor(activeKind, activeFiles, previewCatalog);
  const activeSettings = settings[activeKind];
  const selectedFormat = formats.find((format) => format.id === activeSettings.target);
  const unresolvedTarget = activeSettings.target && !selectedFormat;
  const affectedFiles = unresolvedTarget ? activeFiles.filter(file => previewCatalog ? activeKind === 'video' && file.hasAudio !== true : !file.targets?.includes(activeSettings.target!)) : [];
  const allConfigured = presentKinds.length > 0 && presentKinds.every((kind) => destinationsFor(kind, files.filter((file) => file.kind === kind), previewCatalog).some((format) => format.id === settings[kind].target));
  const folderName = outputFolder?.split(/[\\/]/).filter(Boolean).at(-1) ?? 'Same as source';
  const succeeded = job?.files.filter(file => file.status === 'succeeded') ?? [];
  const failed = job?.files.filter(file => file.status === 'failed') ?? [];
  const retryCount = job ? job.files.length - succeeded.length : 0;
  const currentFile = job?.files.find(file => file.status === 'running');
  const canConvert = conversion.backendReady && !previewCatalog && allConfigured && !inspecting && !busy && files.every(file => file.kind === 'images' && file.path) && settings.images.target === 'webp';
  const jobText = conversion.starting ? 'Preparing conversion…' : job?.status === 'cancelling' ? 'Cancelling…' : busy && job ? `Converting ${Math.min(succeeded.length + failed.length + 1, job.files.length)} of ${job.files.length}` : job ? `${succeeded.length} converted${failed.length ? ` · ${failed.length} failed` : ''}${job.status === 'cancelled' ? ' · Cancelled' : ''}` : '';

  useEffect(() => {
    if (job && !busy && job.files.some(file => file.status === 'failed')) setExpanded(current => current.includes('images') ? current : [...current, 'images']);
  }, [job, busy]);

  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    const apply = () => { document.documentElement.dataset.theme = appearance === 'system' ? media.matches ? 'dark' : 'light' : appearance; };
    apply();
    media.addEventListener('change', apply);
    try { localStorage.setItem('recast.appearance', appearance); } catch { /* Session-only appearance is fine. */ }
    return () => media.removeEventListener('change', apply);
  }, [appearance]);

  useEffect(() => {
    if (!appearanceOpen) return;
    const dismiss = (event: PointerEvent) => { if (!appearanceRef.current?.contains(event.target as Node)) setAppearanceOpen(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') setAppearanceOpen(false); };
    document.addEventListener('pointerdown', dismiss);
    document.addEventListener('keydown', escape);
    return () => { document.removeEventListener('pointerdown', dismiss); document.removeEventListener('keydown', escape); };
  }, [appearanceOpen]);

  const addPaths = useCallback(async (paths: string[]) => {
    if (!paths.length || conversion.busyRef.current) return;
    inspectionCount.current++;
    setInspecting(true);
    try {
      const result = await invoke<{ files: InputFile[]; errors: { name: string; message: string }[] }>('inspect_inputs', { paths });
      conversion.clear();
      setFiles((current) => {
        const realFiles = current.filter(file => file.path);
        const seen = new Set(realFiles.map((file) => file.id));
        return [...realFiles, ...result.files.filter((file) => !seen.has(file.id) && seen.add(file.id))];
      });
      if (result.errors.length) setNotice(result.errors.map((error) => `${error.name}: ${error.message}`).join(' '));
      else setNotice('');
    } catch (error) { setNotice(`Files could not be added. ${String(error)}`); }
    finally { inspectionCount.current--; setInspecting(inspectionCount.current > 0); }
  }, []);

  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let stop: (() => void) | undefined;
    void getCurrentWebview().onDragDropEvent((event) => {
      if (conversion.busyRef.current) { setDragging(false); return; }
      if (event.payload.type === 'over' || event.payload.type === 'enter') setDragging(true);
      if (event.payload.type === 'leave') setDragging(false);
      if (event.payload.type === 'drop') { setDragging(false); void addPaths(event.payload.paths); }
    }).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten; }).catch(() => setNotice('Drag and drop is unavailable. Use Choose files instead.'));
    return () => { disposed = true; stop?.(); };
  }, [addPaths]);

  async function chooseFiles() {
    if (conversion.busyRef.current) return;
    if (!isTauri()) { setNotice('File selection is available in the desktop app. Use the sample batches below to explore this browser preview.'); return; }
    try {
      const paths = await open({ multiple: true, directory: false, title: 'Choose files to convert' });
      if (paths) await addPaths(Array.isArray(paths) ? paths : [paths]);
    } catch { setNotice('The file picker could not open. Try dropping files into the window.'); }
  }

  async function chooseFolder() {
    if (conversion.busyRef.current) return;
    if (!isTauri()) { setNotice('Destination folder selection is available in the desktop app.'); return; }
    try { const folder = await open({ directory: true, multiple: false, title: 'Choose an output folder', defaultPath: outputFolder ?? undefined }); if (typeof folder === 'string') { conversion.clear(); setOutputFolder(folder); } }
    catch { setNotice('The folder picker could not open.'); }
  }

  function changeScenario(next: Scenario) {
    if (conversion.busyRef.current) return;
    conversion.clear();
    setScenario(next); setFiles(sampleFiles(next)); setExpanded([]); setNotice(''); setOutputFolder(null);
    const fresh = defaultSettings();
    if (next === 'mixed') { fresh.images.target = 'webp'; fresh.audio.target = 'wav'; }
    setSettings(fresh); setSelected(next === 'video' ? 'video' : 'images');
  }

  function updateSettings(change: Partial<GroupSettings>) {
    if (conversion.busyRef.current) return;
    conversion.clear();
    setSettings((current) => ({ ...current, [activeKind]: { ...current[activeKind], ...change } }));
  }

  function newBatch() {
    if (conversion.busyRef.current) return;
    conversion.clear(); setFiles([]); setSettings(defaultSettings()); setExpanded([]); setOutputFolder(null); setNotice('');
  }

  async function runConversion(retry = false) {
    if ((!canConvert && !retry) || conversion.busyRef.current || inspectionCount.current > 0) return;
    setNotice('');
    try { await conversion.start(files, settings.images, outputFolder, retry); }
    catch (error) { setNotice(String(error)); }
  }

  async function revealOutput(path: string) {
    try { await invoke('reveal_output', { path }); }
    catch (error) { setNotice(String(error)); }
  }

  const AppearanceIcon = appearance === 'dark' ? Moon : appearance === 'light' ? Sun : Monitor;
  return <div className={`app-shell ${designMode ? 'with-design-tools' : ''}`}>
    <header className="window-bar" data-tauri-drag-region>
      <span className="app-name" data-tauri-drag-region>Recast</span>
      <div className="window-actions">
        <div className="appearance-control" ref={appearanceRef}>
          <button className="icon-button" aria-label="Appearance" aria-expanded={appearanceOpen} onClick={() => setAppearanceOpen(!appearanceOpen)}><AppearanceIcon size={17} strokeWidth={1.7} /></button>
          {appearanceOpen && <div className="appearance-menu" aria-label="Appearance choices">
            {(['system', 'light', 'dark'] as Appearance[]).map((mode) => <button key={mode} onClick={() => { setAppearance(mode); setAppearanceOpen(false); }}>{mode === 'system' ? <Monitor size={15} /> : mode === 'light' ? <Sun size={15} /> : <Moon size={15} />}<span>{mode[0].toUpperCase() + mode.slice(1)}</span>{appearance === mode && <Check size={14} />}</button>)}
          </div>}
        </div>
        {files.length > 0 && <button className="quiet-button add-files" onClick={() => void chooseFiles()} disabled={inspecting || busy}><Plus size={16} />{inspecting ? 'Adding…' : 'Add files'}</button>}
      </div>
    </header>

    {notice && <div className="notice" role="status"><span>{notice}</span><button className="icon-button" aria-label="Dismiss message" onClick={() => setNotice('')}><X size={15} /></button></div>}
    {conversion.backendError && <div className="notice" role="alert">{conversion.backendError}</div>}

    {files.length === 0 ? <main className="empty-state">
      <div className="empty-symbol" aria-hidden="true"><File className="file-back" size={49} strokeWidth={1.15} /><File className="file-front" size={49} strokeWidth={1.15} /><ArrowDownToLine className="file-arrow" size={18} strokeWidth={1.6} /></div>
      <h1>Drop files here</h1>
      <p>{previewCatalog ? 'Images, audio, or video. All on your device.' : 'PNG and JPEG to WebP. All on your device.'}</p>
      <button className="primary-button choose-files" onClick={() => void chooseFiles()} disabled={inspecting}>{inspecting ? 'Reading files…' : 'Choose files'}<Plus size={16} /></button>
    </main> : <>
      <main className="workspace" ref={workspaceRef}>
        <section className="input-region" aria-label="Input files">
          <div className="mass input-mass">
            <div className="region-heading"><h1>Input</h1></div>
            <ScrollArea className="group-scroll" onScroll={syncGroupScroll}>
            <GroupStack selected={activeKind} onSizes={updateGroupSizes}>{presentKinds.map((kind) => {
              const groupFiles = files.filter((file) => file.kind === kind);
              const isExpanded = expanded.includes(kind);
              return <section key={kind} data-group-slot={kind} className={`input-group ${activeKind === kind ? 'selected' : ''}`}>
                <div className="group-header" data-selection-key={kind}>
                  <button className="group-select" aria-label={kindLabels[kind]} aria-pressed={activeKind === kind} onClick={() => setSelected(kind)}>
                    <span className="group-title">{kindLabels[kind]}</span>
                    <span className="format-counts">{formatCounts(groupFiles).map(([format, count]) => <span key={format}>{count} {format}</span>)}</span>
                  </button>
                  <button className="file-count" aria-expanded={isExpanded} aria-controls={isExpanded ? `files-${kind}` : undefined} onClick={() => setExpanded((current) => isExpanded ? current.filter((item) => item !== kind) : [...current, kind])}>{groupFiles.length} files<ChevronDown size={12} className={isExpanded ? 'rotated' : ''} /></button>
                </div>
                {isExpanded && <ScrollArea className="file-list-scroll"><div className="file-list" id={`files-${kind}`}>{groupFiles.map((file) => {
                  const result = job?.files.find(result => result.id === file.id);
                  const detail = result?.error ?? file.conversionIssue ?? (result ? ({ pending: 'Waiting', running: 'Converting…', succeeded: 'Converted', failed: 'Failed', cancelled: 'Cancelled' }[result.status]) : '');
                  return <div className="file-row" key={file.id}><div className="file-detail"><span title={file.path || file.name}>{file.name}</span>{detail && <span className="file-status">{detail}</span>}</div><button className="remove-file" disabled={busy} aria-label={`Remove ${file.name}`} onClick={() => { conversion.clear(); setFiles((current) => current.filter((item) => item.id !== file.id)); }}><X size={13} /></button></div>;
                })}</div></ScrollArea>}
              </section>;
            })}</GroupStack>
          </ScrollArea></div>
        </section>

        <section className="conversion-region" aria-label="Conversion settings">
          <div className="conversion-heading">{activeFiles.length} {unitLabels[activeKind]}</div>
          <ScrollArea className="conversion-scroll"><div className="conversion-content" key={activeKind}>
            {unresolvedTarget && <div className="compatibility-message" role="status"><p>{affectedFiles.length === 1 ? affectedFiles[0].name : `${affectedFiles.length} files`} cannot be converted to {activeSettings.target?.toUpperCase()}.</p><p className="compatibility-reason">{previewCatalog ? affectedFiles.every((file) => file.hasAudio === false) ? 'No audio track was found.' : 'An audio track has not been confirmed for every file.' : affectedFiles.length === 1 ? affectedFiles[0].conversionIssue : 'Not every file supports this output.'}</p><span>Remove {affectedFiles.length === 1 ? 'it' : 'them'} to keep this format{formats.length ? ', or choose another for the whole group.' : '.'}</span><button className="text-button" onClick={() => { setSelected(activeKind); setExpanded((current) => current.includes(activeKind) ? current : [...current, activeKind]); }}>Show files<ChevronRight size={13} /></button></div>}
            <fieldset className="conversion-fields" disabled={busy}>
            {selectedFormat ? <>
              <button className="format-back" onClick={() => updateSettings({ target: null })} aria-label="Change output format"><ArrowLeft size={18} strokeWidth={1.7} /><h2>{selectedFormat.label}</h2></button>
              <FormatSettings kind={activeKind} destination={selectedFormat} value={activeSettings} onChange={updateSettings} />
            </> : <DestinationBrowser formats={formats} onChoose={(target) => updateSettings({ target })} />}
            </fieldset>
            {!unresolvedTarget && !formats.length && !previewCatalog && <button className="text-button" onClick={() => setExpanded(current => current.includes(activeKind) ? current : [...current, activeKind])}>Show files<ChevronRight size={13} /></button>}
          </div></ScrollArea>
        </section>

        <section className="output-region" aria-label="Output summary">
          <div className="mass output-mass">
            <div className="region-heading"><h1>Output</h1></div>
            <ScrollArea className="group-scroll" onScroll={syncGroupScroll}><GroupStack selected={activeKind}>{presentKinds.map((kind) => {
            const groupFiles = files.filter((file) => file.kind === kind);
            const target = destinationsFor(kind, groupFiles, previewCatalog).find((format) => format.id === settings[kind].target);
            return <section key={kind} className="output-group" style={{ minHeight: groupSizes[kind] }}>
              <button className="group-header output-select" data-selection-key={kind} aria-label={`${kindLabels[kind]} output`} aria-pressed={activeKind === kind} onClick={() => setSelected(kind)}>
                <span className={`output-result ${target ? '' : 'pending-output'}`}>{groupFiles.length} {target?.label ?? 'files'}</span>
                <span className="output-detail">{job && kind === 'images' ? jobText : target ? outputFolder ? folderName : 'Same folder as source' : 'Choose an output format'}</span>
              </button>
              {kind === 'images' && succeeded.length > 0 && <div className="output-files" aria-label="Converted files">{succeeded.map(file => <button className="output-file" key={file.id} title={file.outputPath!} aria-label={`Show ${file.outputPath!.split(/[\\/]/).at(-1)} in folder`} onClick={() => void revealOutput(file.outputPath!)}><Check size={13} aria-hidden="true" /><span>{file.outputPath!.split(/[\\/]/).at(-1)}</span><Folder size={13} aria-hidden="true" /></button>)}</div>}
            </section>;
          })}</GroupStack>
            </ScrollArea></div>
        </section>
      </main>

      <footer className="batch-footer">
        <div className="batch-total"><span role="status" aria-live="polite">{job || conversion.starting ? jobText : `${files.length} files`}</span>{busy && job && <progress aria-label="Files processed" max={job.files.length} value={succeeded.length + failed.length} />}{currentFile && <span className="current-file" title={currentFile.name}>{currentFile.name}</span>}</div>
        {!busy && job && retryCount > 0 && <button className="quiet-button" onClick={newBatch}>New batch</button>}
        <div className="folder-choice"><button className="folder-button" disabled={busy} onClick={() => void chooseFolder()} title={outputFolder ?? 'Choose an output folder'}><Folder size={16} strokeWidth={1.6} /><span>{folderName}</span><ChevronDown size={12} /></button>{outputFolder && <button className="icon-button" disabled={busy} aria-label="Use source folders" title="Use source folders" onClick={() => { conversion.clear(); setOutputFolder(null); }}><X size={12} /></button>}</div>
        {busy ? <button className="primary-button" disabled={conversion.starting || job?.status === 'cancelling'} onClick={() => void conversion.cancel().catch(error => setNotice(String(error)))}>{job?.status === 'cancelling' ? 'Cancelling…' : 'Cancel'}</button> : job ? retryCount > 0 ? <button className="primary-button" onClick={() => void runConversion(true)}>Retry {retryCount} {retryCount === 1 ? 'file' : 'files'}</button> : <button className="primary-button" onClick={newBatch}>New batch</button> : <button className={`primary-button convert-button ${allConfigured ? 'configured' : ''}`} disabled={!canConvert} onClick={() => void runConversion()} title={previewCatalog ? 'Sample files are for design review. Add real PNG or JPEG files to convert.' : !allConfigured ? 'Choose a supported output for every group.' : undefined}>Convert {files.length} {files.length === 1 ? 'file' : 'files'}</button>}
      </footer>
    </>}

    {dragging && <div className="drop-overlay"><ArrowDownToLine size={27} /><span>Add to this batch</span></div>}

    {designMode && <aside className="design-tools" aria-label="Design preview controls"><span className="design-label">Design preview<span className="subtle-dot">·</span>{previewCatalog ? 'Sample files' : 'Real files'}</span><div className="scenario-control"><label htmlFor="scenario">View</label><select id="scenario" disabled={busy} value={scenario} onChange={(event) => changeScenario(event.target.value as Scenario)}><option value="empty">Empty</option><option value="mixed">Mixed batch</option><option value="images">Images</option><option value="video">Video</option></select></div>{scenario === 'video' && <button className="design-action" disabled={busy || files.some((file) => file.id === 'sample-silent')} onClick={() => setFiles((current) => [...current, { id: 'sample-silent', name: 'Screen recording.mp4', path: '', kind: 'video', format: 'MP4', bytes: 24_000_000, hasAudio: false }])}>Add silent video</button>}<span className="design-status">{previewCatalog ? 'Sample conversion disabled' : 'PNG / JPEG → WebP'}</span></aside>}
  </div>;
}
