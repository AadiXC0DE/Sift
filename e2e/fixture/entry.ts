/**
 * The single source of truth for the test-only app entry. Playwright's
 * webServer readiness URL and every spec navigation must agree, so the path
 * lives in one module rather than being duplicated.
 */
export const E2E_ENTRY = '/e2e/fixture/index.e2e.html';
export const E2E_PORT = 4399;
