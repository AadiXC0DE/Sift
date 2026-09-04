// Injected into email iframes (sandbox="allow-scripts", opaque origin).
export function buildShim(nonce: string): string {
  void nonce;
  return `
(function(){
  function post(m){ parent.postMessage({ __sift: true, ...m }, '*'); }
  function report(){ post({ type:'size', height: document.documentElement.scrollHeight }); }
  new ResizeObserver(report).observe(document.documentElement);
  window.addEventListener('load', function(){ report(); setTimeout(report, 300); });
  document.addEventListener('click', function(e){
    var a = e.target.closest ? e.target.closest('a') : null;
    if (a) { e.preventDefault(); post({ type:'link', href: a.getAttribute('href'), text: a.textContent.slice(0,120) }); return; }
    var img = e.target.closest ? e.target.closest('img.sift-blocked') : null;
    if (img) { post({ type:'image', src: img.getAttribute('data-sift-src') }); return; }
  });
  var hoverT = null;
  document.addEventListener('mouseover', function(e){
    var a = e.target.closest ? e.target.closest('a') : null;
    if (a) { clearTimeout(hoverT); hoverT = setTimeout(function(){ post({ type:'hover', href: a.getAttribute('href') }); }, 300); }
  });
  document.addEventListener('keydown', function(e){
    if (['j','k','e','#','s','h','l','r','n','p','o','/'].includes(e.key)) post({ type:'key', key: e.key });
  });
  document.addEventListener('contextmenu', function(e){
    var t = e.target;
    if (!(t.closest && (t.closest('a') || t.closest('img')))) e.preventDefault();
  });
  // find-in-thread
  window.__siftFind = function(q){
    document.querySelectorAll('mark.__sift-hl').forEach(function(m){ m.replaceWith(document.createTextNode(m.textContent)); });
    if (!q) return 0;
    var count = 0;
    var walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    var nodes = [];
    while (walker.nextNode()) nodes.push(walker.currentNode);
    nodes.forEach(function(n){
      var i = n.textContent.toLowerCase().indexOf(q.toLowerCase());
      if (i >= 0 && n.parentElement.tagName !== 'MARK') {
        var mark = document.createElement('mark');
        mark.className = '__sift-hl';
        mark.style.background = 'var(--accent-soft)';
        var mid = n.splitText(i);
        var end = mid.splitText(q.length);
        mid.parentNode.insertBefore(mark, end);
        mark.appendChild(mid);
        count++;
      }
    });
    report();
    return count;
  };
})();
`;
}
