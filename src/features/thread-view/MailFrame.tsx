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
export function MailFrame({ messageId, html, allowed, dark }: Props) {
  const ref = useRef<HTMLIFrameElement>(null);
  const [height, setHeight] = useState<number | null>(null);
  const [hoverHref, setHoverHref] = useState<string | null>(null);
  const nonce = useMemo(() => Math.random().toString(36).slice(2), []);

  const srcdoc = useMemo(() => {
    const csp = `default-src 'none'; img-src data: sift-att:${allowed ? ' https:' : ''}; style-src 'unsafe-inline'; script-src 'nonce-${nonce}'; font-src 'none'; form-action 'none'; base-uri 'none'`;
    return `<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="${csp}"><style>${mailCss}</style></head><body class="sift-mail${dark ? ' dark' : ''}">${html ?? ''}<script nonce="${nonce}">${buildShim(nonce)}</script></body></html>`;
  }, [html, allowed, dark, nonce]);

  useEffect(() => {
    setHeight(null);
    const h = (e: MessageEvent) => {
      if (!e.data?.__sift) return;
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
        title={`message-${messageId}`}
        sandbox="allow-scripts"
        srcDoc={srcdoc}
        style={{
          width: '100%',
          height: height ?? 120,
          border: 'none',
          visibility: height ? 'visible' : 'hidden',
          maxHeight: 2000,
        }}
        scrolling="no"
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
