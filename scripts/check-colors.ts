// No raw hex in src/ components (tokens only).
import { execSync } from 'node:child_process';
try {
  const out = execSync(
    'grep -rnE "#[0-9a-fA-F]{6}" src/ --include="*.tsx" --include="*.ts" | grep -v test | grep -v mail-frame | grep -v "lib/colors.ts" || true',
  ).toString();
  // Allowlist: mail-frame.css and comments with color-mix fallbacks? Keep strict: fail on 6-digit hex in components.
  const lines = out
    .trim()
    .split('\n')
    .filter(Boolean)
    .filter((l) => !l.includes('mail-frame'));
  if (lines.length > 0) {
    console.error('raw hex found (use tokens):\n' + lines.slice(0, 10).join('\n'));
    process.exit(1);
  }
  console.log('colors ok');
} catch (e) {
  console.error(e);
  process.exit(1);
}
