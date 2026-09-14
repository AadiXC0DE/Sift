/**
 * Wall-clock arithmetic for Send Later and reminders (P8.1/P8.2).
 *
 * Scheduling is the one place where "what the user meant" and "what a UTC
 * timestamp says" can disagree: a DST transition either repeats an hour or
 * skips one, and the user picks a time in the repeated/skipped window. We
 * therefore never persist a bare timestamp — the exact local wall time and the
 * IANA zone travel with it, and the UI always shows the zone it will use.
 */
import { format } from 'date-fns';

/** The device zone, as an IANA name. Falls back to UTC on an exotic runtime. */
export function timezoneName(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
  } catch {
    return 'UTC';
  }
}

/** A `Date` as `YYYY-MM-DDTHH:MM` in local time — the `<input type="datetime-local">` value. */
export function localWallTime(d: Date): string {
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export interface LocalTimeResolution {
  /** The instant the wall time maps to. */
  when: Date;
  /**
   * False when the wall time does not exist in this zone (the spring-forward
   * gap): the browser silently shifts it, and the user must be told which
   * instant the schedule will actually use.
   */
  exact: boolean;
  /** The local wall time the instant really renders as; `local` when exact. */
  effective: string;
}

/**
 * Resolve a local wall time to an instant.
 *
 * In a DST gap `new Date('2026-03-08T02:30')` becomes 03:30, so `exact` is
 * false and `effective` reports the shifted wall time. In a repeated hour the
 * platform picks one of the two offsets; that is honest — the UTC instant is
 * unambiguous — and `offsetLabel` is what the UI shows beside it.
 */
export function resolveLocalWallTime(local: string): LocalTimeResolution | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/.exec(local.trim());
  if (!m) return null;
  const [, y, mo, d, h, mi] = m;
  const month = Number(mo) - 1;
  const when = new Date(Number(y), month, Number(d), Number(h), Number(mi), 0, 0);
  if (Number.isNaN(when.getTime())) return null;
  if (when.getMonth() !== month || when.getDate() !== Number(d)) return null;
  const effective = localWallTime(when);
  return { when, exact: effective === local, effective };
}

/** Local 08:00 on the day after `now`. */
export function tomorrowMorning(now = new Date(), hour = 8): Date {
  const d = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1, hour, 0, 0, 0);
  return d;
}

/** Local `hour:00` on the next occurrence of `daysAhead` from today. */
export function nextOccurrence(now: Date, daysAhead: number, hour: number): Date {
  return new Date(now.getFullYear(), now.getMonth(), now.getDate() + daysAhead, hour, 0, 0, 0);
}

/** UTC offset of an instant as `UTC+2` / `UTC-05:30`. */
export function offsetLabel(ms: number): string {
  const minutes = -new Date(ms).getTimezoneOffset();
  const sign = minutes < 0 ? '-' : '+';
  const abs = Math.abs(minutes);
  const h = Math.floor(abs / 60);
  const m = abs % 60;
  return `UTC${sign}${h}${m ? `:${String(m).padStart(2, '0')}` : ''}`;
}

/**
 * Render an instant in a named IANA zone. A zone the runtime does not know
 * falls back to the device zone rather than throwing while formatting a row.
 */
export function formatInZone(ms: number, timeZone: string): string {
  const d = new Date(ms);
  try {
    return new Intl.DateTimeFormat(undefined, {
      weekday: 'short',
      day: 'numeric',
      month: 'short',
      hour: '2-digit',
      minute: '2-digit',
      timeZone,
      timeZoneName: 'short',
    }).format(d);
  } catch {
    return `${format(d, 'EEE d MMM HH:mm')} (${timezoneName()})`;
  }
}

/**
 * The copy that must sit beside scheduling (P8.1): Sift has no cloud
 * scheduler, so an overdue send goes out when the app is next running online.
 */
export const SEND_LATER_COPY =
  'Sift must be running and online. Otherwise this sends when Sift next connects.';

/** Short label for a schedule, used in the composer chip and the outbox row. */
export function describeSchedule(ms: number, timeZone: string): string {
  return `Sends ${formatInZone(ms, timeZone)} (${timeZone})`;
}
