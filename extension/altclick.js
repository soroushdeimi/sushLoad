// Alt+click on a link = "let the browser download this one". Tell the worker; nothing else.
addEventListener('mousedown', e => {
  if (e.altKey) { try { chrome.runtime.sendMessage({ t: 'alt' }); } catch { /* extension reloaded */ } }
}, true);
