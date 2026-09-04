import React from 'react';

type Variant = 'primary' | 'secondary' | 'ghost' | 'danger';
type Size = 'sm' | 'md';

export function Button({
  variant = 'secondary',
  size = 'md',
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; size?: Size }) {
  const v: Record<Variant, string> = {
    primary: 'background:var(--accent);color:var(--fg-on-accent);border:1px solid transparent;',
    secondary: 'background:var(--n0);color:var(--fg);border:1px solid var(--border-strong);',
    ghost: 'background:transparent;color:var(--fg);border:1px solid transparent;',
    danger: 'background:var(--danger);color:#fff;border:1px solid transparent;',
  };
  const s: Record<Size, string> = {
    sm: 'height:28px;padding:0 10px;font-size:12px;border-radius:var(--r-sm);',
    md: 'height:32px;padding:0 14px;font-size:13px;border-radius:var(--r-md);',
  };
  return (
    <button
      {...props}
      style={{
        ...(props.style as object),
        transition: 'transform 120ms var(--ease-out), background 120ms ease',
        cursor: 'pointer',
        fontWeight: 500,
        ...({} as object),
      }}
      css-text={`${v[variant]}${s[size]}`}
      className={`sift-btn sift-btn-${variant} ${props.className ?? ''}`}
      onMouseDown={(e) => {
        (e.currentTarget as HTMLElement).style.transform = 'scale(0.97)';
        props.onMouseDown?.(e);
      }}
      onMouseUp={(e) => {
        (e.currentTarget as HTMLElement).style.transform = '';
        props.onMouseUp?.(e);
      }}
    />
  );
}
