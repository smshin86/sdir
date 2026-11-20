// A selected virtual row must be rendered before its native viewport can be reconciled.
window.__sdirScrollSelection = (pane, position, fallback) => {
  const list = document.querySelector(`[data-pane="${pane}"] .list-scroll`);
  if (!list) return;
  window.__sdirScrollRequests ??= {};
  window.__sdirScrollRequests[pane]?.disconnect();
  const request = { element: list, disconnect() {
    observer.disconnect();
    if (window.__sdirScrollRequests[pane] === request) delete window.__sdirScrollRequests[pane];
  }};
  const apply = () => {
    if (!list.isConnected) return request.disconnect();
    if (window.__sdirScrollRequests[pane] !== request || list.clientHeight <= 0) return;
    const row = list.querySelector(`[data-row-index="${position}"]`);
    if (row) {
      const r = row.getBoundingClientRect(), top = list.getBoundingClientRect().top + list.clientTop;
      if (r.top < top) list.scrollTop += r.top - top;
      else if (r.bottom > top + list.clientHeight) list.scrollTop += r.bottom - top - list.clientHeight;
      request.disconnect();
    } else {
      const owner = list.closest('.pane'), style = getComputedStyle(owner);
      const first = list.querySelector('.file-row');
      const height = first?.getBoundingClientRect().height || Number.parseFloat(style.getPropertyValue(owner.dataset.view === 'icons' ? '--tile-height' : '--row-height'));
      const columns = Number(list.dataset.columns) || 1;
      const top = Math.floor(position / columns) * height;
      list.scrollTop = Number.isFinite(top) ? (top < list.scrollTop ? top : Math.max(list.scrollTop, top + height - list.clientHeight)) : fallback;
      // The previous scroll event may have captured an obsolete viewport/offset.
      list.dispatchEvent(new Event('scroll'));
    }
  };
  const observer = new MutationObserver(apply);
  window.__sdirScrollRequests[pane] = request;
  observer.observe(list, { childList: true, subtree: true, attributes: true, attributeFilter: ['style', 'data-columns'] });
  if (!list.__sdirScrollCancellation) {
    list.__sdirScrollCancellation = true;
    const cancel = () => {
      const current = window.__sdirScrollRequests[pane];
      if (current?.element === list) current.disconnect();
    };
    list.addEventListener('wheel', cancel, { passive: true });
    list.addEventListener('pointerdown', cancel, { passive: true });
  }
  apply();
};
