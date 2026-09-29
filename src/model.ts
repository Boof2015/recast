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
}

export interface GroupSettings {
  target: string | null;
  quality: number;
  lossless: boolean;
  metadata: boolean;
  background: string;
  resize: string;
  bitrate: string;
  resolution: string;
  frameRate: string;
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
  { id: 'gif', label: 'GIF', description: 'Limited-color images', category: 'Image' },
  { id: 'bmp', label: 'BMP', description: 'Uncompressed images', category: 'Image' },
];
const audioDestinations: Destination[] = [
  { id: 'mp3', label: 'MP3', description: 'Plays almost anywhere', category: 'Audio' },
  { id: 'wav', label: 'WAV', description: 'Uncompressed audio', category: 'Audio' },
  { id: 'flac', label: 'FLAC', description: 'Lossless & compact', category: 'Audio' },
  { id: 'alac', label: 'ALAC', description: 'Apple lossless audio', category: 'Audio' },
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
    resize: 'Original', bitrate: '192 kbps', resolution: 'Original', frameRate: 'Original',
  };
  return { images: { ...settings }, audio: { ...settings }, video: { ...settings } };
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
