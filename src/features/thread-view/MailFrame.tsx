import React, { useEffect, useMemo, useRef, useState } from 'react';
import { buildShim } from './shim';
import { api } from '../../app/ipc/commands';
import mailCss from '../../styles/mail-frame.css?raw';

interface Props {
  messageId: string;
  html?: string;
  allowed: boolean;
  dark: boolean;
}

// Pooled iframes (2) are managed by the parent; this component is the frame itself.
export function MailFrame({ messageId, html, allowed }: Props) {
  const ref = useRef<HTMLIFrameElement>(null);
  const [height, setHeight] = useState<number | null>(null);
  const [hoverHref, setHoverHref] = useState<string | null>(null);
  const nonce = useMemo(() => Math.random().toString(36).slice(2), []);

  const srcdoc = useMemo(() => {
    const remote = allowed ? ' http: https:' : '';
    const csp = `default-src 'none'; img-src data: blob:${remote}; media-src data: blob:${remote}; style-src 'unsafe-inline'${remote}; font-src data:${remote}; script-src 'nonce-${nonce}'; frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'`;
    return `<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="${csp}"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="referrer" content="no-referrer"><style>${mailCss}</style></head><body class="sift-mail">${html ?? ''}<script nonce="${nonce}">${buildShim(nonce)}</script></body></html>`;
  }, [allowed, html, nonce]);

  useEffect(() => {
    setHeight(null);
    const h = (e: MessageEvent) => {
      if (e.source !== ref.current?.contentWindow || !e.data?.__sift) return;
      if (e.data.type === 'size' && typeof e.data.height === 'number') setHeight(e.data.height);
      if (e.data.type === 'link') {
        const href: string = e.data.href ?? '';
        if (href.startsWith('javascript:')) return;
        if (href.startsWith('http://') || href.startsWith('https://') || href.startsWith('mailto:')) {
          // phishing heuristic: visible text looks like URL with different host
          const text: string = e.data.text ?? '';
          try {
            const looksUrl = /https?:\/\//.test(text);
            if (looksUrl) {
              const h1 = new URL(href).host;
              const m = text.match(/https?:\/\/([^/\s)]+)/);
              if (m && m[1] !== h1) {
                if (!window.confirm(`This link shows “${text.slice(0, 60)}” but goes to ${h1}. Open anyway?`))
                  return;
              }
            }
          } catch {
            /* ignore */
          }
          void api.app_open_url(href);
        }
      }
      if (e.data.type === 'hover') setHoverHref(e.data.href);
      if (e.data.type === 'key' && typeof e.data.key === 'string') {
        window.dispatchEvent(new KeyboardEvent('keydown', { key: e.data.key, bubbles: true }));
      }
      if (e.data.type === 'image') {
        document.dispatchEvent(new CustomEvent('sift:load-remote', { detail: { messageId } }));
      }
    };
    window.addEventListener('message', h);
    return () => window.removeEventListener('message', h);
  }, [messageId, srcdoc]);

  return (
    <div style={{ position: 'relative' }}>
      <iframe
        ref={ref}
        title="Email"
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
