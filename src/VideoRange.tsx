import { useEffect, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { ChevronLeft, ChevronRight, Play, Square, RotateCcw } from 'lucide-react';
import { RangeTimeline, rangeTime as stamp } from './RangeTimeline';
import { videoAnimationOptions, sourceRange, type GroupSettings, type InputFile, type SourceRange } from './model';

const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));
const round = (value: number) => Math.round(value * 100) / 100;
function TimeField({ label, value, min, max, onChange }: { label: string; value: number; min: number; max: number; onChange: (value: number) => void }) {
  const [draft, setDraft] = useState(value.toFixed(2));
  const discard = useRef(false);
  useEffect(() => setDraft(value.toFixed(2)), [value]);
  return <label className="range-time"><span>{label}</span><div><input aria-label={`${label} in seconds`} type="text" inputMode="decimal" value={draft} onChange={e => setDraft(e.target.value)} onBlur={() => {
    if (discard.current) { discard.current = false; setDraft(value.toFixed(2)); return; }
    const parsed = draft.trim() ? Number(draft) : NaN;
    const next = Number.isFinite(parsed) ? clamp(round(parsed), min, max) : value;
    setDraft(next.toFixed(2)); onChange(next);
  }} onKeyDown={e => { if (e.key === 'Enter') e.currentTarget.blur(); if (e.key === 'Escape') { discard.current = true; e.currentTarget.blur(); } }} /><span>s</span></div></label>;
}

export function VideoRange({ files, settings, disabled, onChange }: { files: InputFile[]; settings: GroupSettings; disabled: boolean; onChange: (id: string, range: SourceRange) => void }) {
  const [selected, setSelected] = useState(files[0]?.id);
  const index = Math.max(0, files.findIndex(file => file.id === selected));
  const file = files[index];
  if (!file) return null;
  return <div className="video-range">
    <div className="range-source"><span title={file.name}>{file.name}</span>{files.length > 1 && <div><button className="icon-button" aria-label="Previous source" disabled={disabled || index === 0} onClick={() => setSelected(files[index - 1].id)}><ChevronLeft size={15} /></button><span>{index + 1} / {files.length}</span><button className="icon-button" aria-label="Next source" disabled={disabled || index === files.length - 1} onClick={() => setSelected(files[index + 1].id)}><ChevronRight size={15} /></button></div>}</div>
    <RangeEditor key={file.id} file={file} settings={settings} disabled={disabled} onChange={range => onChange(file.id, range)} />
  </div>;
}

function RangeEditor({ file, settings, disabled, onChange }: { file: InputFile; settings: GroupSettings; disabled: boolean; onChange: (range: SourceRange) => void }) {
  const range = sourceRange(file);
  const duration = file.animation?.duration ?? file.video?.duration ?? 5;
  const [position, setPosition] = useState(range.start);
  const [motion, setMotion] = useState(false);
  const [motionTime, setMotionTime] = useState(range.start);
  const [image, setImage] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [revision, setRevision] = useState(0);
  const url = useRef('');
  const time = motion ? range.start : position;
  const activeTime = motion && !loading ? motionTime : position;

  useEffect(() => { if (disabled) setMotion(false); }, [disabled]);
  useEffect(() => {
    if (disabled) return;
    if (!isTauri() || !file.path) { setError('Add a video to preview its range.'); return; }
    let active = true;
    let started = false;
    const id = crypto.randomUUID();
    setLoading(true); setError('');
    const timer = setTimeout(() => {
      started = true;
      const animation = motion ? [videoAnimationOptions(settings), range] : null;
      void invoke<ArrayBuffer | number[]>('preview_video', { request: { requestId: id, path: file.path, time, animation } }).then(data => {
        if (!active) return;
        const next = URL.createObjectURL(new Blob([data instanceof ArrayBuffer ? data : new Uint8Array(data)], { type: motion ? settings.target === 'animated-webp' ? 'image/webp' : 'image/gif' : 'image/bmp' }));
        if (url.current) URL.revokeObjectURL(url.current);
        url.current = next; setImage(next); setLoading(false);
      }).catch(reason => { if (active) { setLoading(false); setError(String(reason)); } });
    }, motion ? 0 : 90);
    return () => { active = false; clearTimeout(timer); if (started) void invoke('cancel_video_preview', { requestId: id }).catch(() => {}); };
  }, [file.path, time, motion, range.start, range.end, settings.animationSize, settings.animationFrameRate, settings.colors, settings.dither, settings.target, settings.quality, settings.lossless, disabled, revision, duration]);
  useEffect(() => () => { if (url.current) URL.revokeObjectURL(url.current); }, []);
  useEffect(() => {
    if (!motion || loading || error || disabled) return;
    const start = performance.now(); let frame = 0;
    const tick = () => { setMotionTime(range.start + ((performance.now() - start) / 1000) % (range.end - range.start)); frame = requestAnimationFrame(tick); };
    frame = requestAnimationFrame(tick); return () => cancelAnimationFrame(frame);
  }, [motion, loading, error, disabled, range.start, range.end, image]);

  function update(which: 'start' | 'end' | 'position', value: number) {
    if (disabled) return;
    setMotion(false);
    if (which === 'position') setPosition(clamp(value, 0, duration));
    else {
      const next = which === 'start' ? { ...range, start: clamp(round(value), 0, range.end - 0.01) } : { ...range, end: clamp(round(value), range.start + 0.01, duration) };
      if (next.start !== range.start || next.end !== range.end) onChange(next);
      setPosition(next[which]);
    }
  }
  return <>
    <div className="range-picture" aria-busy={loading}>
      {image && !error && <img src={image} alt={motion ? `Selected range of ${file.name}` : `Video preview at ${stamp(position)}`} draggable={false} />}
      {loading && <span className="preview-loading" role="status">{motion ? 'Preparing preview…' : 'Loading…'}</span>}
      {error && <div className="preview-error" role="status"><span>{error}</span><button className="quiet-button" disabled={disabled} onClick={() => setRevision(value => value + 1)}>Try again<RotateCcw size={12} /></button></div>}
    </div>
    <div className="range-transport"><button className="quiet-button" aria-label={motion ? 'Stop preview' : 'Play selected range'} disabled={disabled} onClick={() => { setPosition(range.start); setMotion(!motion); }}>{motion ? <Square size={12} /> : <Play size={13} />}<span>{motion ? 'Stop' : 'Play range'}</span></button><span>{stamp(activeTime)} <span className="range-total">/ {stamp(duration)}</span></span></div>
    <RangeTimeline duration={duration} range={range} position={activeTime} disabled={disabled} onBoundary={update} onSeek={value => update('position', value)} onMove={next => {
      if (disabled) return;
      setMotion(false); onChange(next); setPosition(next.start);
    }} />
    <div className="range-times"><TimeField label="Start" value={range.start} min={0} max={range.end - 0.01} onChange={value => { if (value !== range.start) update('start', value); }} /><TimeField label="End" value={range.end} min={range.start + 0.01} max={duration} onChange={value => { if (value !== range.end) update('end', value); }} /><output aria-label="Selected duration">{(range.end - range.start).toFixed(2)} s</output></div>
  </>;
}
