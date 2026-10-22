import { useEffect, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { ChevronLeft, ChevronRight, StepBack, StepForward, RotateCcw } from 'lucide-react';
import { RangeTimeline, rangeTime } from './RangeTimeline';
import type { FrameSelection, InputFile } from './model';

type Change = { frameTime: number; frame?: FrameSelection };
type Preview = { selection: FrameSelection; previous: number | null; next: number | null };

export function VideoFrame({ files, disabled, onChange }: { files: InputFile[]; disabled: boolean; onChange: (id: string, change: Change, edited: boolean) => void }) {
  const [selected, setSelected] = useState(files[0]?.id);
  const [loading, setLoading] = useState(true);
  const index = Math.max(0, files.findIndex(file => file.id === selected));
  const file = files[index];
  if (!file) return null;
  return <div className="video-range video-frame">
    <div className="range-source"><span title={file.name}>{file.name}</span>{files.length > 1 && <div><button className="icon-button" aria-label="Previous source" disabled={disabled || loading || index === 0} onClick={() => { setLoading(true); setSelected(files[index - 1].id); }}><ChevronLeft size={15} /></button><span>{index + 1} / {files.length}</span><button className="icon-button" aria-label="Next source" disabled={disabled || loading || index === files.length - 1} onClick={() => { setLoading(true); setSelected(files[index + 1].id); }}><ChevronRight size={15} /></button></div>}</div>
    <FrameEditor key={file.id} file={file} disabled={disabled} onLoading={setLoading} onChange={(change, edited) => onChange(file.id, change, edited)} />
  </div>;
}

function FrameEditor({ file, disabled, onChange, onLoading }: { file: InputFile; disabled: boolean; onChange: (change: Change, edited: boolean) => void; onLoading: (value: boolean) => void }) {
  const duration = file.animation?.duration ?? file.video?.duration ?? 5;
  const [time, setTime] = useState(file.frameTime ?? 0);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [image, setImage] = useState('');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [revision, setRevision] = useState(0);
  const [draft, setDraft] = useState(time.toFixed(3));
  const discard = useRef(false);
  const url = useRef('');
  const change = useRef(onChange);
  change.current = onChange;
  const position = loading || error || !preview ? time : preview.selection.time;

  useEffect(() => setDraft(position.toFixed(3)), [position]);
  useEffect(() => onLoading(loading), [loading, onLoading]);
  useEffect(() => {
    if (disabled) return;
    let active = true;
    let started = false;
    const requestId = crypto.randomUUID();
    setLoading(true); setError('');
    change.current({ frameTime: time, frame: undefined }, false);
    if (!isTauri() || !file.path) { setLoading(false); setError('Add a video to choose a frame.'); return; }
    const timer = setTimeout(() => {
      started = true;
      void invoke<ArrayBuffer | number[]>('preview_video_frame', { request: { requestId, path: file.path, time } }).then(data => {
        if (!active) return;
        const bytes = data instanceof ArrayBuffer ? new Uint8Array(data) : new Uint8Array(data);
        if (bytes.byteLength < 4) throw new Error('The frame preview is incomplete.');
        const length = new DataView(bytes.buffer).getUint32(0, true);
        if (length > bytes.byteLength - 4) throw new Error('The frame preview is incomplete.');
        const result = JSON.parse(new TextDecoder().decode(bytes.subarray(4, 4 + length))) as Preview;
        const next = URL.createObjectURL(new Blob([bytes.subarray(4 + length)], { type: 'image/bmp' }));
        if (url.current) URL.revokeObjectURL(url.current);
        url.current = next; setImage(next); setPreview(result); setLoading(false);
        change.current({ frameTime: result.selection.time, frame: result.selection }, false);
      }).catch(reason => { if (active) { setLoading(false); setError(String(reason)); } });
    }, 90);
    return () => { active = false; clearTimeout(timer); if (started) void invoke('cancel_video_preview', { requestId }).catch(() => {}); };
  }, [file.path, time, revision, disabled]);
  useEffect(() => () => { if (url.current) URL.revokeObjectURL(url.current); }, []);

  function seek(value: number) {
    if (disabled) return;
    const next = Math.min(duration, Math.max(0, value));
    setTime(next); setLoading(true);
    change.current({ frameTime: next, frame: undefined }, true);
    if (next === time) setRevision(value => value + 1);
  }
  function step(direction: -1 | 1) {
    if (disabled || loading || error) return;
    const next = direction === -1 ? preview?.previous : preview?.next;
    if (next != null) seek(next);
  }
  return <>
    <div className="range-picture" aria-busy={loading}>
      {image && !error && <img src={image} alt={`Selected frame at ${preview?.selection.time.toFixed(3)} seconds`} draggable={false} />}
      {loading && <span className="preview-loading" role="status">Loading…</span>}
      {error && <div className="preview-error" role="status"><span>{error}</span><button className="quiet-button" disabled={disabled} onClick={() => setRevision(value => value + 1)}>Try again<RotateCcw size={12} /></button></div>}
    </div>
    <div className="range-transport frame-transport"><div>
      <button className="icon-button" aria-label="Previous frame" title="Previous frame" disabled={disabled || loading || !!error || preview?.previous == null} onClick={() => step(-1)}><StepBack size={14} /></button>
      <button className="icon-button" aria-label="Next frame" title="Next frame" disabled={disabled || loading || !!error || preview?.next == null} onClick={() => step(1)}><StepForward size={14} /></button>
    </div><label className="frame-position"><input aria-label="Frame time in seconds" inputMode="decimal" value={draft} disabled={disabled} onChange={e => setDraft(e.target.value)} onBlur={() => {
      if (discard.current) { discard.current = false; setDraft(position.toFixed(3)); return; }
      const value = draft.trim() ? Number(draft) : NaN;
      if (Number.isFinite(value) && draft !== position.toFixed(3)) seek(value);
      else setDraft(position.toFixed(3));
    }} onKeyDown={e => { if (e.key === 'Enter') e.currentTarget.blur(); if (e.key === 'Escape') { discard.current = true; e.currentTarget.blur(); } }} /><span>s</span></label><span className="range-total">/ {rangeTime(duration)}</span></div>
    <RangeTimeline frameOnly duration={duration} range={{ start: position, end: position }} position={position} disabled={disabled} onBoundary={() => {}} onMove={() => {}} onSeek={seek} onStep={step} />
  </>;
}
