/**
 * Test-only replacement for `@tauri-apps/plugin-dialog`.
 *
 * Returns deterministic paths so the composer's attach route can be driven
 * without a native file picker.
 */
let nextPath = '/tmp/sift-e2e/attached-report.pdf';

export function setNextOpenPath(path: string): void {
  nextPath = path;
}

export async function open(options?: { multiple?: boolean }): Promise<string | string[] | null> {
  return options?.multiple ? [nextPath] : nextPath;
}

export async function save(): Promise<string | null> {
  return '/tmp/sift-e2e/saved.pdf';
}

export async function message(msg: string): Promise<void> {
  void msg;
}

export async function ask(msg: string): Promise<boolean> {
  void msg;
  return true;
}

export async function confirm(msg: string): Promise<boolean> {
  void msg;
  return true;
}
