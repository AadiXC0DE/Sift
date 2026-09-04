import React from 'react';
export function Skeleton({ h = 12, w = '100%' }: { h?: number; w?: number | string }) {
  return <div style={{ height: h, width: w, background: 'var(--n2)', borderRadius: 4 }} />;
}
