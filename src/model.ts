export type MediaKind = 'images' | 'audio' | 'video';
export type Appearance = 'system' | 'light' | 'dark';
export interface SourceRange { start: number; end: number }
export interface FrameSelection { pts: string; time: number; source: string }

export interface InputFile {
  id: string;
  name: string;
  path: string;
  kind: MediaKind;
  format: string;
  bytes: number;
  hasAudio: boolean | null;
  targets?: string[];
  conversionIssue?: string | null;
  animated?: boolean;
  sequence?: { width: number; height: number; delays: number[]; loops: number } | null;
  targetIssues?: Record<string, string>;
  video?: { codec: string; width: number; height: number; rotation: number; duration: number; frameRate: number; audioTracks: number; subtitleTracks: number; aacIssue?: string | null; movCopyIssue?: string | null; webmCopyIssue?: string | null; opusIssue?: string | null } | null;
  audio?: { container: string; codec: string; sampleRate: number; sampleFormat: string; channels: number; channelLayout: string | null; duration: number | null } | null;
  audioTracks?: { index: number; label: string; default: boolean; audio: InputFile['audio']; issue: string | null; targets: string[]; targetIssues: Record<string, string> }[];
  selectedAudioTrack?: number | null;
  animation?: { duration: number; width: number; height: number } | null;
  sourceRange?: SourceRange;
  frameTime?: number;
  frame?: FrameSelection;
}

export interface GroupSettings {
  target: string | null;
  quality: number;
  lossless: boolean;
  metadata: boolean;
  background: string;
  colors: number;
  dither: boolean;
  resize: string;
  bitrate: string;
  mp3Mode: 'vbr' | 'cbr';
  mp3Quality: string;
  aacBitrate: string;
  opusBitrate: string;
  vorbisQuality: string;
  resolution: string;
  frameRate: string;
  acceleration: 'Automatic' | 'Software';
  videoMode: 'Quality' | 'Target size';
  mkvMode: 'Keep original' | 'Quality' | 'Target size';
  movMode: 'Keep original' | 'Quality' | 'Target size';
  webmMode: 'Keep original' | 'Quality' | 'Target size';
  targetSize: string;
  sizeResolution: string;
  sampleRate: string;
  sampleFormat: string;
  audioDither: boolean;
  allowClipping: boolean;
  gifSize: string;
  gifFrameRate: string;
}

export interface Destination {
  id: string;
  label: string;
  description: string;
  category: string;
  hidden?: boolean;
}

export const kinds: MediaKind[] = ['images', 'audio', 'video'];
export const kindLabels: Record<MediaKind, string> = { images: 'Images', audio: 'Audio', video: 'Video' };
export const unitLabels: Record<MediaKind, string> = { images: 'images', audio: 'audio files', video: 'videos' };

const imageDestinations: Destination[] = [
  { id: 'webp', label: 'WebP', description: 'Compact & versatile', category: 'Image' },
  { id: 'jpeg', label: 'JPEG', description: 'Everyday photos', category: 'Image' },
  { id: 'png', label: 'PNG', description: 'Lossless & transparent', category: 'Image' },
  { id: 'avif', label: 'AVIF', description: 'Smaller photo files', category: 'Image' },
  { id: 'tiff', label: 'TIFF', description: 'Lossless, single-page images', category: 'Image' },
  { id: 'gif', label: 'GIF', description: 'Images & animations', category: 'Image' },
  { id: 'bmp', label: 'BMP', description: 'Uncompressed images', category: 'Image' },
];
const audioDestinations: Destination[] = [
  { id: 'mp3', label: 'MP3', description: 'Plays almost anywhere', category: 'Audio' },
  { id: 'wav', label: 'WAV', description: 'Uncompressed audio', category: 'Audio' },
  { id: 'flac', label: 'FLAC', description: 'Lossless & compact', category: 'Audio' },
  { id: 'alac', label: 'ALAC', description: 'Apple lossless · .m4a', category: 'Audio' },
  { id: 'm4a', label: 'AAC / M4A', description: 'Compact, clear audio', category: 'Audio' },
  { id: 'opus', label: 'Opus', description: 'Efficient audio', category: 'Audio' },
  { id: 'ogg', label: 'Ogg Vorbis', description: 'Open audio format', category: 'Audio' },
];
const videoDestinations: Destination[] = [
  { id: 'mp4', label: 'MP4', description: 'Broad compatibility', category: 'Video' },
  { id: 'mkv', label: 'MKV', description: 'Flexible container', category: 'Video' },
  { id: 'webm', label: 'WebM', description: 'Made for the web', category: 'Video' },
  { id: 'mov', label: 'MOV', description: 'QuickTime video', category: 'Video' },
  { id: 'gif', label: 'GIF', description: 'A short animation', category: 'Animation' },
  ...imageDestinations.filter(format => format.id !== 'gif').map(format => ({ ...format, category: 'Still image' })),
];

export function isImageFrameTarget(target: string | null): boolean {
  return !!target?.startsWith('frame-');
}
function imageCatalog(files: InputFile[]): Destination[] {
  const animated = files.some(file => file.animated);
  return [
    ...(animated ? imageDestinations.filter(format => ['gif', 'webp'].includes(format.id)).map(format => ({ ...format, category: 'Animation', description: 'Keep animation' })) : imageDestinations),
    ...imageDestinations.map(format => ({ ...format, id: `frame-${format.id}`, description: format.id === 'gif' ? 'Single frame' : format.description, category: 'Still image', hidden: !animated })),
  ];
}

export function isFrameTarget(target: string | null): boolean {
  return imageDestinations.some(format => format.id !== 'gif' && format.id === target);
}

export function isAudioTarget(target: string | null): boolean {
  return audioDestinations.some(format => format.id === target);
}

export function selectAudioTrack(file: InputFile, index: number): InputFile {
  const track = file.audioTracks?.find(track => track.index === index);
  if (!track) return file;
  return { ...file, selectedAudioTrack: index, audio: track.audio, conversionIssue: track.targets.length ? null : file.conversionIssue,
    targets: [...(file.targets ?? []).filter(target => !isAudioTarget(target)), ...track.targets],
    targetIssues: { ...Object.fromEntries(Object.entries(file.targetIssues ?? {}).filter(([target]) => !isAudioTarget(target))), ...track.targetIssues },
  };
}

// Preview fixtures use the full planned catalog. Real file targets are inspected
// by Rust and intersected here: every file must support the whole-group target.
export function destinationsFor(kind: MediaKind, files: InputFile[], preview = false): Destination[] {
  if (!preview) {
    if (!files.length) return [];
    const catalog = kind === 'images' ? imageCatalog(files) : kind === 'audio' ? audioDestinations : [...videoDestinations, ...audioDestinations.map(format => ({ ...format, category: 'Audio only' }))];
    return catalog.filter(format => files.every(file => file.targets?.includes(format.id)));
  }
  if (kind === 'images') return imageCatalog(files);
  if (kind === 'audio') return audioDestinations;
  return [
    ...videoDestinations,
    ...(files.length > 0 && files.every((file) => file.hasAudio === true)
      ? audioDestinations.map((format) => ({ ...format, category: 'Audio only' }))
      : []),
  ];
}

export function defaultSettings(): Record<MediaKind, GroupSettings> {
  const settings: GroupSettings = {
    target: null, quality: 85, lossless: false, metadata: true, background: '#ffffff',
    colors: 256, dither: true, resize: 'Original', bitrate: '192 kbps', resolution: 'Original', frameRate: 'Original', acceleration: 'Automatic',
    sampleRate: 'Keep source', sampleFormat: 'Keep source', audioDither: true, allowClipping: false,
    mp3Mode: 'vbr', mp3Quality: 'V2 · High', aacBitrate: '192 kbps', opusBitrate: '128 kbps', vorbisQuality: '5 · High',
    videoMode: 'Quality', mkvMode: 'Keep original', movMode: 'Quality', webmMode: 'Quality', targetSize: '10', sizeResolution: 'Auto',
    gifSize: '480 px', gifFrameRate: '15 fps',
  };
  return { images: { ...settings }, audio: { ...settings }, video: { ...settings } };
}

export function videoModeKey(target: string | null): 'mkvMode' | 'movMode' | 'webmMode' | 'videoMode' {
  return target === 'mkv' ? 'mkvMode' : target === 'mov' ? 'movMode' : target === 'webm' ? 'webmMode' : 'videoMode';
}

export function videoSettingIssue(settings: GroupSettings, files: InputFile[] = []): string | null {
  if (isFrameTarget(settings.target)) {
    return files.some(file => file.kind === 'video' && file.frameTime != null && !file.frame)
      ? 'Preview the selected frame before converting.' : null;
  }
  if (settings.target === 'gif') {
    for (const file of files.filter(file => file.kind === 'video')) {
      if (!file.animation) continue;
      const { start, end } = sourceRange(file);
      if (!Number.isFinite(start) || !Number.isFinite(end) || start < 0 || end - start < 0.01 - 1e-8 || end > file.animation.duration + 0.001) return `${file.name}: choose a valid range.`;
      const factor = Math.min(1, Number.parseInt(settings.gifSize) / Math.max(file.animation.width, file.animation.height));
      const frames = Math.ceil((end - start) * Number.parseInt(settings.gifFrameRate) - 1e-8);
      const pixels = Math.round(file.animation.width * factor) * Math.round(file.animation.height * factor) * frames;
      if (frames > 1000 || pixels > 100_000_000) return `${file.name}: shorten the range, or lower size or frame rate.`;
    }
    return null;
  }
  const mode = settings[videoModeKey(settings.target)];
  if (settings.target === 'mov' && mode === 'Keep original') return files.find(file => file.kind === 'video' && file.video?.movCopyIssue)?.video?.movCopyIssue ?? null;
  if (settings.target === 'webm' && mode === 'Keep original') return files.find(file => file.kind === 'video' && file.video?.webmCopyIssue)?.video?.webmCopyIssue ?? null;
  if (!['mp4', 'mkv', 'mov', 'webm'].includes(settings.target ?? '') || mode !== 'Target size') return null;
  const issueKey = settings.target === 'webm' ? 'opusIssue' : 'aacIssue';
  const audioIssue = files.find(file => file.kind === 'video' && file.video?.[issueKey])?.video?.[issueKey];
  if (audioIssue) return audioIssue;
  const size = Number(settings.targetSize);
  return !/^\d+(\.\d*)?$/.test(settings.targetSize) || !Number.isFinite(size) || size < 0.1 || size > 10000
    ? 'Enter a maximum size from 0.1 to 10,000 MB.' : null;
}

export function sourceRange(file: InputFile): SourceRange {
  return file.sourceRange ?? { start: 0, end: Math.min(5, file.animation?.duration ?? file.video?.duration ?? 5) };
}

export const audioSampleFormats: Record<string, string> = {
  'Keep source': 'source', '8-bit PCM': 'u8', '16-bit PCM': 's16', '24-bit PCM': 's24',
  '32-bit PCM': 's32', '32-bit float': 'f32', '64-bit float': 'f64',
};

export const mp3Rates = ['8000', '11025', '12000', '16000', '22050', '24000', '32000', '44100', '48000'];
export const aacRates = ['7350', ...mp3Rates, '64000', '88200', '96000'];
export const aacBitrates = [16, 24, 32, 48, 64, 80, 96, 128, 160, 192, 224, 256, 320, 384, 448, 512, 640, 768];

export function aacBitrateChoices(files: InputFile[], settings: GroupSettings): string[] {
  return aacBitrates.filter(bitrate => files.every(file => !file.audio || bitrate * 1000 <= 6 * file.audio.channels * (settings.sampleRate === 'Keep source' ? file.audio.sampleRate : Number(settings.sampleRate))))
    .map(bitrate => `${bitrate} kbps`);
}

export const vorbisQualities = ['-1 · Smallest', '0', '1', '2', '3 · Standard', '4', '5 · High', '6', '7', '8', '9', '10 · Highest'];

export const opusBitrates = [16, 24, 32, 48, 64, 80, 96, 128, 160, 192, 224, 256, 320, 384, 448, 512];
export function opusBitrateChoices(files: InputFile[]): string[] {
  return opusBitrates.filter(bitrate => files.every(file => !file.audio || bitrate <= 256 * file.audio.channels)).map(bitrate => `${bitrate} kbps`);
}

export function isLossyAudio(file: InputFile): boolean {
  return ['mp3', 'aac', 'opus', 'vorbis'].includes(file.audio?.codec ?? '');
}

export const mp3Qualities = ['V0 · Highest', 'V2 · High', 'V4 · Standard', 'V6 · Smaller', 'V8 · Smallest'];

function mp3BitratesForRate(rate: number): number[] {
  return rate < 16000 ? [8, 16, 24, 32, 40, 48, 56, 64]
    : rate < 32000 ? [8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160]
    : [32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
}

export function mp3BitrateChoices(files: InputFile[], settings: GroupSettings): string[] {
  const rates = settings.sampleRate === 'Keep source' ? files.flatMap(file => file.audio ? [file.audio.sampleRate] : []) : [Number(settings.sampleRate)];
  return [8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 192, 224, 256, 320]
    .filter(bitrate => rates.every(rate => !mp3Rates.includes(String(rate)) || mp3BitratesForRate(rate).includes(bitrate)))
    .map(bitrate => `${bitrate} kbps`);
}

function resolvedAudioFormat(file: InputFile, settings: GroupSettings): string {
  if (['mp3', 'm4a', 'opus', 'ogg'].includes(settings.target ?? '')) return 'f32';
  if (settings.sampleFormat !== 'Keep source') return audioSampleFormats[settings.sampleFormat];
  return isLossyAudio(file) ? settings.target === 'wav' ? 'f32' : 's24' : file.audio!.sampleFormat;
}

export function audioSettingIssue(file: InputFile, settings: GroupSettings): string | null {
  if (!file.audio || !settings.target) return null;
  if (settings.target === 'ogg') {
    const rate = settings.sampleRate === 'Keep source' ? file.audio.sampleRate : Number(settings.sampleRate);
    if (rate > 200000) return `${file.name}: Vorbis cannot preserve ${rate} Hz. Choose a supported sample rate.`;
    if (!vorbisQualities.includes(settings.vorbisQuality)) return `${file.name}: choose a Vorbis quality from -1 to 10.`;
    return null;
  }
  if (settings.target === 'opus') {
    const bitrate = Number.parseInt(settings.opusBitrate);
    return !opusBitrates.includes(bitrate) || bitrate > 256 * file.audio.channels ? `${file.name}: choose a compatible Opus bitrate (at most 256 kbps per channel).` : null;
  }
  if (settings.target === 'mp3') {
    const rate = settings.sampleRate === 'Keep source' ? file.audio.sampleRate : Number(settings.sampleRate);
    if (!mp3Rates.includes(String(rate))) return `${file.name}: MP3 cannot preserve ${rate} Hz. Choose a supported sample rate.`;
    if (settings.mp3Mode === 'cbr' && !mp3BitratesForRate(rate).includes(Number.parseInt(settings.bitrate))) return `${file.name}: ${settings.bitrate} is unavailable at ${rate} Hz. Choose a compatible bitrate or use variable bitrate.`;
    return null;
  }
  if (settings.target === 'm4a') {
    const rate = settings.sampleRate === 'Keep source' ? file.audio.sampleRate : Number(settings.sampleRate);
    if (!aacRates.includes(String(rate))) return `${file.name}: AAC cannot preserve ${rate} Hz. Choose a supported sample rate.`;
    const bitrate = Number.parseInt(settings.aacBitrate);
    if (!aacBitrates.includes(bitrate) || bitrate * 1000 > 6 * rate * file.audio.channels) return `${file.name}: ${settings.aacBitrate} is unavailable at ${rate} Hz with ${file.audio.channels} channels. Choose a compatible bitrate.`;
    return null;
  }
  const format = resolvedAudioFormat(file, settings);
  if (['flac', 'alac'].includes(settings.target) && !['s16', 's24'].includes(format)) {
    return `${file.name}: choose 16-bit or 24-bit PCM for ${settings.target.toUpperCase()}, or use WAV to keep the source format.`;
  }
  if (settings.target === 'flac' && settings.sampleRate === 'Keep source' && file.audio.sampleRate > 655350) {
    return `${file.name}: choose a compatible sample rate for FLAC, or use WAV to keep the source rate.`;
  }
  if (settings.target === 'alac' && settings.sampleRate === 'Keep source' && file.audio.sampleRate > 384000) {
    return `${file.name}: ALAC output is limited to 384000 Hz. Choose a compatible rate or use WAV.`;
  }
  return null;
}

export function audioWillDither(file: InputFile, settings: GroupSettings): boolean {
  if (!file.audio || !settings.audioDither || ['mp3', 'm4a', 'opus', 'ogg'].includes(settings.target ?? '')) return false;
  const format = resolvedAudioFormat(file, settings);
  return !format.startsWith('f') && (isLossyAudio(file) || file.audio.sampleFormat.startsWith('f')
    || (settings.sampleRate !== 'Keep source' && Number(settings.sampleRate) !== file.audio.sampleRate)
    || Number(format.slice(1)) < Number(file.audio.sampleFormat.slice(1)));
}

export function audioDescription(file: InputFile): string {
  if (!file.audio) return '';
  const label = isLossyAudio(file) ? file.audio.codec.toUpperCase() : Object.entries(audioSampleFormats).find(([, code]) => code === file.audio!.sampleFormat)?.[0] ?? file.audio.sampleFormat;
  return `${file.audio.sampleRate} Hz · ${label} · ${file.audio.channelLayout ?? `${file.audio.channels} channels`}`;
}

export function formatCounts(files: InputFile[]): [string, number][] {
  const counts = new Map<string, number>();
  for (const file of files) counts.set(file.format, (counts.get(file.format) ?? 0) + 1);
  return [...counts.entries()];
}

export function formatBytes(bytes: number): string {
  if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
  if (bytes >= 1_000_000) return `${(bytes / 1_000_000).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1_000))} KB`;
}

export function videoDescription(file: InputFile): string {
  const video = file.video;
  if (!video) return '';
  const portrait = Math.abs(video.rotation) % 180 === 90;
  const seconds = Math.round(video.duration);
  return `${portrait ? video.height : video.width} × ${portrait ? video.width : video.height} · ${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')} · ${Number(video.frameRate.toFixed(2))} fps · ${video.audioTracks ? `${video.audioTracks} audio track${video.audioTracks === 1 ? '' : 's'}` : 'No audio'}`;
}
