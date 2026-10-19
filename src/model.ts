export type MediaKind = 'images' | 'audio' | 'video';
export type Appearance = 'system' | 'light' | 'dark';

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
  targetIssues?: Record<string, string>;
  audio?: { container: string; codec: string; sampleRate: number; sampleFormat: string; channels: number; channelLayout: string | null; duration: number | null } | null;
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
  resolution: string;
  frameRate: string;
  sampleRate: string;
  sampleFormat: string;
  audioDither: boolean;
  allowClipping: boolean;
}

export interface Destination {
  id: string;
  label: string;
  description: string;
  category: string;
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
];

// Preview fixtures use the full planned catalog. Real file targets are inspected
// by Rust and intersected here: every file must support the whole-group target.
export function destinationsFor(kind: MediaKind, files: InputFile[], preview = false): Destination[] {
  if (!preview) {
    if (!files.length) return [];
    const catalog = kind === 'images' ? imageDestinations : kind === 'audio' ? audioDestinations : videoDestinations;
    return catalog.filter(format => files.every(file => file.targets?.includes(format.id)));
  }
  if (kind === 'images') return imageDestinations;
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
    colors: 256, dither: true, resize: 'Original', bitrate: '192 kbps', resolution: 'Original', frameRate: 'Original',
    sampleRate: 'Keep source', sampleFormat: 'Keep source', audioDither: true, allowClipping: false,
    mp3Mode: 'vbr', mp3Quality: 'V2 · High', aacBitrate: '192 kbps',
  };
  return { images: { ...settings }, audio: { ...settings }, video: { ...settings } };
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

export function isLossyAudio(file: InputFile): boolean {
  return ['mp3', 'aac'].includes(file.audio?.codec ?? '');
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
  if (['mp3', 'm4a'].includes(settings.target ?? '')) return 'f32';
  if (settings.sampleFormat !== 'Keep source') return audioSampleFormats[settings.sampleFormat];
  return isLossyAudio(file) ? settings.target === 'wav' ? 'f32' : 's24' : file.audio!.sampleFormat;
}

export function audioSettingIssue(file: InputFile, settings: GroupSettings): string | null {
  if (!file.audio || !settings.target) return null;
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
  if (!file.audio || !settings.audioDither || ['mp3', 'm4a'].includes(settings.target ?? '')) return false;
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
