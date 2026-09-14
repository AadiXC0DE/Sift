import source from './mail-shim.js?raw';

/** Test harness uses the same source shipped as the hash-approved frame script. */
export function buildShim(nonce: string, token: string): string {
  void nonce;
  return source.replace('document.currentScript.dataset.token', JSON.stringify(token));
}
