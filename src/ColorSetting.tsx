import { useEffect, useId, useLayoutEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type PointerEvent } from 'react';
import { createPortal } from 'react-dom';
import { Check } from 'lucide-react';
import { hexToHsv, hsvToHex, parseHex, type HsvColor } from './color';

const swatches = [
  { name: 'White', hex: '#ffffff' }, { name: 'Black', hex: '#000000' },
  { name: 'Gray', hex: '#808080' }, { name: 'Sand', hex: '#e8ba62' },
  { name: 'Sage', hex: '#70aa86' }, { name: 'Blue', hex: '#6495ce' },
];
const focusableSelector = 'button:not(:disabled), input:not(:disabled), select:not(:disabled), a[href], [tabindex="0"]';

export function ColorSetting({ label, value, onChange, disabled = false }: { label: string; value: string; onChange: (value: string) => void; disabled?: boolean }) {
  const id = useId();
  const trigger = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const saturation = useRef<HTMLInputElement>(null);
  const [isOpen, setOpen] = useState(false);
  const [color, setColor] = useState(() => ({ hex: value, hsv: hexToHsv(value) }));
  const [draft, setDraft] = useState(value.toUpperCase());
  const [invalid, setInvalid] = useState(false);
  const [position, setPosition] = useState<CSSProperties>({ visibility: 'hidden' });
  // Keep externally replaced settings in sync without converting every HSV
  // movement back from rounded RGB (which would lose hue and thumb precision).
  if (color.hex !== value) setColor({ hex: value, hsv: hexToHsv(value, color.hsv) });
  const { hsv } = color;
  const open = isOpen && !disabled;

  function close(restoreFocus = false) {
    setOpen(false);
    if (restoreFocus) trigger.current?.focus({ preventScroll: true });
  }

  function changeHex(hex: string) {
    if (disabled || hex === value) return;
    setColor({ hex, hsv: hexToHsv(hex, hsv) });
    onChange(hex);
  }

  function changeHsv(next: HsvColor) {
    if (disabled) return;
    const hex = hsvToHex(next);
    setColor({ hex, hsv: next });
    setDraft(hex.toUpperCase());
    setInvalid(false);
    if (hex !== value) onChange(hex);
  }

  function commitDraft() {
    const hex = parseHex(draft);
    setInvalid(!hex);
    if (!hex) return false;
    changeHex(hex);
    setDraft(hex.toUpperCase());
    return true;
  }

  useEffect(() => { if (disabled) setOpen(false); }, [disabled]);

  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      if (!trigger.current || !panel.current) return;
      const rect = trigger.current.getBoundingClientRect();
      const width = Math.min(252, window.innerWidth - 16);
      const height = Math.min(panel.current.scrollHeight, window.innerHeight - 16);
      const below = rect.bottom + 6;
      let top = below + height <= window.innerHeight - 8 ? below : rect.top - height - 6;
      let left = rect.right - width;
      // In the short window neither vertical side may fit. Use the adjacent
      // space before clamping, keeping the trigger visible and reachable.
      if (top < 8) {
        if (rect.right + width + 16 <= window.innerWidth) left = rect.right + 8;
        else if (rect.left - width - 8 >= 8) left = rect.left - width - 8;
        top = rect.top + (rect.height - height) / 2;
      }
      setPosition({ width, maxHeight: window.innerHeight - 16, left: Math.max(8, Math.min(left, window.innerWidth - width - 8)), top: Math.max(8, Math.min(top, window.innerHeight - height - 8)), visibility: 'visible' });
    };
    place();
    const observer = new ResizeObserver(place);
    if (trigger.current) observer.observe(trigger.current);
    if (panel.current) observer.observe(panel.current);
    const onScroll = (event: Event) => { if (!panel.current?.contains(event.target as Node)) place(); };
    window.addEventListener('resize', place);
    window.addEventListener('scroll', onScroll, true);
    return () => { observer.disconnect(); window.removeEventListener('resize', place); window.removeEventListener('scroll', onScroll, true); };
  }, [open]);

  useLayoutEffect(() => {
    if (open && position.visibility === 'visible') saturation.current?.focus({ preventScroll: true });
  }, [open, position.visibility]);

  useEffect(() => {
    if (!open) return;
    const outside = (event: Event) => {
      if (!trigger.current?.contains(event.target as Node) && !panel.current?.contains(event.target as Node)) {
        const hex = parseHex(draft); if (hex) changeHex(hex);
        close();
      }
    };
    document.addEventListener('pointerdown', outside);
    document.addEventListener('focusin', outside);
    return () => { document.removeEventListener('pointerdown', outside); document.removeEventListener('focusin', outside); };
  }, [open, draft, hsv, disabled]);

  function handleKey(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === 'Escape') {
      event.preventDefault(); event.stopPropagation(); close(true);
    } else if (event.key === 'Tab' && panel.current) {
      const fields = [...panel.current.querySelectorAll<HTMLElement>(focusableSelector)];
      const edge = event.shiftKey ? fields[0] : fields.at(-1);
      if (event.target !== edge) return;
      // The portal is at the end of the DOM. Continue from the trigger's
      // position in the settings instead of trapping focus or jumping away.
      event.preventDefault();
      const outside = [...document.querySelectorAll<HTMLElement>(focusableSelector)].filter(element => !panel.current?.contains(element) && element.getClientRects().length);
      const index = outside.indexOf(trigger.current!);
      const next = event.shiftKey ? trigger.current : outside[index + 1] ?? trigger.current;
      close(); next?.focus();
    }
  }

  function moveInField(event: PointerEvent<HTMLDivElement>) {
    const rect = event.currentTarget.getBoundingClientRect();
    changeHsv({ ...hsv, s: Math.max(0, Math.min(100, (event.clientX - rect.left) / rect.width * 100)), v: Math.max(0, Math.min(100, (1 - (event.clientY - rect.top) / rect.height) * 100)) });
  }

  return <div className="setting-row color-row">
    <span className="setting-label"><span>{label}</span><small id={`${id}-hint`}>For transparent areas</small></span>
    <button type="button" ref={trigger} className="color-control" aria-label={`${label} color, ${value.toUpperCase()}`} aria-describedby={`${id}-hint`} aria-haspopup="dialog" aria-expanded={open} aria-controls={open ? `${id}-picker` : undefined} disabled={disabled} onClick={() => {
      if (open) close();
      else { setDraft(value.toUpperCase()); setInvalid(false); setOpen(true); }
    }}>
      <span className="color-swatch" style={{ backgroundColor: value }} aria-hidden="true" />
      <span aria-hidden="true">{value.toUpperCase()}</span>
    </button>
    {open && createPortal(<div ref={panel} id={`${id}-picker`} className="color-picker" role="dialog" aria-label={`${label} color`} style={position} onKeyDown={handleKey}>
      <div className="color-area-group">
        <div className="color-area" style={{ '--color-hue': `hsl(${hsv.h} 100% 50%)` } as CSSProperties} role="group" aria-label="Saturation and brightness" onPointerDown={event => {
          if (event.button !== 0 || !event.isPrimary) return;
          event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId);
          saturation.current?.focus({ preventScroll: true }); moveInField(event);
        }} onPointerMove={event => { if (event.currentTarget.hasPointerCapture(event.pointerId)) moveInField(event); }} onPointerUp={event => {
          if (event.currentTarget.hasPointerCapture(event.pointerId)) { moveInField(event); event.currentTarget.releasePointerCapture(event.pointerId); }
        }}>
          <span className="color-area-thumb" style={{ left: `${hsv.s}%`, top: `${100 - hsv.v}%`, backgroundColor: value }} aria-hidden="true" />
          <input ref={saturation} className="color-channel-input" type="range" min="0" max="100" step="1" aria-label="Saturation" aria-valuetext={`${Math.round(hsv.s)}%`} value={hsv.s} onChange={event => changeHsv({ ...hsv, s: Number(event.target.value) })} />
          <input className="color-channel-input" type="range" min="0" max="100" step="1" aria-label="Brightness" aria-valuetext={`${Math.round(hsv.v)}%`} value={hsv.v} onChange={event => changeHsv({ ...hsv, v: Number(event.target.value) })} />
        </div>
        <div className="color-channels" aria-hidden="true"><span>Saturation {Math.round(hsv.s)}%</span><span>Brightness {Math.round(hsv.v)}%</span></div>
      </div>
      <input className="color-hue" type="range" min="0" max="360" step="1" aria-label="Hue" aria-valuetext={`${Math.round(hsv.h)} degrees`} value={hsv.h} onChange={event => changeHsv({ ...hsv, h: Number(event.target.value) })} />
      <div className="color-presets" role="group" aria-label="Quick colors">{swatches.map(swatch => <button key={swatch.hex} type="button" aria-label={swatch.name} aria-pressed={value === swatch.hex} title={`${swatch.name} ${swatch.hex.toUpperCase()}`} style={{ backgroundColor: swatch.hex, color: swatch.hex === '#000000' || swatch.hex === '#808080' ? '#fff' : '#222' }} onClick={() => { changeHex(swatch.hex); setDraft(swatch.hex.toUpperCase()); setInvalid(false); }}>
        {value === swatch.hex && <Check size={14} strokeWidth={2.3} aria-hidden="true" />}
      </button>)}</div>
      <div className="color-picker-footer">
        <label className="color-hex"><span>Hex</span><input aria-label="Hex color" value={draft} maxLength={7} spellCheck={false} autoComplete="off" autoCapitalize="off" aria-invalid={invalid || undefined} aria-describedby={invalid ? `${id}-error` : undefined} onChange={event => {
          setDraft(event.target.value); setInvalid(false);
          const hex = parseHex(event.target.value, false); if (hex) changeHex(hex);
        }} onBlur={commitDraft} onKeyDown={event => {
          if (event.key === 'Enter') { event.preventDefault(); if (commitDraft()) close(true); }
        }} /></label>
        <button type="button" className="color-done" onClick={() => { if (commitDraft()) close(true); }}>Done</button>
      </div>
      {invalid && <p className="color-error" id={`${id}-error`} role="status">Use 3 or 6 hex digits, like #FFFFFF.</p>}
    </div>, document.body)}
  </div>;
}
