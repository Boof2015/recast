import { useEffect, useRef, useState } from 'react';
import { Channel, invoke, isTauri } from '@tauri-apps/api/core';
import type { GroupSettings, InputFile } from './model';

export interface FileResult {
  id: string;
  name: string;
  status: 'pending' | 'running' | 'succeeded' | 'failed' | 'cancelled';
  outputPath: string | null;
  outputBytes: number | null;
  error: string | null;
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

  async function start(files: InputFile[], settings: GroupSettings, outputFolder: string | null, retry = false) {
    if (busyRef.current) return;
    busyRef.current = true;
    setStarting(true);
    const token = ++generation.current;
    const channel = new Channel<BatchSnapshot>();
    channel.onmessage = snapshot => { if (token === generation.current) accept(snapshot); };
    try {
      const snapshot = await invoke<BatchSnapshot>(retry ? 'retry_conversion' : 'start_conversion', {
        onProgress: channel,
        ...(!retry ? { request: {
          paths: files.map(file => file.path), outputFolder,
          options: { target: settings.target, quality: settings.quality, lossless: settings.lossless, resize: settings.resize === 'Original' ? 100 : Number.parseInt(settings.resize, 10), metadata: settings.metadata },
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
