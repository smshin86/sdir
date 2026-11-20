// Only the first click/focus selects all; subsequent clicks can place the caret.
if (!window.__sdirSearchFocusInstalled) {
  window.__sdirSearchFocusInstalled = true;
  const enabled = () => document.querySelector('.app')?.dataset.searchSelectAll === 'true';
  const searchInput = target => target instanceof HTMLInputElement && target.id === 'global-query';
  document.addEventListener('focusin', event => {
    if (searchInput(event.target) && enabled()) event.target.select();
  }, true);
  document.addEventListener('mousedown', event => {
    if (event.button !== 0 || !searchInput(event.target) || !enabled() || document.activeElement === event.target) return;
    // Prevent the browser's later mouse default from collapsing the focus selection.
    event.preventDefault();
    event.target.focus({preventScroll: true});
    event.target.select();
  }, true);
  window.sdirFocusSearch = () => {
    const input = document.getElementById('global-query');
    if (!input) return;
    input.focus({preventScroll: true});
    if (enabled()) input.select();
  };
  const resizer = target => target instanceof Element && target.closest('.search-resizer');
  let resizing;
  const width = (body, value) => {
    const maximum = Math.max(260, Math.min(800, body.clientWidth * 0.55));
    body.style.setProperty('--search-width', Math.min(maximum, Math.max(260, value)) + 'px');
  };
  document.addEventListener('pointerdown', event => {
    const handle = resizer(event.target);
    if (!handle || event.button !== 0 || resizing) return;
    const panel = handle.closest('.search-panel');
    resizing = { id: event.pointerId, handle, body: panel.closest('.body'),
      x: event.clientX, width: panel.getBoundingClientRect().width };
    handle.setPointerCapture(event.pointerId);
    handle.focus({preventScroll: true});
    event.preventDefault();
  });
  document.addEventListener('pointermove', event => {
    if (resizing?.id !== event.pointerId) return;
    width(resizing.body, resizing.width + resizing.x - event.clientX);
  });
  for (const type of ['pointerup', 'pointercancel', 'lostpointercapture']) {
    document.addEventListener(type, event => {
      if (resizing?.id === event.pointerId) resizing = undefined;
    });
  }
  document.addEventListener('keydown', event => {
    const handle = resizer(event.target);
    if (!handle || !['ArrowLeft', 'ArrowRight'].includes(event.key)) return;
    event.preventDefault();
    event.stopPropagation();
    const panel = handle.closest('.search-panel');
    const change = event.key === 'ArrowLeft' ? 24 : -24;
    width(panel.closest('.body'), panel.getBoundingClientRect().width + change);
  }, true);
}
