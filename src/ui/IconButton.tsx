import React from 'react';
import { Tooltip } from './Tooltip';

export function IconButton({
  tip,
  size = 'md',
  children,
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { tip: string; size?: 'sm' | 'md' }) {
  const dim = size === 'sm' ? 28 : 32;
  const btn = (
    <button
      {...props}
      aria-label={tip}
      title={tip}
      className={`sift-iconbtn ${props.className ?? ''}`}
      style={{
        width: dim,
        height: dim,
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        borderRadius: 'var(--r-md)',
        border: '1px solid transparent',
        background: 'transparent',
        color: 'var(--fg-2)',
        cursor: 'pointer',
        transition: 'transform 120ms var(--ease-out), background 80ms ease',
        ...(props.style as object),
      }}
    >
      {children}
    </button>
  );
  return <Tooltip content={tip}>{btn}</Tooltip>;
}
