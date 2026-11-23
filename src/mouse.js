// Cancel browser history synchronously; Rust handles one pane history step on mouseup.
if (!window.__sdirMouseHistoryInstalled) {
  window.__sdirMouseHistoryInstalled = true;
  for (const type of ['mousedown', 'mouseup', 'auxclick']) {
    document.addEventListener(type, event => {
      if ((event.button === 3 || event.button === 4) && event.target instanceof Element && event.target.closest('.app')) {
        event.preventDefault();
      }
    }, { capture: true, passive: false });
  }
}

if(!window.__sdirContextGuard){window.__sdirContextGuard=true;document.addEventListener('contextmenu',e=>{if(e.target instanceof Element&&!e.target.matches('input,textarea')&&e.target.closest('.app'))e.preventDefault();},{capture:true});}

if(!window.__sdirDragGuard){window.__sdirDragGuard=true;document.addEventListener('dragstart',e=>{if(e.target instanceof Element&&e.target.closest('.file-row'))e.preventDefault();},{capture:true});}

if(!window.__sdirTreeKeys){window.__sdirTreeKeys=true;document.addEventListener('keydown',e=>{if(e.target instanceof Element&&e.target.closest('.drive-tree')&&['ArrowUp','ArrowDown','ArrowLeft','ArrowRight','Home','End','Enter'].includes(e.key))e.preventDefault();},{capture:true,passive:false});}

// Handle local drops in the WebView. The native window decides when to hand off to OLE.
if (!window.__sdirNativeGesture) {
  window.__sdirNativeGesture = true;
  let candidate;
  let highlight;
  let hoverTimer;
  let nextId = 0;
  let suppressClick = false;

  function setTarget(element) {
    if (element === highlight) return;
    clearTimeout(hoverTimer);
    highlight?.classList.remove('drag-drop-target');
    highlight = element;
    highlight?.classList.add('drag-drop-target');
    if (!element?.matches('.tab[data-drag-tab]:not(.selected)')) return;

    const id = candidate.id;
    hoverTimer = setTimeout(() => {
      if (candidate?.id === id && element.isConnected) send('hover_tab', id, {tab: tabPoint(element)});
    }, 475);
  }

  function paneOf(element) {
    return Number(element.closest('[data-pane]').dataset.pane);
  }

  function tabPoint(tab) {
    return {pane: paneOf(tab), index: Number(tab.dataset.dragTab), folder: tab.title};
  }

  function point(list, row) {
    return {
      pane: paneOf(list),
      generation: Number(list.dataset.dragGeneration),
      folder: list.dataset.dragFolder,
      row: row ? Number(row.dataset.rowIndex) : null,
      rowPath: row?.dataset.dragKey ?? null,
    };
  }

  function targetAt(event) {
    const hit = document.elementFromPoint(event.clientX, event.clientY);
    const tab = hit?.closest('.tab[data-drag-tab]');
    if (tab) return {element: tab, target: {tab: tabPoint(tab)}};

    const list = hit?.closest('.list-scroll');
    const row = hit?.closest('.file-row');
    if (!list || (row && row.dataset.dropDirectory !== 'true')) return null;
    return {element: row ?? list, target: {list: point(list, row)}};
  }

  function send(kind, id, payload = {}) {
    const input = document.querySelector('.internal-drop-input');
    if (!input) return;
    input.value = JSON.stringify({kind, id, ...payload});
    input.dispatchEvent(new Event('input', {bubbles: true}));
  }

  function clear() {
    if (candidate?.preview) candidate.preview.hidden = true;
    candidate = null;
    setTarget(null);
  }

  function cancel() {
    if (candidate) send('cancel', candidate.id);
    clear();
  }

  document.addEventListener('mousedown', event => {
    cancel();
    suppressClick = false;
    if (event.target instanceof Element && event.target.closest('.inline-rename-input,.file-select-checkbox')) return;
    const row = event.target instanceof Element ? event.target.closest('.file-row') : null;
    const list = row?.closest('.list-scroll');
    if (event.button !== 0 || !list) return;
    candidate = {row, source: point(list, row), id: ++nextId,
      x: event.clientX, y: event.clientY, started: false};
    send('arm', candidate.id, {source: candidate.source});
    event.preventDefault();
  }, {capture: true, passive: false});

  document.addEventListener('mousemove', event => {
    if (!candidate) return;
    if (!(event.buttons & 1)) {
      cancel();
      return;
    }
    if (!candidate.started) {
      const {row, source} = candidate;
      if (!row.isConnected || row.dataset.dragKey !== source.rowPath) {
        cancel();
        return;
      }
      if (Math.hypot(event.clientX - candidate.x, event.clientY - candidate.y) < 8) return;
      candidate.started = true;
      suppressClick = true;
      send('begin', candidate.id);

      const preview = document.querySelector('.file-drag-preview');
      if (preview) {
        const count = row.classList.contains('selected') ? Number(row.closest('.list-scroll').dataset.dragSelectionCount) : 1;
        preview.textContent = count > 1 ? `${count}개 항목` : (row.querySelector('.file-name span')?.textContent ?? '파일');
        preview.hidden = false;
        candidate.preview = preview;
        candidate.previewWidth = preview.offsetWidth;
        candidate.previewHeight = preview.offsetHeight;
      }
    }
    if (candidate.preview) {
      const x = Math.max(0, Math.min(event.clientX + 14, innerWidth - candidate.previewWidth - 4));
      const y = Math.max(0, Math.min(event.clientY + 16, innerHeight - candidate.previewHeight - 4));
      candidate.preview.style.transform = `translate(${x}px, ${y}px)`;
    }
    setTarget(targetAt(event)?.element ?? null);
    event.preventDefault();
  }, {capture: true, passive: false});

  window.addEventListener('sdir-native-drag', event => {
    if (candidate?.id === event.detail) clear();
  });

  document.addEventListener('mouseup', event => {
    if (event.button !== 0 || !candidate?.started) {
      cancel();
      return;
    }
    const hit = targetAt(event);
    if (hit) {
      send('release', candidate.id, {
        target: hit.target, ctrl: event.ctrlKey, shift: event.shiftKey, alt: event.altKey,
      });
      clear();
      event.preventDefault();
    } else {
      cancel();
    }
  }, {capture: true, passive: false});

  document.addEventListener('keydown', event => {
    if (event.key === 'Escape') cancel();
  }, true);

  document.addEventListener('click', event => {
    if (!suppressClick) return;
    suppressClick = false;
    if (event.target instanceof Element && event.target.closest('.app')) {
      event.preventDefault();
      event.stopPropagation();
    }
  }, true);
}

if (!window.__sdirColumnResizeInstalled) {
  window.__sdirColumnResizeInstalled = true;
  let drag;

  document.addEventListener('pointerdown', event => {
    const handle = event.target instanceof Element && event.target.closest('.detail-resize-handle');
    if (!handle || event.button !== 0 || drag) return;
    const pane = handle.closest('[data-pane][data-view="details"]');
    const column = handle.closest('[data-detail-column]')?.dataset.detailColumn;
    const input = pane?.querySelector('.detail-width-input');
    if (!input || !/^[0-3]$/.test(column ?? '')) return;
    const property = `--detail-column-${column}`;
    const original = parseInt(getComputedStyle(pane).getPropertyValue(property), 10);
    if (!Number.isFinite(original)) return;
    handle.setPointerCapture(event.pointerId);
    event.preventDefault();
    drag = {handle, pane, input, column, property, original, width: original, x: event.clientX,
      pointerId: event.pointerId, min: Number(handle.dataset.minWidth), max: Number(handle.dataset.maxWidth)};
  }, true);

  function update(event) {
    const width = Math.round(drag.original + event.clientX - drag.x);
    drag.width = Math.max(drag.min, Math.min(drag.max, width));
    drag.pane.style.setProperty(drag.property, `${drag.width}px`);
  }

  document.addEventListener('pointermove', event => {
    if (drag?.pointerId === event.pointerId) update(event);
  }, true);

  document.addEventListener('pointerup', event => {
    if (drag?.pointerId !== event.pointerId) return;
    update(event);
    const current = drag;
    drag = null;
    if (current.width !== current.original && current.input.isConnected) {
      current.input.value = `${current.column}:${current.width}`;
      current.input.dispatchEvent(new Event('input', {bubbles: true}));
    }
  }, true);

  for (const type of ['pointercancel', 'lostpointercapture']) {
    document.addEventListener(type, event => {
      if (drag?.pointerId !== event.pointerId) return;
      drag.pane.style.setProperty(drag.property, `${drag.original}px`);
      drag = null;
    }, true);
  }
}

window.__sdirCycleFocus = reverse => {
  const selectors = [
    '.drive-tree',
    '.action-bar button:not(:disabled)',
    '.pane .tab.selected',
    '.pane .address-input',
    '.pane .list-scroll',
  ];
  const targets = selectors.flatMap((selector, index) => {
    const nodes = [...document.querySelectorAll(selector)].filter(node => node.getClientRects().length > 0);
    return index === 1 ? nodes.slice(0, 1) : nodes;
  });
  const focused = document.activeElement;
  const index = targets.indexOf(focused);
  targets[index < 0 ? (reverse ? targets.length - 1 : 0) :
    (index + (reverse ? -1 : 1) + targets.length) % targets.length]?.focus({preventScroll:true});
};

if (!window.__sdirPaneZoomInstalled) {
  window.__sdirPaneZoomInstalled = true;
  document.addEventListener('wheel', event => {
    if (!event.ctrlKey || !event.deltaY || !(event.target instanceof Element)) return;
    const list = event.target.closest('.list-scroll');
    const input = list?.closest('.pane')?.querySelector('.pane-zoom-input');
    if (!input || document.querySelector('.modal-backdrop,.context-dismiss')) return;
    event.preventDefault();
    input.value = event.deltaY < 0 ? 'in' : 'out';
    input.dispatchEvent(new Event('input', {bubbles:true}));
  }, { capture:true, passive:false });
}

if (!window.__sdirMarqueeInstalled) {
  window.__sdirMarqueeInstalled = true;
  let drag, ignoreClick;
  const overlay = document.createElement('div');
  overlay.className = 'selection-marquee';
  overlay.setAttribute('aria-hidden', 'true');

  const stop = () => {
    if (drag?.frame) cancelAnimationFrame(drag.frame);
    drag = null;
    overlay.remove();
  };
  const position = (list, x, y) => {
    const bounds = list.getBoundingClientRect();
    const grid = list.querySelector('.virtual-rows')?.getBoundingClientRect();
    return [x - (grid?.left ?? bounds.left), y - bounds.top - list.clientTop + list.scrollTop];
  };
  function draw() {
    if (!drag?.list.isConnected) return stop();
    const {list, start, x, y} = drag;
    const bounds = list.getBoundingClientRect();
    const grid = list.querySelector('.virtual-rows')?.getBoundingClientRect();
    const sx = start[0] + (grid?.left ?? bounds.left);
    const sy = start[1] + bounds.top + list.clientTop - list.scrollTop;
    const clamp = (value, min, max) => Math.max(min, Math.min(max, value));
    const left = clamp(Math.min(x, sx), bounds.left, bounds.right);
    const top = clamp(Math.min(y, sy), bounds.top, bounds.bottom);
    const right = clamp(Math.max(x, sx), bounds.left, bounds.right);
    const bottom = clamp(Math.max(y, sy), bounds.top, bounds.bottom);
    Object.assign(overlay.style, {left:left+'px',top:top+'px',width:(right-left)+'px',height:(bottom-top)+'px'});
  }
  function tick() {
    if (!drag?.list.isConnected) return stop();
    const {list, y} = drag;
    const {top, bottom} = list.getBoundingClientRect();
    if (y < top + 22) list.scrollTop -= Math.min(18, Math.ceil((top + 22 - y)/2));
    else if (y > bottom - 22) list.scrollTop += Math.min(18, Math.ceil((y - bottom + 22)/2));
    draw();
    drag.frame = requestAnimationFrame(tick);
  }
  document.addEventListener('pointerdown', event => {
    if (event.button !== 0 || !(event.target instanceof Element)) return;
    stop();
    ignoreClick = false;
    const list = event.target.closest('.list-scroll');
    if (!list || event.target.closest('.file-row,.file-group-heading') ||
      document.querySelector('.modal-backdrop,.context-dismiss')) return;
    const bounds = list.getBoundingClientRect();
    if (event.clientX >= bounds.left + list.clientWidth ||
        event.clientY >= bounds.top + list.clientHeight) return;
    drag = {list, pointer:event.pointerId, fromX:event.clientX, fromY:event.clientY,
      start:position(list,event.clientX,event.clientY)};
  }, true);
  document.addEventListener('pointermove', event => {
    if (drag?.pointer !== event.pointerId) return;
    if (!(event.buttons & 1) || !drag.list.isConnected) return stop();
    const x = event.clientX, y = event.clientY;
    if (!drag.frame && Math.hypot(x-drag.fromX,y-drag.fromY)<5) return;
    Object.assign(drag,{x,y});
    if (!drag.frame) {
      window.getSelection()?.removeAllRanges();
      document.body.append(overlay);
      tick();
    } else {
      draw();
    }
    event.preventDefault();
  }, true);
  document.addEventListener('pointerup', event => {
    if (drag?.pointer !== event.pointerId) return;
    const {list, start, frame} = drag;
    stop();
    if (!frame || !list.isConnected) return;
    const input = list.closest('.pane')?.querySelector('.pane-marquee-input');
    const grid = list.querySelector('.virtual-rows');
    if (!input || !grid) return;
    ignoreClick = true;
    const bounds = list.getBoundingClientRect();
    input.value = JSON.stringify({generation:Number(list.dataset.dragGeneration), start,
      end:position(list,
        Math.max(bounds.left,Math.min(bounds.left+list.clientWidth,event.clientX)),
        Math.max(bounds.top,Math.min(bounds.top+list.clientHeight-1,event.clientY))),
      width:grid.getBoundingClientRect().width, ctrl:event.ctrlKey, shift:event.shiftKey});
    input.dispatchEvent(new Event('input',{bubbles:true}));
    event.preventDefault();
  }, true);
  document.addEventListener('click', event => {
    if (!ignoreClick) return;
    ignoreClick = false;
    event.stopImmediatePropagation();
    event.preventDefault();
  }, true);
  document.addEventListener('pointercancel', stop, true);
  document.addEventListener('keydown', event => { if (event.key === 'Escape') stop(); }, true);
  window.addEventListener('blur', stop);
}
