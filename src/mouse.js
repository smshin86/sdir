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

// Wry's native inbound drop handler disables Chromium's native HTML drag initiation.
// Detect only a real left-button slop gesture, then route it to the row's Rust handler.
if(!window.__sdirNativeGesture){
 window.__sdirNativeGesture=true;let candidate=null,suppressClick=false;
 document.addEventListener('mousedown',e=>{suppressClick=false;const row=e.target instanceof Element?e.target.closest('.file-row'):null;candidate=e.button===0&&row?{row,x:e.clientX,y:e.clientY,key:row.dataset.dragKey}:null;if(candidate)e.preventDefault();},{capture:true,passive:false});
 document.addEventListener('mousemove',e=>{if(!candidate)return;if(!(e.buttons&1)||!candidate.row.isConnected||candidate.row.dataset.dragKey!==candidate.key){candidate=null;return;}if(Math.hypot(e.clientX-candidate.x,e.clientY-candidate.y)<8)return;const row=candidate.row;candidate=null;suppressClick=true;e.preventDefault();row.dispatchEvent(new DragEvent('dragstart',{bubbles:true,cancelable:true,buttons:1,clientX:e.clientX,clientY:e.clientY,ctrlKey:e.ctrlKey,shiftKey:e.shiftKey,dataTransfer:new DataTransfer()}));},{capture:true,passive:false});
 document.addEventListener('mouseup',()=>{candidate=null;},true);
 document.addEventListener('keydown',e=>{if(e.key==='Escape')candidate=null;},true);
 document.addEventListener('click',e=>{if(suppressClick&&e.target instanceof Element&&e.target.closest('.file-row')){e.preventDefault();e.stopPropagation();suppressClick=false;}},true);
}
