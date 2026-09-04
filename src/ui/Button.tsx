import React, { forwardRef } from 'react';

type Variant = 'primary' | 'secondary' | 'ghost' | 'danger';
type Size = 'sm' | 'md';

const variants: Record<Variant, React.CSSProperties> = {
  primary: {
    background: 'var(--accent)',
    color: 'var(--fg-on-accent)',
    border: '1px solid transparent',
  },
  secondary: {
    background: 'var(--bg-raised)',
    color: 'var(--fg)',
    border: '1px solid var(--border-strong)',
  },
  ghost: {
    background: 'transparent',
    color: 'var(--fg)',
    border: '1px solid transparent',
  },
  danger: {
    background: 'var(--danger)',
    color: 'var(--fg-on-accent)',
    border: '1px solid transparent',
  },
};

const sizes: Record<Size, React.CSSProperties> = {
  sm: { height: 28, padding: '0 10px', fontSize: 12, borderRadius: 'var(--r-sm)' },
  md: { height: 32, padding: '0 14px', fontSize: 13, borderRadius: 'var(--r-md)' },
};

export const Button = forwardRef<
  HTMLButtonElement,
  React.ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; size?: Size }
>(function Button({ variant = 'secondary', size = 'md', ...props }, ref) {
  return (
    <button
      {...props}
      ref={ref}
      className={`sift-btn sift-btn-${variant} ${props.className ?? ''}`}
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        fontWeight: 500,
        cursor: props.disabled ? 'default' : 'pointer',
        opacity: props.disabled ? 0.5 : 1,
        ...variants[variant],
        ...sizes[size],
        transition: 'transform 120ms var(--ease-out), background 80ms ease',
        ...props.style,
      }}
      onMouseDown={(e) => {
        if (!props.disabled) (e.currentTarget as HTMLElement).style.transform = 'scale(0.97)';
        props.onMouseDown?.(e);
      }}
      onMouseUp={(e) => {
        (e.currentTarget as HTMLElement).style.transform = '';
        props.onMouseUp?.(e);
      }}
      onMouseLeave={(e) => {
        (e.currentTarget as HTMLElement).style.transform = '';
        props.onMouseLeave?.(e);
      }}
    />
  );
});
