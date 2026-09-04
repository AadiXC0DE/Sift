import React from 'react';
export function Spinner({ size = 14 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      style={{ animation: 'sift-spin 0.6s linear infinite' }}
      aria-label="Loading"
    >
      <style>{'@keyframes sift-spin { to { transform: rotate(360deg); } }'}</style>
      <circle cx="8" cy="8" r="6.5" fill="none" stroke="var(--n4)" strokeWidth="2" />
      <path
        d="M14.5 8a6.5 6.5 0 0 0-6.5-6.5"
        fill="none"
        stroke="var(--fg-3)"
        strokeWidth="2"
        strokeLinecap="round"
      />
    </svg>
  );
}
