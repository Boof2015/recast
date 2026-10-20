import { useEffect, useRef, useState } from 'react';
import { Channel, invoke, isTauri } from '@tauri-apps/api/core';
import { audioSampleFormats, type GroupSettings, type InputFile, type MediaKind } from './model';

export interface FileResult {
  id: string;
  name: string;
  status: 'pending' | 'running' | 'succeeded' | 'failed' | 'cancelled';
  outputPath: string | null;
  outputBytes: number | null;
  error: string | null;
  warnings?: string[];
  details?: string | null;
  canAllowClipping?: boolean;
  progress?: number | null;
  phase?: string | null;
}

export interface BatchSnapshot {
  id: string;
  revision: number;
  status: 'running' | 'cancelling' | 'completed' | 'cancelled';
  files: FileResult[];
}

export function useConversion() {
  const [job, setJob] = useState<BatchSnapshot | null>(null);
  const [starting, setStarting] = useState(false);
  const [backendReady, setBackendReady] = useState(false);
  const [backendError, setBackendError] = useState('');
  const busyRef = useRef(false);
  const generation = useRef(0);
  const busy = starting || job?.status === 'running' || job?.status === 'cancelling';

  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    void invoke('backend_status').then(() => { if (!disposed) setBackendReady(true); }).catch(error => { if (!disposed) setBackendError(String(error)); });
    return () => { disposed = true; generation.current++; };
  }, []);

  function accept(snapshot: BatchSnapshot) {
    setJob(previous => previous?.id === snapshot.id && previous.revision > snapshot.revision ? previous : snapshot);
    if (snapshot.status === 'completed' || snapshot.status === 'cancelled') busyRef.current = false;
  }

  async function start(files: InputFile[], groups: Record<MediaKind, GroupSettings>, outputFolder: string | null, retry = false, allowClipping = false) {
    if (busyRef.current) return;
    busyRef.current = true;
    setStarting(true);
    const token = ++generation.current;
    const channel = new Channel<BatchSnapshot>();
    channel.onmessage = snapshot => { if (token === generation.current) accept(snapshot); };
    try {
      const settings = groups.images;
      const audio = groups.audio;
      const video = groups.video;
      const mode = video.target === 'mkv' ? video.mkvMode : video.videoMode;
      const targetSize = mode === 'Target size';
      const copyStreams = mode === 'Keep original';
      const resolution = targetSize ? video.sizeResolution : video.resolution;
      const snapshot = await invoke<BatchSnapshot>(retry ? 'retry_conversion' : 'start_conversion', {
        onProgress: channel,
        ...(retry ? { allowClipping } : {}),
        ...(!retry ? { request: {
          paths: files.map(file => file.path), outputFolder,
          imageOptions: files.some(file => file.kind === 'images') ? { target: settings.target, quality: settings.quality, lossless: settings.lossless, resize: settings.resize === 'Original' ? 100 : Number.parseInt(settings.resize, 10), metadata: settings.metadata, background: settings.background, colors: settings.colors, dither: settings.dither } : null,
          videoOptions: files.some(file => file.kind === 'video') ? { target: video.target, quality: video.quality, resolution: copyStreams || ['Auto', 'Original'].includes(resolution) ? null : Number.parseInt(resolution), sizeLimit: targetSize ? Math.floor(Number(video.targetSize) * 1_000_000) : null, autoResolution: targetSize && resolution === 'Auto', copyStreams, acceleration: video.acceleration.toLowerCase(), frameRate: copyStreams || video.frameRate === 'Original' ? null : Number.parseInt(video.frameRate), metadata: video.metadata } : null,
          audioOptions: files.some(file => file.kind === 'audio') ? { target: audio.target, sampleRate: audio.sampleRate === 'Keep source' ? null : Number(audio.sampleRate), sampleFormat: audioSampleFormats[audio.sampleFormat], dither: audio.audioDither, metadata: audio.metadata, allowClipping: audio.allowClipping, mp3: { mode: audio.mp3Mode, quality: Number.parseInt(audio.mp3Quality.slice(1)), bitrate: Number.parseInt(audio.bitrate) }, aac: { bitrate: Number.parseInt(audio.aacBitrate) }, opus: { bitrate: Number.parseInt(audio.opusBitrate) }, vorbis: { quality: Number.parseInt(audio.vorbisQuality) } } : null,
        } } : {}),
      });
      if (token === generation.current) accept(snapshot);
    } catch (error) {
      busyRef.current = false;
      throw error;
    } finally { setStarting(false); }
  }

  async function cancel() {
    const token = generation.current;
    const snapshot = await invoke<BatchSnapshot>('cancel_conversion');
    if (token === generation.current) accept(snapshot);
  }

  function clear() {
    if (busyRef.current) return;
    generation.current++;
    setJob(null);
  }

  return { job, busy, busyRef, starting, backendReady, backendError, start, cancel, clear };
}
