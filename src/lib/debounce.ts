export function debounce<T extends (...a: never[]) => void>(fn: T, ms: number): T {
  let t: ReturnType<typeof setTimeout> | null = null;
  return ((...a: never[]) => {
    if (t) clearTimeout(t);
    t = setTimeout(() => fn(...a), ms);
  }) as T;
}
