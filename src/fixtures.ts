import type { InputFile } from './model';

export type Scenario = 'empty' | 'mixed' | 'images' | 'video';
const imageNames = ['Coastline', 'Morning light', 'Quiet corner', 'On the way', 'Blue hour', 'At home', 'Open water', 'Last summer', 'Still life', 'First snow', 'Window seat', 'Sunday', 'After the rain'];
const audioNames = ['A little further', 'Low tide', 'Silver lining', 'Night drive', 'In between', 'Field notes', 'Soft focus', 'Almost home', 'Overcast', 'Northbound', 'A slow morning', 'Familiar places'];

export function sampleFiles(scenario: Scenario): InputFile[] {
  if (scenario === 'empty') return [];
  if (scenario === 'video') return [
    { id: 'sample-video-1', name: 'Coastline.mov', path: '', kind: 'video', format: 'MOV', bytes: 186_000_000, hasAudio: true },
    { id: 'sample-video-2', name: 'A walk home.mp4', path: '', kind: 'video', format: 'MP4', bytes: 92_000_000, hasAudio: true },
    { id: 'sample-video-3', name: 'Sunday afternoon.mkv', path: '', kind: 'video', format: 'MKV', bytes: 348_000_000, hasAudio: true },
  ];
  const images: InputFile[] = Array.from({ length: 23 }, (_, index) => ({
    id: `sample-image-${index}`, name: `${imageNames[index % imageNames.length]}${index >= imageNames.length ? ' 02' : ''}.${index < 10 ? 'png' : 'jpg'}`,
    path: '', kind: 'images', format: index < 10 ? 'PNG' : 'JPEG',
    bytes: 1_420_000 + index * 230_000, hasAudio: null,
  }));
  if (scenario === 'images') return images;
  return [...images, ...audioNames.map((name, index): InputFile => ({
    id: `sample-audio-${index}`, name: `${name}.${index < 8 ? 'flac' : 'mp3'}`,
    path: '', kind: 'audio', format: index < 8 ? 'FLAC' : 'MP3',
    bytes: 4_420_000 + index * 2_100_000, hasAudio: true,
  }))];
}
