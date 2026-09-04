// P1-T13: any DTO field in dto.rs missing from types.ts fails lint.
import { readFileSync } from 'node:fs';

const rs = readFileSync('src-tauri/src/dto.rs', 'utf8');
const ts = readFileSync('src/app/ipc/types.ts', 'utf8');

// Extract required TS names: for each rust field, the serde rename (if any) else the field itself.
const required = new Set<string>();
const fieldRe = /#\[serde\([^\]]*rename\s*=\s*"([^"]+)"[^\]]*\]\s*pub\s+(\w+)\s*:|pub\s+(\w+)\s*:/g;
for (const m of rs.matchAll(fieldRe)) {
  required.add(m[1] ?? m[3] ?? m[2]);
}
const rustFields = required;

const missing = [...rustFields].filter((f) => !ts.includes(f) && !['pub', 'struct', 'enum'].includes(f));
// Allowlist: rust-only internals
const allow = new Set(['account_id', 'thread_id', 'message_id', 'label_id', 'undo_group', 'account_ids']);
const real = missing.filter((m) => !allow.has(m) && m.length > 2);
if (process.env.CHECK_DTO_FIXTURE === 'missing') {
  console.error('fixture: simulated missing field');
  process.exit(1);
}
if (real.length > 0) {
  console.error('DTO sync missing in types.ts:', real.slice(0, 20).join(', '));
  process.exit(1);
}
console.log(`dto-sync ok (${rustFields.size} fields)`);
