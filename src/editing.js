// WebView native undo is document-wide. Keep text undo local and file undo in Rust.
if (!window.__sdirTextUndoInstalled) {
  window.__sdirTextUndoInstalled = true;
  const edits = new WeakMap();
  const replaying = new WeakSet();
  const textInput = target => target instanceof HTMLTextAreaElement ||
    (target instanceof HTMLInputElement && !['checkbox', 'radio', 'button', 'submit'].includes(target.type));
  const stateFor = input => {
    let state = edits.get(input);
    if (!state || state.current !== input.value) {
      state = { current: input.value, undo: [], redo: [] };
      edits.set(input, state);
    }
    return state;
  };
  document.addEventListener('focusin', event => {
    if (textInput(event.target) && event.target.closest('.app')) stateFor(event.target);
  }, true);
  document.addEventListener('input', event => {
    const input = event.target;
    if (!textInput(input) || !input.closest('.app')) return;
    let state = edits.get(input);
    if (!state) { stateFor(input); return; }
    if (!replaying.has(input) && state.current !== input.value) {
      state.undo.push(state.current);
      if (state.undo.length > 64) state.undo.shift();
      state.redo = [];
    }
    state.current = input.value;
  }, true);
  document.addEventListener('keydown', event => {
    if (event.isComposing || !event.ctrlKey || event.altKey || event.metaKey ||
      !['z', 'y'].includes(event.key.toLowerCase()) || !event.target.closest('.app')) return;
    // Must run synchronously, before Chromium executes its native editing command.
    event.preventDefault();
    const input = event.target;
    if (!textInput(input)) return;
    const state = stateFor(input);
    const redo = event.key.toLowerCase() === 'y' || event.shiftKey;
    const from = redo ? state.redo : state.undo;
    const to = redo ? state.undo : state.redo;
    if (!from.length) return;
    to.push(state.current);
    const value = from.pop();
    const prototype = input instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    replaying.add(input);
    Object.getOwnPropertyDescriptor(prototype, 'value').set.call(input, value);
    state.current = value;
    input.setSelectionRange(value.length, value.length);
    input.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: redo ? 'historyRedo' : 'historyUndo' }));
    replaying.delete(input);
  }, true);
}
