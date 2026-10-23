export type HsvColor = { h: number; s: number; v: number };

export function parseHex(text: string, allowShort = true): string | null {
  const hex = text.trim().replace(/^#/, '');
  if (/^[\da-f]{6}$/i.test(hex)) return `#${hex.toLowerCase()}`;
  if (allowShort && /^[\da-f]{3}$/i.test(hex)) return `#${[...hex].map(part => part + part).join('').toLowerCase()}`;
  return null;
}

export function hexToHsv(hex: string, previous: HsvColor = { h: 0, s: 0, v: 100 }): HsvColor {
  const [r, g, b] = [1, 3, 5].map(start => parseInt(hex.slice(start, start + 2), 16) / 255);
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const delta = max - min;
  // Hue has no RGB representation in a gray, and saturation has none in
  // black. Keep those choices so dragging out of white/black feels continuous.
  if (max === 0) return { ...previous, v: 0 };
  const hue = delta === 0 ? previous.h : 60 * (max === r ? ((g - b) / delta + 6) % 6 : max === g ? (b - r) / delta + 2 : (r - g) / delta + 4);
  return { h: hue, s: delta / max * 100, v: max * 100 };
}

export function hsvToHex({ h, s, v }: HsvColor): string {
  const chroma = v / 100 * s / 100;
  const sector = (h % 360) / 60;
  const x = chroma * (1 - Math.abs(sector % 2 - 1));
  const channels = sector < 1 ? [chroma, x, 0] : sector < 2 ? [x, chroma, 0] : sector < 3 ? [0, chroma, x] : sector < 4 ? [0, x, chroma] : sector < 5 ? [x, 0, chroma] : [chroma, 0, x];
  return `#${channels.map(channel => Math.round((channel + v / 100 - chroma) * 255).toString(16).padStart(2, '0')).join('')}`;
}
