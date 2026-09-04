import { format } from 'date-fns';

export function formatRowDate(ts: number, now = Date.now()): string {
  const d = new Date(ts);
  const n = new Date(now);
  const sameDay = d.toDateString() === n.toDateString();
  if (sameDay) {
    const use24 = !/AM|PM/i.test(new Date().toLocaleTimeString());
    return use24 ? format(d, 'HH:mm') : format(d, 'h:mm a').replace(' ', '');
  }
  const weekAgo = now - 7 * 86400000;
  if (ts > weekAgo) return format(d, 'EEE');
  if (d.getFullYear() === n.getFullYear()) return format(d, 'd MMM');
  return format(d, 'd MMM yyyy');
}

export function formatSnooze(ts: number): string {
  return `Until ${format(new Date(ts), 'EEE h:mm a')}`;
}
