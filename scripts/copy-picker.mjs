// Regression adapter: explicitly choose a destination before the native copy begins.
export async function chooseCopyDestination(evaluate, click, pane, tab) {
  const until = async (fn, label) => {
    const start = Date.now();
    while (Date.now()-start < 20000) { if (await fn()) return; await new Promise(r=>setTimeout(r,40)); }
    throw Error('Copy picker timeout: '+label);
  };
  await until(()=>evaluate(`!!document.querySelector('.copy-destination-dialog')`),'dialog');
  await evaluate(`(()=>{const e=document.querySelector('.copy-target-panel');e.value=${JSON.stringify(String(pane))};e.dispatchEvent(new Event('change',{bubbles:true}));})()`);
  await until(()=>evaluate(`document.querySelector('.copy-destination-preview')?.dataset.destinationPane===${JSON.stringify(String(pane))}`),'panel');
  if (tab !== undefined) {
    await evaluate(`(()=>{const e=document.querySelector('.copy-target-tab');e.value=${JSON.stringify(String(tab))};e.dispatchEvent(new Event('change',{bubbles:true}));})()`);
    await until(()=>evaluate(`document.querySelector('.copy-destination-preview')?.dataset.destinationTab===${JSON.stringify(String(tab))}`),'tab');
  }
  await click('.copy-destination-confirm');
  await until(()=>evaluate(`!document.querySelector('.copy-destination-dialog')`),'confirmation');
}
