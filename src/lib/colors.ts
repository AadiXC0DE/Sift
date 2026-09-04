const hues: Record<string, string> = {
  blue: '#2f6df6',
  indigo: '#5b5bd6',
  violet: '#7c5cdb',
  rose: '#d6456d',
  orange: '#e0742a',
  green: '#2c9a5a',
  teal: '#1f9a94',
  graphite: '#55554f',
};
export function accentHex(key: string): string {
  return hues[key] ?? hues.blue;
}
const gmailMap: Record<string, string> = {};
export function gmailColorToHue(bg?: string | null): string {
  if (!bg) return 'blue';
  const l = bg.toLowerCase();
  if (l.includes('red') || l.includes('rose') || l.includes('pink')) return 'rose';
  if (l.includes('orange') || l.includes('tangerine')) return 'orange';
  if (l.includes('green')) return 'green';
  if (l.includes('teal') || l.includes('cyan')) return 'teal';
  if (l.includes('purple') || l.includes('violet') || l.includes('grape')) return 'violet';
  if (l.includes('indigo') || l.includes('blueberry')) return 'indigo';
  if (l.includes('gray') || l.includes('grey')) return 'graphite';
  void gmailMap;
  return 'blue';
}
