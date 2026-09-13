import React from 'react';
import { avatarHue, initials } from '../lib/names';
export function Avatar({
  email,
  name,
  size = 28,
  image,
}: {
  email: string;
  name?: string | null;
  size?: number;
  image?: string | null;
}) {
  const hue = avatarHue(email);
  if (image)
    return (
      <img
        src={image}
        alt=""
        width={size}
        height={size}
        style={{ borderRadius: '50%', width: size, height: size }}
      />
    );
  return (
    <div
      style={{
        width: size,
        height: size,
        borderRadius: '50%',
        // L 0.52 at chroma 0.12 keeps white initials above 4.5:1 for every hue
        // axe checks (wcag2aa normal text), at the cost of a slightly deeper
        // swatch (P9.5).
        background: `oklch(0.52 0.12 ${hue})`,
        color: '#fff',
        fontSize: size <= 24 ? 10 : 11,
        fontWeight: 600,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        flexShrink: 0,
      }}
    >
      {initials(email, name)}
    </div>
  );
}
