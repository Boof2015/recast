import { useEffect, useId, useLayoutEffect, useRef, useState, type CSSProperties, type KeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown } from 'lucide-react';

export function ColorSetting({ label, value, onChange }: { label: string; value: string; onChange: (value: string) => void }) {
  const hint = useId();
  return <label className="setting-row color-row">
    <span className="setting-label"><span>{label}</span><small id={hint}>For transparent areas</small></span>
    <span className="color-control">
      <span className="color-swatch" style={{ backgroundColor: value }} aria-hidden="true" />
      <span aria-hidden="true">{value.toUpperCase()}</span>
      <input type="color" aria-label={`${label} color`} aria-describedby={hint} value={value} onChange={event => onChange(event.target.value)} />
    </span>
  </label>;
}

export function CheckboxSetting({ label, checked, onChange, disabled = false }: { label: string; checked: boolean; onChange: (checked: boolean) => void; disabled?: boolean }) {
  return <label className={`setting-row checkbox-row ${disabled ? 'is-disabled' : ''}`}>
    <span>{label}</span>
    <span className="checkbox-control">
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(event) => onChange(event.target.checked)} />
      <Check className="checkbox-check" size={14} strokeWidth={2.5} aria-hidden="true" />
    </span>
  </label>;
}

export function QualitySetting({ value, disabled, onChange }: { value: number; disabled: boolean; onChange: (value: number) => void }) {
  const id = useId();
  const thumbSize = 14;
  const progress = (value - 1) / 99;
  // Native thumbs travel from r to width-r, rather than 0 to width. The
  // gradient ends at that exact center, including both endpoint values.
  const style = {
    '--slider-size': `${thumbSize}px`,
    '--slider-fill-stop': `calc(${progress * 100}% + ${(0.5 - progress) * thumbSize}px)`,
  } as CSSProperties;
  return <div className={`quality-setting ${disabled ? 'is-disabled' : ''}`}>
    <div className="setting-row"><label htmlFor={id}>Quality</label><output htmlFor={id}>{value}</output></div>
    <input id={id} type="range" min="1" max="100" value={value} disabled={disabled} onChange={(event) => onChange(Number(event.target.value))} style={style} />
  </div>;
}

export function SelectSetting({ label, value, options, onChange }: { label: string; value: string; options: string[]; onChange: (value: string) => void }) {
  const id = useId();
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const search = useRef({ text: '', time: 0 });
  const [isOpen, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const [position, setPosition] = useState<CSSProperties>({ visibility: 'hidden' });

  function open(index = options.indexOf(value)) {
    setActive(Math.max(0, index));
    search.current = { text: '', time: 0 };
    setOpen(true);
  }

  function commit(index = active) {
    if (options[index] !== undefined) onChange(options[index]);
    setOpen(false);
  }

  useLayoutEffect(() => {
    if (!isOpen) return;
    const place = () => {
      if (!trigger.current || !menu.current) return;
      const rect = trigger.current.getBoundingClientRect();
      const width = Math.min(Math.max(146, rect.width), window.innerWidth - 16);
      const height = Math.min(menu.current.scrollHeight, window.innerHeight - 16);
      const below = window.innerHeight - rect.bottom - 14;
      const above = rect.top - 14;
      const placeBelow = below >= height || below >= above;
      const maxHeight = Math.max(40, placeBelow ? below : above);
      setPosition({
        width,
        maxHeight,
        left: Math.max(8, Math.min(rect.right - width, window.innerWidth - width - 8)),
        top: placeBelow ? rect.bottom + 6 : Math.max(8, rect.top - Math.min(height, maxHeight) - 6),
        visibility: 'visible',
      });
    };
    place();
    const observer = new ResizeObserver(place);
    if (trigger.current) observer.observe(trigger.current);
    window.addEventListener('resize', place);
    // The menu is outside scroll masks, but still follows its field on scroll.
    const onScroll = (event: Event) => { if (event.target !== menu.current) place(); };
    window.addEventListener('scroll', onScroll, true);
    return () => { observer.disconnect(); window.removeEventListener('resize', place); window.removeEventListener('scroll', onScroll, true); };
  }, [isOpen]);

  useEffect(() => {
    if (isOpen) menu.current?.children[active]?.scrollIntoView({ block: 'nearest' });
  }, [active, isOpen]);

  useEffect(() => {
    if (!isOpen) return;
    const dismiss = (event: PointerEvent) => {
      if (!trigger.current?.contains(event.target as Node) && !menu.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener('pointerdown', dismiss);
    return () => document.removeEventListener('pointerdown', dismiss);
  }, [isOpen]);

  function handleKey(event: KeyboardEvent<HTMLButtonElement>) {
    const key = event.key;
    if (key === 'Tab') { if (isOpen) commit(); return; }
    if (key === 'Escape') {
      if (isOpen) { event.preventDefault(); event.stopPropagation(); setOpen(false); }
      return;
    }
    if (key === 'Enter' || key === ' ') {
      event.preventDefault();
      if (isOpen) commit(); else open();
    } else if (key === 'ArrowDown' || key === 'ArrowUp') {
      event.preventDefault();
      if (event.altKey && key === 'ArrowUp' && isOpen) commit();
      else if (!isOpen) open();
      else setActive((index) => Math.max(0, Math.min(options.length - 1, index + (key === 'ArrowDown' ? 1 : -1))));
    } else if (['Home', 'End', 'PageUp', 'PageDown'].includes(key)) {
      event.preventDefault();
      const index = key === 'Home' ? 0 : key === 'End' ? options.length - 1 : Math.max(0, Math.min(options.length - 1, active + (key === 'PageDown' ? 10 : -10)));
      if (isOpen) setActive(index); else open(index);
    } else if (key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey) {
      event.preventDefault();
      const now = performance.now();
      const typed = now - search.current.time < 700 ? search.current.text + key.toLowerCase() : key.toLowerCase();
      const repeated = [...typed].every((character) => character === typed[0]);
      const prefix = repeated ? typed[0] : typed;
      const start = isOpen ? active : options.indexOf(value);
      const order = options.map((_, index) => (start + (repeated ? 1 : 0) + index + options.length) % options.length);
      const match = order.find((index) => options[index].toLowerCase().startsWith(prefix));
      if (!isOpen) open(match ?? options.indexOf(value));
      else if (match !== undefined) setActive(match);
      search.current = { text: typed, time: now };
    }
  }

  return <div className="setting-row select-setting">
    <span id={`${id}-label`}>{label}</span>
    <button type="button" ref={trigger} className="select-trigger" role="combobox" aria-labelledby={`${id}-label`} aria-expanded={isOpen} aria-controls={isOpen ? `${id}-list` : undefined} aria-haspopup="listbox" aria-activedescendant={isOpen ? `${id}-option-${active}` : undefined} onKeyDown={handleKey} onClick={() => isOpen ? setOpen(false) : open()} onBlur={() => { if (isOpen) commit(); }}>
      <span>{value}</span><ChevronDown size={14} aria-hidden="true" />
    </button>
    {isOpen && createPortal(<div ref={menu} id={`${id}-list`} className="select-menu" role="listbox" aria-labelledby={`${id}-label`} style={position}>
      {options.map((option, index) => <div key={option} id={`${id}-option-${index}`} className="select-option" role="option" aria-selected={option === value} data-active={index === active} onPointerMove={() => setActive(index)} onPointerDown={(event) => event.preventDefault()} onClick={() => { commit(index); trigger.current?.focus(); }}>
        <span>{option}</span><Check className="option-check" size={14} strokeWidth={2.2} aria-hidden="true" />
      </div>)}
    </div>, document.body)}
  </div>;
}
