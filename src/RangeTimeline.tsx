import { useEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from 'react';
import { Maximize2, Minus, Plus } from 'lucide-react';
import type { SourceRange } from './model';

const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));
const round = (value: number) => Math.round(value * 100) / 100;
export function rangeTime(seconds: number) {
  const whole = Math.floor(seconds);
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, '0')}.${String(Math.floor((seconds - whole) * 100 + 1e-7)).padStart(2, '0')}`;
}
type View = { start: number; span: number };
type Part = 'start' | 'end' | 'position' | 'selection' | 'overview';
type Drag = { part: Part; x: number; rect: DOMRect; view: View; range: SourceRange; grab: number; moved: boolean };

export function RangeTimeline({ duration, range, position, disabled, onBoundary, onSeek, onMove, frameOnly = false, onStep, sourceLabel = 'video' }: {
  duration: number; range: SourceRange; position: number; disabled: boolean;
  onBoundary: (part: 'start' | 'end', value: number) => void;
  onSeek: (value: number) => void;
  onMove: (range: SourceRange) => void;
  frameOnly?: boolean;
  sourceLabel?: 'video' | 'animation';
  onStep?: (direction: -1 | 1) => void;
}) {
  const frame = (center: number, span: number): View => {
    span = clamp(span, Math.min(1, duration), duration);
    return { start: clamp(center - span / 2, 0, duration - span), span };
  };
  const fit = () => frame((range.start + range.end) / 2, Math.max(10, (range.end - range.start) * 3));
  const [view, setView] = useState(fit);
  const timeline = useRef<HTMLDivElement>(null);
  const overview = useRef<HTMLButtonElement>(null);
  const drag = useRef<Drag | null>(null);
  const previousRange = useRef(range);
  const [dragging, setDragging] = useState(false);
  const minSpan = Math.min(1, duration);
  const viewEnd = view.start + view.span;
  const percent = (time: number) => clamp((time - view.start) / view.span * 100, 0, 100);
  const hasOverview = duration > 10 || view.span < duration - 0.001;

  // Follow the edited boundary after typing, keyboard jumps, or releasing a drag.
  // Pointer geometry stays fixed during a detailed drag, so it never chases itself.
  useEffect(() => {
    if (dragging) return;
    const previous = previousRange.current;
    previousRange.current = range;
    const startChanged = previous.start !== range.start;
    const endChanged = previous.end !== range.end;
    if (!frameOnly && !startChanged && !endChanged) return;
    const point = frameOnly ? position : startChanged && endChanged ? (range.start + range.end) / 2 : startChanged ? range.start : range.end;
    if (point < view.start || point > viewEnd) {
      setView(frame(point, view.span));
    }
  }, [range.start, range.end, dragging, frameOnly, frameOnly ? position : null]); // View changes deliberately allow closer zoom.
  useEffect(() => { if (disabled) { drag.current = null; setDragging(false); } }, [disabled]);

  function move(start: number, original = range, follow = false) {
    const length = original.end - original.start;
    start = clamp(round(start), 0, Math.max(0, duration - length));
    const next = { start, end: start + length };
    onMove(next);
    if (follow) setView(frame((next.start + next.end) / 2, view.span));
  }
  function seek(event: PointerEvent) {
    const d = drag.current;
    if (disabled || !d) return;
    event.stopPropagation();
    d.moved ||= Math.abs(event.clientX - d.x) > 3;
    const time = d.view.start + (event.clientX - d.rect.left) / d.rect.width * d.view.span;
    if (d.part === 'overview' && frameOnly) { onSeek(clamp(time, 0, duration)); setView(frame(time, view.span)); }
    else if (d.part === 'overview') move(time - d.grab, d.range, true);
    else if (d.part === 'selection') {
      if (d.moved) move(d.range.start + (event.clientX - d.x) / d.rect.width * d.view.span, d.range);
    }
    else if (d.part === 'position') onSeek(clamp(time, 0, duration));
    else onBoundary(d.part, d.range[d.part] + (event.clientX - d.x) / d.rect.width * d.view.span);
  }
  function begin(event: PointerEvent<HTMLElement>, part: Part) {
    if (disabled || event.button !== 0) return;
    const rect = (part === 'overview' ? overview : timeline).current?.getBoundingClientRect();
    if (!rect) return;
    event.preventDefault(); event.stopPropagation(); event.currentTarget.focus();
    const scale = part === 'overview' ? { start: 0, span: duration } : view;
    const time = scale.start + (event.clientX - rect.left) / rect.width * scale.span;
    const inside = time >= range.start && time <= range.end;
    drag.current = { part, x: event.clientX, rect, view: scale, range: { ...range }, grab: inside ? time - range.start : (range.end - range.start) / 2, moved: false };
    setDragging(true);
    event.currentTarget.setPointerCapture(event.pointerId);
    if (part === 'position' || (part === 'overview' && (frameOnly || !inside))) seek(event);
    else if (part === 'start' || part === 'end') onSeek(range[part]);
  }
  function end(event: PointerEvent) {
    const d = drag.current;
    if (!d) return;
    event.stopPropagation(); drag.current = null; setDragging(false);
    if (d.part === 'selection' && !d.moved && event.type === 'pointerup') {
      onSeek(clamp(d.view.start + (event.clientX - d.rect.left) / d.rect.width * d.view.span, 0, duration));
    }
  }
  function keyboard(event: KeyboardEvent, part: Part, value: number) {
    if (disabled) return;
    if (frameOnly && part === 'position' && !event.shiftKey && event.key.startsWith('Arrow')) {
      event.preventDefault(); onStep?.(['ArrowRight', 'ArrowUp'].includes(event.key) ? 1 : -1); return;
    }
    const step = part === 'overview' ? (event.shiftKey ? 10 : 1) : event.shiftKey ? 1 : 0.01;
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? duration : ['ArrowRight', 'ArrowUp'].includes(event.key) ? value + step : ['ArrowLeft', 'ArrowDown'].includes(event.key) ? value - step : null;
    if (next == null) return;
    event.preventDefault();
    if (frameOnly && part === 'overview') { onSeek(clamp(next, 0, duration)); setView(frame(next, view.span)); }
    else if (part === 'selection' || part === 'overview') move(next, range, true);
    else if (part === 'position') { onSeek(clamp(next, 0, duration)); if (next < view.start || next > viewEnd) setView(frame(next, view.span)); }
    else onBoundary(part, next);
  }
  function zoom(factor: number) {
    setView(frame(clamp(position, view.start, viewEnd), view.span * factor));
  }
  const events = { onPointerMove: seek, onPointerUp: end, onPointerCancel: end, onLostPointerCapture: end };
  return <div className="range-navigation" data-view-start={view.start} data-view-end={viewEnd}>
    {hasOverview && <div className="range-overview-row"><span>Whole {sourceLabel}</span><button ref={overview} type="button" className="range-overview" role="slider" aria-label={frameOnly ? `Frame position in full ${sourceLabel}` : "Segment position in full video"} title={frameOnly ? "Click or drag to seek" : "Click or drag to move the selected segment through the video"} disabled={disabled} aria-valuemin={0} aria-valuemax={Math.max(0, duration - (range.end - range.start))} aria-valuenow={range.start} aria-valuetext={frameOnly ? `${position.toFixed(3)} seconds` : `${rangeTime(range.start)} to ${rangeTime(range.end)}`} onPointerDown={event => begin(event, 'overview')} onKeyDown={event => keyboard(event, 'overview', range.start)} {...events}>
      <span className="overview-track" /><span className="overview-window" style={{ left: `${view.start / duration * 100}%`, width: `${view.span / duration * 100}%` }} /><span className="overview-selection" style={{ left: `${range.start / duration * 100}%`, width: `${(range.end - range.start) / duration * 100}%` }} />
    </button></div>}
    <div className="range-scale"><span>{rangeTime(view.start)}</span><div>
      <button className="icon-button" aria-label="Zoom out timeline" title="Zoom out" disabled={disabled || view.span >= duration - 0.001} onClick={() => zoom(2)}><Minus size={13} /></button>
      <button className="icon-button" aria-label="Fit timeline to selection" title="Fit selection" disabled={disabled} onClick={() => setView(fit())}><Maximize2 size={12} /></button>
      <button className="icon-button" aria-label="Zoom in timeline" title="Zoom in" disabled={disabled || view.span <= minSpan + 0.001} onClick={() => zoom(0.5)}><Plus size={13} /></button>
    </div><span>{rangeTime(viewEnd)}</span></div>
    <div className="range-timeline" ref={timeline} onPointerDown={event => begin(event, 'position')} {...events}>
      <div className="range-track" />
      {!frameOnly && <button type="button" className="range-selection" role="slider" aria-label="Move selected range" title="Drag to move the selection; click to preview" aria-valuemin={0} aria-valuemax={Math.max(0, duration - (range.end - range.start))} aria-valuenow={range.start} aria-valuetext={`${rangeTime(range.start)} to ${rangeTime(range.end)}`} disabled={disabled} style={{ left: `${percent(range.start)}%`, width: `${percent(range.end) - percent(range.start)}%` }} onPointerDown={event => begin(event, 'selection')} onKeyDown={event => keyboard(event, 'selection', range.start)} {...events}><span /></button>}
      {!frameOnly && (['start', 'end'] as const).map(part => <button key={part} type="button" className="range-handle" role="slider" aria-label={`Range ${part}`} aria-valuemin={part === 'start' ? 0 : range.start + 0.01} aria-valuemax={part === 'start' ? range.end - 0.01 : duration} aria-valuenow={range[part]} aria-valuetext={rangeTime(range[part])} disabled={disabled} data-outside={range[part] < view.start || range[part] > viewEnd} style={{ left: `${percent(range[part])}%` }} onPointerDown={event => begin(event, part)} onKeyDown={event => keyboard(event, part, range[part])} {...events}><span /></button>)}
      <button type="button" className="range-playhead" role="slider" aria-label="Preview position" aria-valuemin={0} aria-valuemax={duration} aria-valuenow={position} aria-valuetext={frameOnly ? `${position.toFixed(3)} seconds` : rangeTime(position)} disabled={disabled} style={{ left: `${percent(position)}%` }} onPointerDown={event => begin(event, 'position')} onKeyDown={event => keyboard(event, 'position', position)} {...events}><span /></button>
    </div>
  </div>;
}
