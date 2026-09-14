export function initMotion() {
  if (matchMedia('(prefers-reduced-motion: reduce)').matches || !('IntersectionObserver' in window)) return;
  const observer = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      entry.target.classList.add('in');
      observer.unobserve(entry.target);
    }
  }, { threshold: 0.08 });
  document.querySelectorAll('[data-chart], .section').forEach((element) => {
    element.classList.add(element.hasAttribute('data-chart') ? 'chart-ready' : 'reveal-ready');
    requestAnimationFrame(() => requestAnimationFrame(() => observer.observe(element)));
  });
}
