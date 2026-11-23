if (!window.sdirSyncSidebarWidth) {
  const handleFor = target => target instanceof Element ? target.closest('.sidebar-resizer') : null;
  const sidebarFor = handle => handle.previousElementSibling;
  const limits = sidebar => {
    const style = getComputedStyle(sidebar);
    return [parseFloat(style.getPropertyValue('--sidebar-min-width')), parseFloat(style.getPropertyValue('--sidebar-max-width'))];
  };
  const updateAccessibility = (handle, sidebar) => {
    const [min, max] = limits(sidebar);
    handle.setAttribute('aria-valuemin', min);
    handle.setAttribute('aria-valuemax', max);
    handle.setAttribute('aria-valuenow', Math.round(sidebar.getBoundingClientRect().width));
  };
  window.sdirSyncSidebarWidth = () => {
    const handle = document.querySelector('.sidebar-resizer');
    if (handle) updateAccessibility(handle, sidebarFor(handle));
  };
  window.addEventListener('resize', window.sdirSyncSidebarWidth);

  const update = (handle, sidebar, next, [min, max]) => {
    const width = Math.round(Math.min(max, Math.max(min, next)));
    sidebar.style.setProperty('--sidebar-live-width', `${width}px`);
    handle.setAttribute('aria-valuenow', width);
    return width;
  };
  const commit = (handle, width) => {
    const input = handle.querySelector('.sidebar-width-commit');
    input.value = String(width);
    input.dispatchEvent(new Event('input', {bubbles: true}));
  };
  let drag;
  document.addEventListener('pointerdown', event => {
    const handle = handleFor(event.target);
    if (!handle || event.button !== 0 || drag) return;
    const sidebar = sidebarFor(handle);
    const width = sidebar.getBoundingClientRect().width;
    drag = {handle, sidebar, id: event.pointerId, startX: event.clientX, width, latestWidth: Math.round(width),
      original: sidebar.style.getPropertyValue('--sidebar-live-width'), bounds: limits(sidebar)};
    handle.setPointerCapture(event.pointerId);
    handle.focus({preventScroll: true});
    handle.dataset.resizing = 'true';
    event.preventDefault();
  });
  document.addEventListener('pointermove', event => {
    if (drag?.id !== event.pointerId) return;
    drag.latestWidth = update(drag.handle, drag.sidebar, drag.width + event.clientX - drag.startX, drag.bounds);
  });
  const finish = (id, save) => {
    if (!drag || drag.id !== id) return;
    const {handle, sidebar, original, latestWidth} = drag;
    drag = undefined;
    delete handle.dataset.resizing;
    if (handle.hasPointerCapture(id)) handle.releasePointerCapture(id);
    if (save) {
      commit(handle, latestWidth);
    } else {
      if (original) sidebar.style.setProperty('--sidebar-live-width', original);
      else sidebar.style.removeProperty('--sidebar-live-width');
      updateAccessibility(handle, sidebar);
    }
  };
  document.addEventListener('pointerup', event => finish(event.pointerId, true));
  for (const name of ['pointercancel', 'lostpointercapture']) {
    document.addEventListener(name, event => finish(event.pointerId, false));
  }
  document.addEventListener('keydown', event => {
    const handle = handleFor(event.target);
    if (!handle) return;
    if (event.key === 'Escape' && drag?.handle === handle) {
      finish(drag.id, false);
    } else {
      if (drag || !['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
      const sidebar = sidebarFor(handle);
      const bounds = limits(sidebar);
      const width = sidebar.getBoundingClientRect().width;
      const next = event.key === 'Home' ? bounds[0] : event.key === 'End' ? bounds[1] :
        width + (event.key === 'ArrowRight' ? 16 : -16);
      commit(handle, update(handle, sidebar, next, bounds));
    }
    event.preventDefault();
    event.stopPropagation();
  }, true);
}
