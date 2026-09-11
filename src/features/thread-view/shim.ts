// Injected into email iframes (sandbox="allow-scripts", opaque origin).
export function buildShim(nonce: string): string {
  void nonce;
  return `
(function(){
  function post(m){ parent.postMessage({ __sift: true, ...m }, '*'); }
  function report(){
    var root = document.documentElement;
    var body = document.body;
    var h = Math.max(root.scrollHeight, body ? body.scrollHeight : 0,
                     root.getBoundingClientRect().height, body ? body.getBoundingClientRect().height : 0);
    post({ type:'size', height: Math.max(1, Math.ceil(h)) });
  }

  // Collapse quoted history from HTML mail (Gmail, Apple Mail, Outlook, Yahoo).
  function collapseQuotes(){
    var sels = ['.gmail_quote', '.yahoo_quoted', '[id^="divRplyFwdMsg"]',
                'blockquote[type="cite"]', 'blockquote.cite', '#divRplyFwdMsg'];
    var seen = [];
    sels.forEach(function(s){
      try {
        document.querySelectorAll(s).forEach(function(n){
          if (seen.indexOf(n) < 0) seen.push(n);
        });
      } catch (e) {}
    });
    seen.forEach(function(n){
      if (n.closest && n.closest('details.sift-quote')) return;
      var wrap = document.createElement('details');
      wrap.className = 'sift-quote';
      var sum = document.createElement('summary');
      sum.textContent = 'Show quoted text';
      n.parentNode.insertBefore(wrap, n);
      wrap.appendChild(sum);
      wrap.appendChild(n);
    });
  }

  try { collapseQuotes(); } catch (e) {}

  var ro = new ResizeObserver(report);
  ro.observe(document.documentElement);
  if (document.body) ro.observe(document.body);

  window.addEventListener('load', function(){ report(); setTimeout(report, 100); setTimeout(report, 400); });
  // Web fonts and images change metrics after load.
  if (document.fonts && document.fonts.ready) document.fonts.ready.then(report);
  document.addEventListener('load', function(e){
    if (e.target && e.target.tagName === 'IMG') report();
  }, true);
  document.addEventListener('error', function(e){
    if (e.target && e.target.tagName === 'IMG') report();
  }, true);

  document.addEventListener('click', function(e){
    var a = e.target.closest ? e.target.closest('a') : null;
    if (a) { e.preventDefault(); post({ type:'link', href: a.getAttribute('href'), text: a.textContent.slice(0,120) }); return; }
  });
  var hoverT = null;
  document.addEventListener('mouseover', function(e){
    var a = e.target.closest ? e.target.closest('a') : null;
    if (a) { clearTimeout(hoverT); hoverT = setTimeout(function(){ post({ type:'hover', href: a.getAttribute('href') }); }, 300); }
  });
  document.addEventListener('keydown', function(e){
    var map = {ArrowDown:'j', ArrowUp:'k'};
    var key = map[e.key] || e.key;
    if (['j','k','e','#','s','h','l','r','n','p','o','/'].includes(key)) {
      e.preventDefault();
      post({ type:'key', key: key });
    }
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
