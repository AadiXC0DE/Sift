import type { OperationState } from '../app/ipc/types';

/**
 * The typed error every Phase 6 command rejects with (P6.1/P6.2/P6.6):
 * `{ code, message, retryable, detail? }`. `detail` is only present on state
 * errors (`send_undo_expired`, `acknowledge_duplicate_risk`, `not_retryable`,
 * `draft_queued`).
 *
 * The shape is read defensively because a rejection crosses the IPC boundary:
 * a Tauri command that returns `Err` may surface as an object or as its JSON
 * string depending on the error type. Both are normalized here, and a plain
 * string that is not JSON becomes `unknown_error` with the original text as the
 * message — never silently dropped.
 */
export interface SiftErrorDetail {
  state?: OperationState;
  opId?: number;
  notBefore?: number;
  failures?: { accountId: string; code: string; message: string }[];
  [key: string]: unknown;
}

export interface SiftError {
  code: string;
  message: string;
  retryable: boolean;
  detail?: SiftErrorDetail;
}

function asObject(value: unknown): Record<string, unknown> | null {
  if (typeof value === 'object' && value !== null) return value as Record<string, unknown>;
  if (typeof value === 'string') {
    const text = value.trim();
    if (text.startsWith('{')) {
      try {
        const parsed: unknown = JSON.parse(text);
        if (typeof parsed === 'object' && parsed !== null) return parsed as Record<string, unknown>;
      } catch {
        // A non-JSON message is handled by the caller as prose.
      }
    }
  }
  return null;
}

export function readSiftError(e: unknown): SiftError {
  const raw = asObject(e);
  if (raw) {
    const detail = asObject(raw.detail);
    return {
      code: typeof raw.code === 'string' ? raw.code : 'unknown_error',
      message:
        typeof raw.message === 'string'
          ? raw.message
          : typeof raw.code === 'string'
            ? raw.code
            : 'Something went wrong',
      retryable: raw.retryable === true,
      ...(detail ? { detail: detail as SiftErrorDetail } : {}),
    };
  }
  if (e instanceof Error) return { code: 'unknown_error', message: e.message, retryable: false };
  if (typeof e === 'string' && e) return { code: 'unknown_error', message: e, retryable: false };
  return { code: 'unknown_error', message: 'Something went wrong', retryable: false };
}
