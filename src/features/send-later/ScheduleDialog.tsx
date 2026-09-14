import React, { useEffect, useMemo, useState } from 'react';
import { Dialog } from '../../ui/Dialog';
import { Button } from '../../ui/Button';
import { Switch } from '../../ui/Switch';
import {
  SEND_LATER_COPY,
  formatInZone,
  localWallTime,
  nextOccurrence,
  resolveLocalWallTime,
  timezoneName,
  tomorrowMorning,
} from '../mail-utilities/time';

/** What the composer and the outbox both need to persist a schedule (P8.1). */
export interface ScheduleChoice {
  notBefore: number;
  /** The exact intended local wall time, `YYYY-MM-DDTHH:MM`. */
  localTime: string;
  /** The IANA zone that wall time belongs to. */
  timezone: string;
  archiveAfterSend: boolean;
}

/**
 * The smallest scheduling surface that is still honest.
 *
 * There is no date-picker dependency and no calendar: a native
 * `datetime-local` input plus two presets, with the resolved instant, the zone
 * it belongs to and the "Sift must be running and online" consequence all
 * visible before anything is queued (P8.1).
 */
export function ScheduleDialog({
  open,
  onClose,
  onConfirm,
  initialLocal,
  archiveDefault = false,
  title = 'Send later',
  submitLabel = 'Schedule send',
}: {
  open: boolean;
  onClose: () => void;
  onConfirm: (choice: ScheduleChoice) => void;
  /** Pre-filled wall time when moving an existing schedule. */
  initialLocal?: string;
  archiveDefault?: boolean;
  title?: string;
  submitLabel?: string;
}) {
  const [draft, setDraft] = useState(initialLocal ?? '');
  const [archive, setArchive] = useState(archiveDefault);
  const [error, setError] = useState<string | null>(null);
  const zone = useMemo(timezoneName, []);

  // Each open starts from the caller's value, never from the previous schedule.
  useEffect(() => {
    if (!open) return;
    setDraft(initialLocal ?? localWallTime(tomorrowMorning(new Date())));
    setArchive(archiveDefault);
    setError(null);
  }, [open, initialLocal, archiveDefault]);

  const resolution = draft ? resolveLocalWallTime(draft) : null;
  const tooSoon = !!resolution && resolution.when.getTime() <= Date.now() + 5000;

  const confirm = () => {
    if (!resolution) {
      setError('Choose a date and time');
      return;
    }
    if (tooSoon) {
      setError('That time has already passed — choose a future time');
      return;
    }
    onConfirm({
      notBefore: resolution.when.getTime(),
      localTime: localWallTime(resolution.when),
      timezone: zone,
      archiveAfterSend: archive,
    });
  };

  return (
    <Dialog open={open} onClose={onClose} title={title} width={440}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
        <div style={{ display: 'flex', gap: 8 }}>
          <Button size="sm" onClick={() => setDraft(localWallTime(tomorrowMorning(new Date())))}>
            Tomorrow 8:00
          </Button>
          <Button size="sm" onClick={() => setDraft(localWallTime(nextOccurrence(new Date(), 1, 18)))}>
            Tomorrow 18:00
          </Button>
          <Button size="sm" onClick={() => setDraft(localWallTime(nextOccurrence(new Date(), 7, 8)))}>
            Next week
          </Button>
        </div>
        <label style={{ display: 'flex', flexDirection: 'column', gap: 4, fontSize: 13 }}>
          Send at
          <input
            type="datetime-local"
            value={draft}
            onChange={(e) => {
              setDraft(e.target.value);
              setError(null);
            }}
            aria-label="Send at"
            data-testid="schedule-input"
            style={{
              height: 32,
              border: '1px solid var(--border-strong)',
              borderRadius: 6,
              padding: '0 10px',
              background: 'var(--bg-raised)',
              color: 'var(--fg)',
            }}
          />
        </label>
        {resolution && (
          <div
            style={{ fontSize: 12.5, color: 'var(--fg-2)' }}
            data-testid="schedule-resolved"
            role="status"
            aria-live="polite"
          >
            Sends {formatInZone(resolution.when.getTime(), zone)} · {zone}
            {/* A wall time inside the spring-forward gap does not exist in this
                zone, so say which instant will actually be used rather than
                silently sending an hour later (P8.1). */}
            {!resolution.exact && (
              <div style={{ color: 'var(--warning)', marginTop: 4 }}>
                {draft.replace('T', ' ')} does not exist in {zone} that day; this sends at{' '}
                {resolution.effective.replace('T', ' ')} instead.
              </div>
            )}
          </div>
        )}
        <Switch checked={archive} onChange={setArchive} label="Archive the conversation after sending" />
        <div
          style={{
            fontSize: 12,
            color: 'var(--fg-3)',
            borderTop: '1px solid var(--border)',
            paddingTop: 10,
          }}
        >
          {SEND_LATER_COPY}
        </div>
        {error && (
          <div role="alert" style={{ fontSize: 12.5, color: 'var(--danger)' }}>
            {error}
          </div>
        )}
        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8 }}>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" onClick={confirm} disabled={!resolution || tooSoon}>
            {submitLabel}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
