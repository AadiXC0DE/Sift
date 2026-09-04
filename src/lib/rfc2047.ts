/** Decode RFC 2047 encoded-words (`=?UTF-8?Q?...?=`) for subjects already in the DB. */
export function decodeRfc2047(input: string): string {
  if (!input || !input.includes('=?')) return input;
  const re = /=\?([^?]+)\?([bBqQ])\?([^?]*)\?=\s*/g;
  let out = '';
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(input))) {
    out += input.slice(last, m.index);
    last = m.index + m[0].length;
    const charset = m[1].toLowerCase();
    const enc = m[2].toUpperCase();
    const data = m[3];
    try {
      const bytes = enc === 'B' ? b64(data) : qBytes(data);
      out += decodeBytes(bytes, charset);
    } catch {
      out += m[0];
    }
  }
  out += input.slice(last);
  return out.replace(/\s+/g, ' ').trim() || input;
}

function b64(data: string): Uint8Array {
  const bin = atob(data.replace(/\s+/g, ''));
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function qBytes(data: string): Uint8Array {
  const s = data.replace(/_/g, ' ');
  const out: number[] = [];
  for (let i = 0; i < s.length; i++) {
    if (s[i] === '=' && i + 2 < s.length) {
      const n = parseInt(s.slice(i + 1, i + 3), 16);
      if (!Number.isNaN(n)) {
        out.push(n);
        i += 2;
        continue;
      }
    }
    out.push(s.charCodeAt(i));
  }
  return new Uint8Array(out);
}

function decodeBytes(bytes: Uint8Array, charset: string): string {
  const label = charset === 'utf8' ? 'utf-8' : charset;
  try {
    return new TextDecoder(label).decode(bytes);
  } catch {
    return new TextDecoder('utf-8').decode(bytes);
  }
}
