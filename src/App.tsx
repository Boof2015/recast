import { useCallback, useEffect, useRef, useState, type ReactNode, type UIEventHandler } from 'react';
import { ArrowDownToLine, ArrowLeft, ArrowRight, Check, ChevronDown, ChevronRight, File, Folder, Monitor, Moon, Plus, Settings, Sun, X } from 'lucide-react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open } from '@tauri-apps/plugin-dialog';
import { extensionValues, isVideoAnimationTarget, isImageFrameTarget, isFrameTarget, isAudioTarget, selectAudioTrack, videoModeKey, formatBytes, videoSettingIssue, vorbisQualities, opusBitrateChoices, aacBitrateChoices, aacRates, isLossyAudio, audioDescription, videoDescription, audioSampleFormats, audioSettingIssue, audioWillDither, defaultSettings, destinationsFor, formatCounts, kindLabels, kinds, unitLabels, mp3BitrateChoices, mp3Qualities, mp3Rates, type Appearance, type Destination, type GroupSettings, type InputFile, type MediaKind } from './model';
import { sampleFiles, type Scenario } from './fixtures';
import { CheckboxSetting, QualitySetting, SelectSetting, SizeSetting } from './controls';
import { VideoRange } from './VideoRange';
import { FramePicker } from './FramePicker';
import { ExtensionSettings } from './ExtensionSettings';
import { ColorSetting } from './ColorSetting';
import { GroupStack } from './GroupStack';
import { useConversion, type FileResult } from './useConversion';

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

function ConversionResult({ result }: { result: FileResult }) {
  const summary = `Converted${result.outputBytes != null ? ` · ${formatBytes(result.outputBytes)}` : ''}`;
  return <>
    {result.details ? <details className="file-result file-status">
      <summary aria-label={`${summary}. Conversion details for ${result.name}`}><span>{summary}</span><ChevronDown size={12} aria-hidden="true" /></summary>
      <p className="file-result-info">{result.details}</p>
    </details> : <span className="file-status">{summary}</span>}
    {result.warnings?.map((warning, index) => <span className="file-status" key={index}>{warning}</span>)}
  </>;
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

function FormatSettings({ kind, destination, value, onChange, disabled, animated, files }: { kind: MediaKind; destination: Destination; value: GroupSettings; onChange: (change: Partial<GroupSettings>) => void; disabled: boolean; animated: boolean; files: InputFile[] }) {
  if (destination.providerKey) {
    const values = extensionValues(destination, value);
    return <div className="settings-stack">{destination.settings?.map(setting => <SelectSetting key={setting.id} label={setting.label} value={values[setting.id]} options={setting.choices} onChange={next => onChange({ extensionSettings: { ...value.extensionSettings, [destination.id]: { ...values, [setting.id]: next } } })} />)}</div>;
  }
  const target = destination.id.replace(/^frame-/, '');
  const isFrame = kind === 'video' && destination.category === 'Still image';
  const isAnimation = kind === 'video' && destination.category === 'Animation';
  const isWebpAnimation = isAnimation && target === 'animated-webp';
  const isImage = kind === 'images' || isFrame;
  const isAudio = kind === 'audio' || destination.category === 'Audio only';
  const isVideo = kind === 'video' && !isAudio;
  const videoOutput = isVideo && ['mp4', 'mkv', 'mov', 'webm'].includes(target);
  const modeKey = videoModeKey(target);
  const mode = value[modeKey];
  const targetSize = videoOutput && mode === 'Target size';
  const copyStreams = videoOutput && mode === 'Keep original';
  const hasQuality = isWebpAnimation || (isImage && ['webp', 'jpeg', 'avif'].includes(target)) || (videoOutput && mode === 'Quality');
  const hasLossless = isWebpAnimation || (isImage && ['webp', 'avif'].includes(target));
  const lossyInput = files.some(isLossyAudio);
  const sampleFormatLabel = (label: string) => lossyInput && label === 'Keep source' ? 'Auto' : label;
  return <div className="settings-stack">
    {videoOutput && <SelectSetting label="Mode" value={mode} options={[...(['mkv', 'mov', 'webm'].includes(target) ? ['Keep original'] : []), 'Quality', 'Target size']} onChange={mode => onChange({ [modeKey]: mode })} />}
    {copyStreams && <p className="settings-note">Copies the original tracks without re-encoding.</p>}
    {targetSize && <><SizeSetting value={value.targetSize} onChange={targetSize => onChange({ targetSize })} /><p className="settings-note">Limit applies to each file.</p></>}
    {hasQuality && <QualitySetting value={value.quality} disabled={hasLossless && value.lossless} onChange={(quality) => onChange({ quality })} />}
    {hasLossless && <CheckboxSetting label="Lossless" checked={value.lossless} onChange={(lossless) => onChange({ lossless })} />}
    {isImage && target === 'gif' && <>
      <SelectSetting label="Colors" value={String(value.colors)} options={['256', '128', '64', '32']} onChange={(colors) => onChange({ colors: Number(colors) })} />
      <CheckboxSetting label="Dithering" checked={value.dither} onChange={(dither) => onChange({ dither })} />
    </>}
    {isImage && <SelectSetting label="Resize" value={value.resize} options={['Original', '75%', '50%', '25%']} onChange={(resize) => onChange({ resize })} />}
    {isImage && !isFrame && ['jpeg', 'bmp'].includes(target) && <ColorSetting label="Background" value={value.background} disabled={disabled} onChange={(background) => onChange({ background })} />}
    {isAudio && target === 'mp3' && <>
      <SelectSetting label="Encoding" value={value.mp3Mode === 'vbr' ? 'Variable bitrate' : 'Constant bitrate'} options={['Variable bitrate', 'Constant bitrate']} onChange={(mode) => onChange({ mp3Mode: mode === 'Variable bitrate' ? 'vbr' : 'cbr' })} />
      {value.mp3Mode === 'vbr' ? <SelectSetting label="Quality" value={value.mp3Quality} options={mp3Qualities} onChange={(mp3Quality) => onChange({ mp3Quality })} /> : <SelectSetting label="Bitrate" value={value.bitrate} options={mp3BitrateChoices(files, value)} onChange={(bitrate) => onChange({ bitrate })} />}
    </>}
    {isAudio && target === 'm4a' && <SelectSetting label="Bitrate" value={value.aacBitrate} options={aacBitrateChoices(files, value)} onChange={(aacBitrate) => onChange({ aacBitrate })} />}
    {isAudio && target === 'opus' && <SelectSetting label="Bitrate" value={value.opusBitrate} options={opusBitrateChoices(files)} onChange={(opusBitrate) => onChange({ opusBitrate })} />}
    {isAudio && target === 'ogg' && <SelectSetting label="Quality" value={value.vorbisQuality} options={vorbisQualities} onChange={(vorbisQuality) => onChange({ vorbisQuality })} />}
    {isAudio && <>
      {target !== 'opus' && <SelectSetting label="Sample rate" value={value.sampleRate} options={['Keep source', ...(target === 'mp3' ? mp3Rates : target === 'm4a' ? aacRates : ['7350', ...mp3Rates, '64000', '88200', '96000', '176400', '192000'])]} onChange={(sampleRate) => onChange({ sampleRate })} />}
      {['wav', 'flac', 'alac'].includes(target) && <>
        <SelectSetting label="Sample format" value={sampleFormatLabel(value.sampleFormat)} options={Object.keys(audioSampleFormats).filter(label => target === 'wav' || ['Keep source', '16-bit PCM', '24-bit PCM'].includes(label)).map(sampleFormatLabel)} onChange={(sampleFormat) => onChange({ sampleFormat: sampleFormat === 'Auto' ? 'Keep source' : sampleFormat })} />
        {lossyInput && value.sampleFormat === 'Keep source' && <p className="settings-note">Auto uses {target === 'wav' ? '32-bit float' : '24-bit PCM'} for lossy inputs and keeps other files’ source formats.</p>}
        <SelectSetting label="Dithering" value={value.audioDither ? 'Auto' : 'Off'} options={['Auto', 'Off']} onChange={(dither) => onChange({ audioDither: dither === 'Auto' })} />
        <CheckboxSetting label="Allow clipping" checked={value.allowClipping} onChange={(allowClipping) => onChange({ allowClipping })} />
        {value.allowClipping && <p className="settings-note">Peaks outside the integer output range may be clipped.</p>}
        {files.some(file => file.audio) && <p className="settings-note">{value.audioDither ? `Dither applies to ${files.filter(file => audioWillDither(file, value)).length} of ${files.length} files when quantizing to integer samples.` : 'Dithering is off.'}</p>}
      </>}
      <p className="settings-note">{target === 'opus' ? '48,000 Hz · resamples when needed. Keeps each file’s original channels.' : 'Keeps each file’s original channels. Sample rate is in Hz.'}</p>
    </>}
    {videoOutput && !copyStreams && <>
      <SelectSetting label="Encoding" value={value.acceleration} options={['Automatic', 'Software']} onChange={(acceleration) => onChange({ acceleration: acceleration as GroupSettings['acceleration'] })} />
      <div className="setting-row"><span>Video codec</span><span className="setting-value">{target === 'webm' ? 'VP9' : 'H.264'}</span></div>
      <SelectSetting label="Resolution" value={targetSize ? value.sizeResolution : value.resolution} options={[...(targetSize ? ['Auto'] : []), 'Original', '1080p', '720p', '480p']} onChange={(resolution) => onChange(targetSize ? { sizeResolution: resolution } : { resolution })} />
      <SelectSetting label="Frame rate" value={value.frameRate} options={['Original', '24 fps', '30 fps', '60 fps']} onChange={(frameRate) => onChange({ frameRate })} />
      <div className="setting-row"><span>Audio codec</span><span className="setting-value">{target === 'webm' ? targetSize ? 'Opus' : 'Original / Opus' : target === 'mkv' && !targetSize ? 'Keep original' : target === 'mov' && !targetSize ? 'Original / AAC' : 'AAC'}</span></div>
    </>}
    {videoOutput && target === 'webm' && !copyStreams && files.some(file => file.hasAudio) && <p className="settings-note">Opus encodes at 48,000 Hz · resamples when needed.</p>}
    {targetSize && <p className="settings-note">Size is checked before saving. Audio may be re-encoded.</p>}
    {isVideo && target === 'mp4' && !targetSize && <p className="settings-note">Automatic uses available hardware, with software fallback. Quality and file size vary by encoder. Keeps original timing unless a frame rate is selected. Resolution fits without upscaling. AAC audio is kept; other supported audio is encoded to AAC.</p>}
    {isAnimation && <>
      <SelectSetting label="Size" value={value.animationSize} options={['320 px', '480 px', '720 px', '960 px']} onChange={animationSize => onChange({ animationSize })} />
      <SelectSetting label="Frame rate" value={value.animationFrameRate} options={['10 fps', '15 fps', '20 fps', '25 fps', '30 fps']} onChange={animationFrameRate => onChange({ animationFrameRate })} />
      {!isWebpAnimation && <><SelectSetting label="Colors" value={String(value.colors)} options={['256', '128', '64', '32']} onChange={colors => onChange({ colors: Number(colors) })} />
      <CheckboxSetting label="Dithering" checked={value.dither} onChange={dither => onChange({ dither })} /></>}
      <p className="settings-note">Loops continuously · No audio</p>
    </>}
    {!isFrame && !isAnimation && target !== 'gif' && target !== 'bmp' && <CheckboxSetting label="Keep metadata" checked={value.metadata} onChange={(metadata) => onChange({ metadata })} />}
    {isAudio && value.metadata && <p className="settings-note">Keeps supported text tags. Artwork, loudness tags, and production metadata are omitted.</p>}
    {isAudio && target === 'alac' && <p className="settings-note">Apple Lossless audio in an .m4a file.</p>}
    {isAudio && target === 'mp3' && <p className="settings-note">{value.mp3Mode === 'vbr' ? 'Bitrate varies with the audio to meet the selected quality.' : 'Bitrate stays fixed; available rates depend on sample rate.'} MP3 encoding is lossy.</p>}
    {isAudio && target === 'm4a' && <p className="settings-note">AAC-LC audio in an .m4a file. Bitrate is a target for the whole stream; actual bitrate varies with the audio.</p>}
    {isAudio && target === 'opus' && <p className="settings-note">Opus audio in an .opus file. Variable bitrate targets the whole stream; actual bitrate varies with the audio.</p>}
    {isAudio && target === 'ogg' && <p className="settings-note">Vorbis audio in an .ogg file. Higher quality makes larger files; bitrate varies with the audio, sample rate, and channels.</p>}
    {isAudio && lossyInput && <p className="settings-note">{['mp3', 'm4a', 'opus', 'ogg'].includes(target) ? 'Re-encoding lossy audio adds another lossy generation.' : 'Converting lossy audio to PCM or a lossless format does not restore lost detail.'}</p>}
    {isImage && !isFrame && target === 'tiff' && value.metadata && <p className="settings-note">Keeps color profiles and supported tags. Camera EXIF may not carry over.</p>}
    {isImage && target === 'gif' && <p className="settings-note">Partial transparency becomes clear or opaque.{animated && <> Timing rounds to 10&nbsp;ms.</>}</p>}
    {isImage && target === 'avif' && <p className="settings-note">AVIF supports up to 12-bit color.</p>}
  </div>;
}

function DestinationBrowser({ formats, onChoose }: { formats: Destination[]; onChoose: (format: string) => void }) {
  formats = formats.filter(format => !format.hidden);
  const categories = [...new Set(formats.map((format) => format.category))];
  return <div className="destination-browser">
    <h2>Convert to</h2>
    {formats.length === 0 && <p className="settings-note">These files don’t share an available output yet. Expand the input group for details about unsupported files.</p>}
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
  const filesRef = useRef(files);
  filesRef.current = files;
  const pickerOpen = useRef(false);
  const [outputFolder, setOutputFolder] = useState<string | null>(null);
  const conversion = useConversion();
  const { job, busy } = conversion;
  const [appearance, setAppearance] = useState<Appearance>(() => {
    try { const saved = localStorage.getItem('recast.appearance'); return saved === 'light' || saved === 'dark' ? saved : 'system'; }
    catch { return 'system'; }
  });
  const [settingsOpen, setSettingsOpen] = useState(false);
  const settingsOpenRef = useRef(false);
  settingsOpenRef.current = settingsOpen;
  const settingsButton = useRef<HTMLButtonElement>(null);
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
  const activeFormatLabel = destinationsFor(activeKind, activeFiles, true).find(format => format.id === activeSettings.target)?.label ?? activeSettings.target?.split(':').at(-1)?.toUpperCase();
  const unresolvedTarget = activeSettings.target && !selectedFormat;
  const affectedFiles = unresolvedTarget ? activeFiles.filter(file => previewCatalog ? activeKind === 'video' && file.hasAudio !== true : !file.targets?.includes(activeSettings.target!)) : [];
  const activeAudioIssue = isAudioTarget(activeSettings.target) ? activeFiles.map(file => audioSettingIssue(file, activeSettings)).find(Boolean) : null;
  const videoIssue = files.some(file => file.kind === 'video') ? videoSettingIssue(settings.video, files) : null;
  const audioSettingsValid = files.filter(file => isAudioTarget(settings[file.kind].target)).every(file => !audioSettingIssue(file, settings[file.kind]));
  const allConfigured = presentKinds.length > 0 && presentKinds.every((kind) => destinationsFor(kind, files.filter((file) => file.kind === kind), previewCatalog).some((format) => format.id === settings[kind].target));
  const folderName = outputFolder?.split(/[\\/]/).filter(Boolean).at(-1) ?? 'Same as source';
  const succeeded = job?.files.filter(file => file.status === 'succeeded') ?? [];
  const failed = job?.files.filter(file => file.status === 'failed') ?? [];
  const retryCount = job ? job.files.length - succeeded.length : 0;
  const currentFile = job?.files.find(file => file.status === 'running');
  const currentInput = files.find(file => file.id === currentFile?.id);
  const currentTarget = currentInput && settings[currentInput.kind].target;
  const currentFormat = currentInput && destinationsFor(currentInput.kind, [currentInput], true).find(format => format.id === currentTarget)?.label;
  const currentPhase = busy && currentFile ? currentFile.phase || (currentFormat ? `Converting to ${currentFormat}` : 'Converting file') : '';
  const measuredProgress = currentFile?.progress;
  const completedUnits = succeeded.length + failed.length;
  const progressTotal = Math.max(job?.files.length ?? files.length, 1);
  const progressValue = completedUnits + (measuredProgress ?? 0);
  const progressPercent = measuredProgress != null ? Math.round(progressValue / progressTotal * 100) : null;
  const imageFramePending = isImageFrameTarget(settings.images.target) && files.some(file => file.kind === 'images' && file.animated && file.frameTime != null && !file.frame);
  const canConvert = !imageFramePending && !videoIssue && audioSettingsValid && conversion.backendReady && !previewCatalog && allConfigured && !inspecting && !busy && files.every(file => file.path);
  const jobText = conversion.starting ? 'Preparing conversion…' : job?.status === 'cancelling' ? 'Cancelling…' : busy && job ? `Converting ${Math.min(succeeded.length + failed.length + 1, job.files.length)} of ${job.files.length}` : job ? `${succeeded.length} converted${failed.length ? ` · ${failed.length} failed` : ''}${job.status === 'cancelled' ? ' · Cancelled' : ''}` : '';

  useEffect(() => {
    if (job && !busy) setExpanded(current => [...new Set([...current, ...filesRef.current.filter(file => job.files.some(result => result.id === file.id && (result.status === 'failed' || result.warnings?.length))).map(file => file.kind)])]);
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
    if (settingsOpenRef.current || !paths.length || conversion.busyRef.current) return;
    inspectionCount.current++;
    setInspecting(true);
    try {
      const result = await invoke<{ files: InputFile[]; errors: { name: string; message: string }[] }>('inspect_inputs', { paths });
      const realFiles = filesRef.current.filter(file => file.path);
      const seen = new Set(realFiles.map(file => file.id));
      const additions = result.files.filter(file => !seen.has(file.id) && seen.add(file.id));
      if (additions.length) {
        conversion.clear();
        filesRef.current = [...realFiles, ...additions];
        setFiles(filesRef.current);
      }
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
      if (settingsOpenRef.current || conversion.busyRef.current) { setDragging(false); return; }
      if (event.payload.type === 'over' || event.payload.type === 'enter') setDragging(true);
      if (event.payload.type === 'leave') setDragging(false);
      if (event.payload.type === 'drop') { setDragging(false); void addPaths(event.payload.paths); }
    }).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten; }).catch(() => setNotice('Drag and drop is unavailable. Use Choose files instead.'));
    return () => { disposed = true; stop?.(); };
  }, [addPaths]);

  async function chooseFiles() {
    if (settingsOpenRef.current || conversion.busyRef.current || pickerOpen.current || inspectionCount.current > 0) return;
    if (!isTauri()) { setNotice('File selection is available in the desktop app. Use the sample batches below to explore this browser preview.'); return; }
    pickerOpen.current = true;
    try {
      const paths = await open({ multiple: true, directory: false, title: 'Choose files to convert' });
      if (paths) await addPaths(Array.isArray(paths) ? paths : [paths]);
    } catch { setNotice('The file picker could not open. Try dropping files into the window.'); }
    finally { pickerOpen.current = false; }
  }

  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let stop: (() => void) | undefined;
    void getCurrentWindow().listen('recast:add-files', () => void chooseFiles())
      .then(unlisten => { if (disposed) unlisten(); else stop = unlisten; })
      .catch(() => setNotice('The Add Files menu is unavailable. Use the button or drop files here.'));
    return () => { disposed = true; stop?.(); };
  }, []);

  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let stop: (() => void) | undefined;
    void getCurrentWindow().listen('recast:settings', () => { setSettingsOpen(true); setAppearanceOpen(false); })
      .then(unlisten => { if (disposed) unlisten(); else stop = unlisten; });
    return () => { disposed = true; stop?.(); };
  }, []);

  const [extensionRevision, setExtensionRevision] = useState(0);
  const appliedExtensionRevision = useRef(0);
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let stop: (() => void) | undefined;
    void getCurrentWindow().listen('recast:extensions-changed', () => setExtensionRevision(value => value + 1))
      .then(unlisten => { if (disposed) unlisten(); else stop = unlisten; });
    return () => { disposed = true; stop?.(); };
  }, []);
  useEffect(() => {
    if (busy || inspecting || appliedExtensionRevision.current === extensionRevision) return;
    appliedExtensionRevision.current = extensionRevision;
    const paths = filesRef.current.filter(file => file.path).map(file => file.path);
    if (!paths.length) return;
    inspectionCount.current++; setInspecting(true);
    void invoke<{ files: InputFile[]; errors: { name: string; message: string }[] }>('inspect_inputs', { paths }).then(result => {
      setFiles(current => current.map(old => {
        const fresh = result.files.find(file => file.id === old.id);
        if (!fresh) return { ...old, targets: [], extensionTargets: [], conversionIssue: result.errors.find(error => error.name === old.name)?.message ?? "This file is no longer available." };
        const updated = { ...old, ...fresh };
        return old.selectedAudioTrack != null ? selectAudioTrack(updated, old.selectedAudioTrack) : updated;
      }));
      if (result.errors.length) setNotice(result.errors.map(error => `${error.name}: ${error.message}`).join(' '));
    }).catch(error => setNotice(String(error))).finally(() => { inspectionCount.current--; setInspecting(inspectionCount.current > 0); });
  }, [extensionRevision, busy, inspecting]);

  async function chooseFolder() {
    if (conversion.busyRef.current || pickerOpen.current) return;
    if (!isTauri()) { setNotice('Destination folder selection is available in the desktop app.'); return; }
    pickerOpen.current = true;
    try { const folder = await open({ directory: true, multiple: false, title: 'Choose an output folder', defaultPath: outputFolder ?? undefined }); if (typeof folder === 'string' && folder !== outputFolder) { conversion.clear(); setOutputFolder(folder); } }
    catch { setNotice('The folder picker could not open.'); }
    finally { pickerOpen.current = false; }
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
    if (Object.entries(change).every(([key, value]) => activeSettings[key as keyof GroupSettings] === value)) return;
    conversion.clear();
    if (activeKind === 'video' && change.target && isAudioTarget(change.target) && activeFiles.some(file => (file.audioTracks?.length ?? 0) > 1)) {
      setExpanded(current => current.includes('video') ? current : [...current, 'video']);
    }
    setSettings((current) => ({ ...current, [activeKind]: { ...current[activeKind], ...change } }));
  }

  function newBatch() {
    if (conversion.busyRef.current) return;
    conversion.clear(); setFiles([]); setSettings(defaultSettings()); setExpanded([]); setOutputFolder(null); setNotice('');
  }

  async function runConversion(retry = false, allowClipping = false) {
    if ((!canConvert && !retry) || conversion.busyRef.current || inspectionCount.current > 0) return;
    setNotice('');
    try { await conversion.start(files, settings, outputFolder, retry, allowClipping); }
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
        <button ref={settingsButton} className="icon-button" aria-label="Settings" aria-pressed={settingsOpen} disabled={settingsOpen} onClick={() => { setSettingsOpen(true); setAppearanceOpen(false); }}><Settings size={17} strokeWidth={1.7} /></button>
        {!settingsOpen && files.length > 0 && <button className="quiet-button add-files" onClick={() => void chooseFiles()} disabled={inspecting || busy}><Plus size={16} />{inspecting ? 'Adding…' : 'Add files'}</button>}
      </div>
    </header>

    {notice && <div className="notice" role="status"><span>{notice}</span><button className="icon-button" aria-label="Dismiss message" onClick={() => setNotice('')}><X size={15} /></button></div>}
    {conversion.backendError && <div className="notice" role="alert">{conversion.backendError}</div>}

    {settingsOpen ? <ExtensionSettings onClose={() => { setSettingsOpen(false); requestAnimationFrame(() => settingsButton.current?.focus()); }} /> : files.length === 0 ? <main className="empty-state">
      <div className="empty-symbol" aria-hidden="true"><File className="file-back" size={49} strokeWidth={1.15} /><File className="file-front" size={49} strokeWidth={1.15} /><ArrowDownToLine className="file-arrow" size={18} strokeWidth={1.6} /></div>
      <h1>Drop files here</h1>
      <p>Convert files. All on your device.</p>
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
                  const detail = result?.error ?? (settings[kind].target ? file.targetIssues?.[settings[kind].target!] : undefined) ?? file.conversionIssue ?? (result ? ({ pending: 'Waiting', running: result.phase ? `${result.phase} · ${Math.round((result.progress ?? 0) * 100)}%` : 'Converting…', succeeded: 'Converted', failed: 'Failed', cancelled: 'Cancelled' }[result.status]) : file.animated ? 'Animated' : videoDescription(file) || audioDescription(file));
                  return <div className="file-row" key={file.id}><div className="file-detail"><span title={file.path || file.name}>{file.name}</span>{result?.status === 'succeeded' ? <ConversionResult result={result} /> : detail && <span className="file-status">{detail}</span>}
                    {kind === 'video' && (!settings.video.target || isAudioTarget(settings.video.target)) && (file.audioTracks?.length ?? 0) > 1 && <fieldset className="track-choice" disabled={busy}>
                      <SelectSetting label="Extract track" value={file.audioTracks!.find(track => track.index === file.selectedAudioTrack)?.label ?? ''} options={file.audioTracks!.map(track => track.label)} onChange={label => {
                        if (conversion.busyRef.current) return;
                        const track = file.audioTracks!.find(track => track.label === label);
                        if (!track || track.index === file.selectedAudioTrack) return;
                        conversion.clear();
                        setFiles(current => current.map(input => input.id === file.id ? selectAudioTrack(input, track.index) : input));
                      }} />
                    </fieldset>}
                  </div><button className="remove-file" disabled={busy} aria-label={`Remove ${file.name}`} onClick={() => { conversion.clear(); setFiles((current) => current.filter((item) => item.id !== file.id)); }}><X size={13} /></button></div>;
                })}</div></ScrollArea>}
              </section>;
            })}</GroupStack>
          </ScrollArea></div>
        </section>

        <section className="conversion-region" aria-label="Conversion settings">
          <div className="conversion-heading">{activeFiles.length} {unitLabels[activeKind]}</div>
          <ScrollArea className="conversion-scroll"><div className="conversion-content" key={activeKind}>
            {unresolvedTarget && <div className="compatibility-message" role="status"><p>{affectedFiles.length === 1 ? affectedFiles[0].name : `${affectedFiles.length} files`} cannot be converted to {activeFormatLabel}.</p><p className="compatibility-reason">{previewCatalog ? affectedFiles.every((file) => file.hasAudio === false) ? 'No audio track was found.' : 'An audio track has not been confirmed for every file.' : affectedFiles.length === 1 ? (affectedFiles[0].targetIssues?.[activeSettings.target ?? ''] ?? affectedFiles[0].conversionIssue) : 'Not every file supports this output.'}</p><span>Remove {affectedFiles.length === 1 ? 'it' : 'them'} to keep this format{formats.length ? ', or choose another for the whole group.' : '.'}</span><button className="text-button" onClick={() => { setSelected(activeKind); setExpanded((current) => current.includes(activeKind) ? current : [...current, activeKind]); }}>Show files<ChevronRight size={13} /></button></div>}
            <fieldset className="conversion-fields" disabled={busy}>
            {selectedFormat ? <>
              <button className="format-back" onClick={() => updateSettings({ target: null })} aria-label="Change output format"><ArrowLeft size={18} strokeWidth={1.7} /><h2>{selectedFormat.label}</h2></button>
              {activeKind === 'video' && selectedFormat.category !== 'Still image' && videoIssue && <p className="compatibility-message" role="status">{videoIssue}</p>}
              {activeAudioIssue && <p className="compatibility-message" role="status">{activeAudioIssue}</p>}
              {activeKind === 'video' && isVideoAnimationTarget(selectedFormat.id) && <VideoRange files={activeFiles} settings={activeSettings} disabled={busy} onChange={(id, sourceRange) => {
                if (conversion.busyRef.current) return;
                conversion.clear();
                setFiles(current => current.map(file => file.id === id ? { ...file, sourceRange } : file));
              }} />}
              {selectedFormat.category === 'Still image' && (activeKind === 'video' || activeFiles.some(file => file.animated)) && <FramePicker files={activeKind === 'images' ? activeFiles.filter(file => file.animated) : activeFiles} disabled={busy} onChange={(id, change, edited) => {
                if (conversion.busyRef.current) return;
                if (edited) conversion.clear();
                setFiles(current => current.map(file => file.id === id ? { ...file, ...change } : file));
              }} />}
              <FormatSettings kind={activeKind} destination={selectedFormat} value={activeSettings} onChange={updateSettings} disabled={busy} animated={!isImageFrameTarget(activeSettings.target) && activeFiles.some(file => file.animated)} files={activeFiles} />
            </> : <DestinationBrowser formats={formats} onChoose={(target) => updateSettings({ target })} />}
            </fieldset>
            {activeKind === 'video' && !selectedFormat && !expanded.includes('video') && activeFiles.some(file => (file.audioTracks?.length ?? 0) > 1) && <button className="text-button" onClick={() => setExpanded(current => [...current, 'video'])}>Audio tracks<ChevronRight size={13} /></button>}
            {!unresolvedTarget && !formats.length && !previewCatalog && <button className="text-button" onClick={() => setExpanded(current => current.includes(activeKind) ? current : [...current, activeKind])}>Show files<ChevronRight size={13} /></button>}
          </div></ScrollArea>
        </section>

        <section className="output-region" aria-label="Output summary">
          <div className="mass output-mass">
            <div className="region-heading"><h1>Output</h1></div>
            <ScrollArea className="group-scroll" onScroll={syncGroupScroll}><GroupStack selected={activeKind}>{presentKinds.map((kind) => {
            const groupFiles = files.filter((file) => file.kind === kind);
            const target = destinationsFor(kind, groupFiles, previewCatalog).find((format) => format.id === settings[kind].target);
            const groupResults = job?.files.filter(result => groupFiles.some(file => file.id === result.id)) ?? [];
            const groupSucceeded = groupResults.filter(result => result.status === 'succeeded');
            const groupFailed = groupResults.filter(result => result.status === 'failed').length;
            const running = groupResults.find(result => result.status === 'running');
            const groupText = running ? running.phase ? `${running.phase} · ${Math.round((running.progress ?? 0) * 100)}%` : 'Converting…' : groupResults.every(result => result.status === 'pending') ? 'Waiting' : `${groupSucceeded.length} converted${groupFailed ? ` · ${groupFailed} failed` : ''}${groupResults.some(result => result.status === 'cancelled') ? ' · Cancelled' : ''}`;
            return <section key={kind} className="output-group" style={{ minHeight: groupSizes[kind] }}>
              <button className="group-header output-select" data-selection-key={kind} aria-label={`${kindLabels[kind]} output`} aria-pressed={activeKind === kind} onClick={() => setSelected(kind)}>
                <span className={`output-result ${target ? '' : 'pending-output'}`}>{groupFiles.length} {target?.label ?? 'files'}</span>
                <span className="output-detail">{job ? groupText : kind === 'video' && videoIssue && !isFrameTarget(settings.video.target) ? 'Choose compatible video settings' : isAudioTarget(settings[kind].target) && groupFiles.some(file => audioSettingIssue(file, settings[kind])) ? 'Choose compatible audio settings' : target ? outputFolder ? folderName : 'Same folder as source' : 'Choose an output format'}</span>
              </button>
              {groupSucceeded.length > 0 && <div className="output-files" aria-label="Converted files">{groupSucceeded.map(file => <button className="output-file" key={file.id} title={file.outputPath!} aria-label={`Show ${file.outputPath!.split(/[\\/]/).at(-1)} in folder`} onClick={() => void revealOutput(file.outputPath!)}><Check size={13} aria-hidden="true" /><span>{file.outputPath!.split(/[\\/]/).at(-1)}</span><Folder size={13} aria-hidden="true" /></button>)}</div>}
            </section>;
          })}</GroupStack>
            </ScrollArea></div>
        </section>
      </main>

      <footer className="batch-footer">
        <div className="batch-total">
          <div className="batch-status-row">
            <span className="batch-status" role="status" aria-live="polite" aria-atomic="true">
              <span>{job || conversion.starting ? jobText : `${files.length} files`}</span>
              {currentPhase && <span className="batch-phase" title={currentPhase}>{currentPhase}</span>}
            </span>
            {busy && progressPercent != null && <span className="batch-percent" aria-hidden="true">{progressPercent}%</span>}
          </div>
          {busy && <div className="batch-progress">
            <span className="progress-activity" aria-hidden="true" />
            <progress aria-label="Batch progress" aria-valuetext={progressPercent != null ? `${progressPercent}% of batch complete. ${currentPhase}` : `${completedUnits} of ${progressTotal} files complete. ${currentPhase || jobText}`} max={progressTotal} value={progressValue} />
          </div>}
          {currentFile && <span className="current-file" title={currentFile.name}>{currentFile.name}</span>}
        </div>
        {!busy && failed.some(file => file.canAllowClipping) && <button className="quiet-button" onClick={() => { setSettings(current => ({ ...current, audio: { ...current.audio, allowClipping: true }, video: { ...current.video, allowClipping: true } })); void runConversion(true, true); }}>Allow clipping & retry</button>}
        {!busy && job && retryCount > 0 && <button className="quiet-button" onClick={newBatch}>New batch</button>}
        <div className="folder-choice"><button className="folder-button" disabled={busy} onClick={() => void chooseFolder()} title={outputFolder ?? 'Choose an output folder'}><Folder size={16} strokeWidth={1.6} /><span>{folderName}</span><ChevronDown size={12} /></button>{outputFolder && <button className="icon-button" disabled={busy} aria-label="Use source folders" title="Use source folders" onClick={() => { conversion.clear(); setOutputFolder(null); }}><X size={12} /></button>}</div>
        {busy ? <button className="primary-button" disabled={conversion.starting || job?.status === 'cancelling'} onClick={() => void conversion.cancel().catch(error => setNotice(String(error)))}>{job?.status === 'cancelling' ? 'Cancelling…' : 'Cancel'}</button> : job ? retryCount > 0 ? <button className="primary-button" onClick={() => void runConversion(true)}>Retry {retryCount} {retryCount === 1 ? 'file' : 'files'}</button> : <button className="primary-button" onClick={newBatch}>New batch</button> : <button className={`primary-button convert-button ${allConfigured ? 'configured' : ''}`} disabled={!canConvert} onClick={() => void runConversion()} title={previewCatalog ? 'Sample files are for design review. Add real images, audio, or video files to convert.' : !allConfigured ? 'Choose a supported output for every group.' : !audioSettingsValid ? 'Choose compatible audio settings.' : imageFramePending ? 'Preview the selected frame before converting.' : videoIssue ?? undefined}>Convert {files.length} {files.length === 1 ? 'file' : 'files'}</button>}
      </footer>
    </>}

    {dragging && <div className="drop-overlay"><ArrowDownToLine size={27} /><span>Add to this batch</span></div>}

    {designMode && !settingsOpen && <aside className="design-tools" aria-label="Design preview controls"><span className="design-label">Design preview<span className="subtle-dot">·</span>{previewCatalog ? 'Sample files' : 'Real files'}</span><div className="scenario-control"><label htmlFor="scenario">View</label><select id="scenario" disabled={busy} value={scenario} onChange={(event) => changeScenario(event.target.value as Scenario)}><option value="empty">Empty</option><option value="mixed">Mixed batch</option><option value="images">Images</option><option value="video">Video</option></select></div>{scenario === 'video' && <button className="design-action" disabled={busy || files.some((file) => file.id === 'sample-silent')} onClick={() => setFiles((current) => [...current, { id: 'sample-silent', name: 'Screen recording.mp4', path: '', kind: 'video', format: 'MP4', bytes: 24_000_000, hasAudio: false }])}>Add silent video</button>}<span className="design-status">{previewCatalog ? 'Sample conversion disabled' : 'PNG / JPEG → WebP'}</span></aside>}
  </div>;
}
