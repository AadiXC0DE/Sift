// Injected into email iframes (sandbox="allow-scripts", opaque origin).
export function buildShim(nonce: string, token: string): string {
  void nonce;
  return `
(function(){
  // Every message carries the frame's random token (P9.2): the parent rejects a
  // payload that does not match the frame it was posted from.
  var TOKEN = ${JSON.stringify(token)};
  function post(m){ parent.postMessage({ __sift: true, token: TOKEN, ...m }, '*'); }
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
  // Clear the link preview as soon as the pointer leaves the link (or the
  // document), so it does not linger over the message.
  document.addEventListener('mouseout', function(e){
    var a = e.target.closest ? e.target.closest('a') : null;
    if (!a) return;
    var to = e.relatedTarget;
    if (to && to.closest && to.closest('a') === a) return;
    clearTimeout(hoverT);
    post({ type:'hover', href: '' });
  });
  window.addEventListener('blur', function(){ clearTimeout(hoverT); post({ type:'hover', href: '' }); });
  document.addEventListener('keydown', function(e){
    // Cmd/Ctrl+F inside the message asks the app to open its own find bar
    // (P9.3): the chord belongs to the app, so mail HTML never sees it.
    if ((e.metaKey || e.ctrlKey) && !e.altKey && (e.key === 'f' || e.key === 'F')) {
      e.preventDefault();
      post({ type:'find-open' });
      return;
    }
    // Escape only leaves the frame while a find the app started is live, so an
    // unmodified Escape inside mail HTML keeps its own meaning (P9.3).
    if (e.key === 'Escape' && findActive) { post({ type:'find-escape' }); return; }
    // Navigation only, and only without modifiers: a modifier chord belongs to
    // the app's own windows, not to mail HTML (P9.2).
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.isComposing || e.keyCode === 229) return;
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
  // find-in-thread (P9.3)
  // The frame is sandboxed with an opaque origin, so the app cannot call in:
  // a search arrives as a postMessage carrying this frame's token, and every
  // answer goes back the same way.
  var findMarks = [];      // highlight wrappers, in document order
  var findIndex = -1;      // 0-based position within findMarks
  var findActive = false;  // a find started by the app is live

  // Merge the text nodes that splitText created back into their parents, so the
  // message reads exactly as it shipped and the next scan starts clean.
  function unwrapMarks(){
    var marks = document.querySelectorAll('mark.__sift-hl');
    var parents = [];
    for (var i = 0; i < marks.length; i++) {
      var m = marks[i];
      var p = m.parentNode;
      if (!p) continue;
      p.replaceChild(document.createTextNode(m.textContent), m);
      if (parents.indexOf(p) < 0) parents.push(p);
    }
    for (var j = 0; j < parents.length; j++) {
      if (parents[j].normalize) parents[j].normalize();
    }
    findMarks = [];
  }

  function isOwnMark(node){
    return node.nodeName === 'MARK' && String(node.className || '').indexOf('__sift-hl') >= 0;
  }

  // Never search our own highlight text, or the frame's own script/style nodes
  // — neither is message content, and marking them would corrupt it.
  function skipText(node){
    var p = node.parentNode;
    while (p && p.nodeType === 1) {
      if (isOwnMark(p)) return true;
      var tag = p.nodeName;
      if (tag === 'SCRIPT' || tag === 'STYLE' || tag === 'NOSCRIPT' ||
          tag === 'TEXTAREA' || tag === 'TITLE') return true;
      p = p.parentNode;
    }
    return false;
  }

  // Every non-overlapping occurrence, left to right, across all text nodes —
  // a match count has to describe the message, not one hit per text node.
  function scanText(q){
    var needle = q.toLowerCase();
    var walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT, null);
    var nodes = [];
    var node;
    while ((node = walker.nextNode())) {
      if (!skipText(node)) nodes.push(node);
    }
    var found = [];
    for (var i = 0; i < nodes.length; i++) {
      var rest = nodes[i];
      while (rest) {
        var at = rest.textContent.toLowerCase().indexOf(needle);
        if (at < 0) break;
        var mid = rest.splitText(at);
        var tail = mid.splitText(needle.length);
        var mark = document.createElement('mark');
        mark.className = '__sift-hl';
        mark.setAttribute('data-sift-match', String(found.length + 1));
        mark.style.background = 'var(--sift-mark)';
        mark.style.color = 'inherit';
        mid.parentNode.insertBefore(mark, mid);
        mark.appendChild(mid);
        found.push(mark);
        rest = tail;
      }
    }
    return found;
  }

  function activate(idx){
    for (var i = 0; i < findMarks.length; i++) {
      var active = i === idx;
      findMarks[i].className = active ? '__sift-hl __sift-hl-active' : '__sift-hl';
      findMarks[i].style.background = active ? 'var(--sift-mark-active)' : 'var(--sift-mark)';
      findMarks[i].style.color = 'inherit';
    }
    if (idx >= 0 && findMarks[idx] && findMarks[idx].scrollIntoView) {
      try { findMarks[idx].scrollIntoView({ block: 'center' }); } catch (e) {}
    }
  }

  // A reset request re-scans the whole message and lands on match 1; otherwise
  // the current match moves one step, wrapping at either end.
  window.__siftFind = function(q, direction, reset){
    var text = typeof q === 'string' ? q : '';
    if (!text) {
      unwrapMarks();
      findIndex = -1;
      findActive = false;
      return { count: 0, index: 0 };
    }
    findActive = true;
    // A scan that had to run anyway (fresh body, or the first Next after a
    // reload) already points at match 1, so it does not also step.
    var rescanned = false;
    if (reset || !findMarks.length) {
      unwrapMarks();
      findMarks = scanText(text);
      findIndex = 0;
      rescanned = true;
    }
    if (findMarks.length && !rescanned) {
      var step = direction < 0 ? -1 : 1;
      findIndex = (findIndex + step + findMarks.length) % findMarks.length;
    }
    if (!findMarks.length) findIndex = -1;
    activate(findIndex);
    return { count: findMarks.length, index: findIndex < 0 ? 0 : findIndex + 1 };
  };

  window.addEventListener('message', function(e){
    // The token ties the request to this frame; a source check alone cannot
    // separate two frames that share an origin.
    if (e.source !== parent) return;
    var d = e.data;
    if (!d || typeof d !== 'object' || d.__siftFindReq !== true) return;
    if (d.token !== TOKEN || d.type !== 'find') return;
    var res = window.__siftFind(d.q, d.direction, d.reset);
    post({ type:'find-result', count: res.count, index: res.index });
    // Highlighting changes line wrapping, so the frame has to re-report.
    report();
  });
})();
`;
}
