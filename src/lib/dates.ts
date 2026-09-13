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

/**
 * Truthful "how long ago" for a past event (P3.6/P4.6).
 *
 * The empty inbox used to claim "Last synced just now" unconditionally. This
 * says how old the timestamp actually is, and only says "just now" inside the
 * minute.
 */
export function relativeTime(ts: number, now = Date.now()): string {
  const diff = Math.max(0, now - ts);
  if (diff < 60_000) return 'just now';
  const minutes = Math.round(diff / 60_000);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  return `${Math.round(hours / 24)} d ago`;
}
