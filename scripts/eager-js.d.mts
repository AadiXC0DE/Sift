// Type surface for scripts/eager-js.mjs (plain Node ESM, no build step).
export const KIB: number;
export const DEFAULT_EAGER_JS_GATE_KIB: number;

export interface EagerChunk {
  key: string;
  file: string;
  rawBytes: number;
  gzipBytes: number;
}

export interface EagerMeasurement {
  source: 'manifest' | 'index-html';
  distDir: string;
  entryKeys: string[];
  chunks: EagerChunk[];
  rawBytes: number;
  eagerBytes: number;
}

export interface EagerBudgetResult extends EagerMeasurement {
  gateKib: number;
  gateBytes: number;
  ok: boolean;
}

export function measureEagerJs(distDir: string): EagerMeasurement;
export function checkEagerBudget(distDir: string, gateKib?: number): EagerBudgetResult;
export function formatReport(result: EagerBudgetResult): string;
