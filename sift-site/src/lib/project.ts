/**
 * The project's identity: where the source lives, who maintains it, what the
 * license is, and the two marks the site draws in its own chrome.
 *
 * Every page imports from here rather than typing a URL, so a link, an issue
 * address or the license name cannot disagree between pages.
 */

/** Canonical repository. */
export const REPO_URL = 'https://github.com/AadiXC0DE/Sift';

/** The maintainer. */
export const AUTHOR_URL = 'https://github.com/AadiXC0DE';

/** Where bugs and feature requests go. */
export const ISSUES_URL = `${REPO_URL}/issues`;

/** The grant itself, read from the repository so the claim and the file agree. */
export const LICENSE_NAME = 'MIT';
export const LICENSE_URL = `${REPO_URL}/blob/main/LICENSE`;

/** The line the site uses when it says what the license permits. */
export const LICENSE_SUMMARY =
  'Use, modify and redistribute it, including in closed-source builds, as long as the copyright notice and the license text travel with it.';

/** Apple's mark, as every download button draws it. */
export const appleMark = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 384 512" width="15" height="15" aria-hidden="true" focusable="false"><path fill="currentColor" d="M318.7 268.7c-.2-36.7 16.4-64.4 50-84.8-18.8-26.9-47.2-41.7-84.7-44.6-35.5-2.8-74.3 20.7-88.5 20.7-15 0-49.4-19.7-76.4-19.7C63.3 141.2 4 184.8 4 273.5q0 39.3 14.4 81.2c12.8 36.7 59 126.7 107.2 125.2 25.2-.6 43-17.9 75.8-17.9 31.8 0 48.3 17.9 76.4 17.9 48.6-.7 90.4-82.5 102.6-119.3-65.2-30.7-61.7-90-61.7-91.9zm-56.6-164.2c27.3-32.4 24.8-61.9 24-72.5-24.1 1.4-52 16.4-67.9 34.9-17.5 19.8-27.8 44.3-25.6 71.9 26.1 2 49.9-11.4 69.5-34.3z"/></svg>`;

/** GitHub's mark (Octicons, MIT), drawn at 16px so it matches nav text. */
export const githubMark = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true" focusable="false"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27s1.36.09 2 .27c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8z"/></svg>`;
