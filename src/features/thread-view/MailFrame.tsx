import React, { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { buildShim } from './shim';
import { api } from '../../app/ipc/commands';
import mailCss from '../../styles/mail-frame.css?raw';

interface Props {
  messageId: string;
  html?: string;
  allowed: boolean;
  /**
   * The renderer's own verdict on the message's palette: true when the message
   * only paints white/transparent backgrounds. A dark app theme is only applied
   * on top of such a message — a newsletter with a baked-in dark palette stays
   * on a light surface, because "dark text on a dark background" is worse than
   * a bright frame (P9.2).
   */
  darkSafe: boolean;
}

/** Longest URL we forward to the native opener (P9.2). */
const MAX_URL_LENGTH = 2048;
/** Message heights are clamped to a finite, sane band before they size the frame. */
const MIN_FRAME_HEIGHT = 1;
const MAX_FRAME_HEIGHT = 20_000;

/**
 * Keys the frame may forward as list/reader navigation (P9.2). The shim already
 * restricts what it posts; this is the second, authoritative check, so a
 * crafted payload cannot reach the keymap engine and run an application
 * command from inside mail HTML.
 */
const FORWARDED_KEYS = new Set(['j', 'k', 'e', '#', 's', 'h', 'l', 'r', 'n', 'p', 'o', '/']);

function randomToken(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

/**
 * Effective app theme, tracked from the root element the settings store writes
 * to. `system` resolves to a concrete `data-theme`, so the attribute is the
 * authority and no media query is needed here.
 */
function useAppDarkTheme(): boolean {
  const read = () => document.documentElement.dataset.theme === 'dark';
  return useSyncExternalStore(
    (notify) => {
      const observer = new MutationObserver(notify);
      observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
      return () => observer.disconnect();
    },
    read,
    () => false,
  );
}

// Pooled iframes (2) are managed by the parent; this component is the frame itself.
export function MailFrame({ messageId, html, allowed, darkSafe }: Props) {
  const ref = useRef<HTMLIFrameElement>(null);
  const [height, setHeight] = useState<number | null>(null);
  const [hoverHref, setHoverHref] = useState<string | null>(null);
  // Both the CSP nonce and the frame token come from the platform CSPRNG and
  // live for exactly one frame (P9.2); a token is what ties a message event to
  // *this* frame, since the source check alone cannot distinguish two frames
  // that share an origin.
  const nonce = useMemo(randomToken, []);
  const token = useMemo(randomToken, []);
  const appDark = useAppDarkTheme();
  const surface = appDark && darkSafe ? 'dark' : 'light';

  const srcdoc = useMemo(() => {
    const remote = allowed ? ' http: https:' : '';
    // `sift-att:` is the app's own scheme for cached inline content: it is
    // local, never touches the network, and lets a cached body keep its CID
    // references instead of carrying base64 bytes through every IPC payload.
    const csp = `default-src 'none'; img-src data: blob: sift-att:${remote}; media-src data: blob: sift-att:${remote}; style-src 'unsafe-inline'${remote}; font-src data:${remote}; script-src 'nonce-${nonce}'; frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'`;
    return `<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="${csp}"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="referrer" content="no-referrer"><style>${mailCss}</style></head><body class="sift-mail sift-mail-${surface}">${html ?? ''}<script nonce="${nonce}">${buildShim(nonce, token)}</script></body></html>`;
  }, [allowed, html, nonce, token, surface]);

  useEffect(() => {
    setHeight(null);
    const h = (e: MessageEvent) => {
      const frame = ref.current?.contentWindow;
      // Exact source first: another window's message must be ignored outright.
      if (!frame || e.source !== frame) return;
      const data = e.data as
        | {
            __sift?: unknown;
            token?: unknown;
            type?: unknown;
            height?: unknown;
            href?: unknown;
            text?: unknown;
            key?: unknown;
            metaKey?: unknown;
            ctrlKey?: unknown;
            altKey?: unknown;
          }
        | null
        | undefined;
      if (!data || typeof data !== 'object' || data.__sift !== true) return;
      if (data.token !== token) return;
      switch (data.type) {
        case 'size': {
          const reported = Number(data.height);
          if (!Number.isFinite(reported)) return;
          setHeight(Math.min(MAX_FRAME_HEIGHT, Math.max(MIN_FRAME_HEIGHT, Math.ceil(reported))));
          return;
        }
        case 'link': {
          const href = typeof data.href === 'string' ? data.href : '';
          if (!href || href.length > MAX_URL_LENGTH) return;
          if (href.startsWith('javascript:')) return;
          if (href.startsWith('http://') || href.startsWith('https://') || href.startsWith('mailto:')) {
            // phishing heuristic: visible text looks like URL with different host
            const text = typeof data.text === 'string' ? data.text : '';
            try {
              const looksUrl = /https?:\/\//.test(text);
              if (looksUrl) {
                const h1 = new URL(href).host;
                const m = text.match(/https?:\/\/([^/\s)]+)/);
                if (m && m[1] !== h1) {
                  if (
                    !window.confirm(`This link shows “${text.slice(0, 60)}” but goes to ${h1}. Open anyway?`)
                  )
                    return;
                }
              }
            } catch {
              /* ignore */
            }
            void api.app_open_url(href);
          }
          return;
        }
        case 'hover': {
          const href = typeof data.href === 'string' && data.href.length <= MAX_URL_LENGTH ? data.href : null;
          setHoverHref(href);
          return;
        }
        case 'key': {
          // A forwarded key is navigation only: no modifiers, verified
          // whitelist. Anything else is dropped, not re-dispatched.
          if (typeof data.key !== 'string' || !FORWARDED_KEYS.has(data.key)) return;
          if (data.metaKey || data.ctrlKey || data.altKey) return;
          window.dispatchEvent(new KeyboardEvent('keydown', { key: data.key, bubbles: true }));
          return;
        }
        default:
          return;
      }
    };
    window.addEventListener('message', h);
    return () => window.removeEventListener('message', h);
  }, [messageId, srcdoc, token]);

  // Safety net: never leave the link preview stuck over the message.
  useEffect(() => {
    if (!hoverHref) return;
    const t = setTimeout(() => setHoverHref(null), 4000);
    return () => clearTimeout(t);
  }, [hoverHref]);

  return (
    <div style={{ position: 'relative' }}>
      <iframe
        ref={ref}
        title="Email"
        // Scripts are needed for the size shim only. Without `allow-same-origin`
        // the frame keeps an opaque origin, so mail HTML can never reach the app
        // document even if sanitizing ever missed something (P9.2).
        sandbox="allow-scripts"
        srcDoc={srcdoc}
        style={{
          width: '100%',
          maxWidth: '100%',
          height: height ?? 160,
          border: 'none',
          display: 'block',
          overflow: 'hidden',
          borderRadius: 8,
        }}
        scrolling="auto"
      />
      {hoverHref && (
        <div
          style={{
            position: 'absolute',
            bottom: 4,
            left: 4,
            background: 'var(--n10)',
            color: 'var(--n0)',
            fontSize: 11,
            padding: '2px 8px',
            borderRadius: 4,
            maxWidth: '80%',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            whiteSpace: 'nowrap',
          }}
        >
          {hoverHref}
        </div>
      )}
    </div>
  );
}
