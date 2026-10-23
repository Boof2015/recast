import { useLayoutEffect, useRef, type ReactNode } from 'react';
import type { MediaKind } from './model';

export function GroupStack({ selected, children, onSizes }: { selected: MediaKind; children: ReactNode; onSizes?: (sizes: Partial<Record<MediaKind, number>>) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const stack = ref.current;
    if (!stack) return;
    const surface = stack.querySelector<HTMLElement>('.selection-surface')!;
    const update = () => {
      const header = stack.querySelector<HTMLElement>(`[data-selection-key="${selected}"]`);
      if (header) {
        surface.style.transform = `translateY(${header.getBoundingClientRect().top - stack.getBoundingClientRect().top}px)`;
        surface.style.height = `${header.offsetHeight}px`;
        surface.style.visibility = 'visible';
      }
      if (onSizes) {
        const sizes: Partial<Record<MediaKind, number>> = {};
        stack.querySelectorAll<HTMLElement>('[data-group-slot]').forEach((element) => { sizes[element.dataset.groupSlot as MediaKind] = element.offsetHeight; });
        onSizes(sizes);
      }
    };
    update();
    let frame = requestAnimationFrame(() => { frame = requestAnimationFrame(() => { stack.dataset.ready = 'true'; }); });
    const observer = new ResizeObserver(update);
    observer.observe(stack);
    for (const child of stack.children) if (child !== surface) observer.observe(child);
    return () => { observer.disconnect(); cancelAnimationFrame(frame); };
  }, [selected, children, onSizes]);
  return <div className="group-stack" ref={ref}><div className="selection-surface" aria-hidden="true" />{children}</div>;
}
