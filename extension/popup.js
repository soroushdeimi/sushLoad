import { applyI18n, msg, getSettings, saveSettings, sendNative } from './lib.js';

applyI18n();
const sw = document.getElementById('sw');
const dot = document.getElementById('dot');
const text = document.getElementById('stext');

const settings = await getSettings();
sw.setAttribute('aria-checked', String(settings.enabled));
sw.addEventListener('click', async () => {
  const on = sw.getAttribute('aria-checked') !== 'true';
  sw.setAttribute('aria-checked', String(on));
  await saveSettings({ ...(await getSettings()), enabled: on });
});

function status(kind, key) {
  dot.className = 'dot ' + kind;
  text.textContent = msg(key);
}
// One ping when the popup opens; nothing runs afterwards.
const r = await sendNative({ type: 'ping' });
if (!r.ok) status('err', 'statusMissing');
else if (r.running) status('ok', 'statusRunning');
else status('warn', 'statusIdle');
// When the app asks before every download, say so; otherwise the popup stays quiet.
if (r.ok && r.confirm) {
  const note = document.getElementById('note');
  note.textContent = msg('popupConfirmNote');
  note.hidden = false;
}

document.getElementById('open').addEventListener('click', async () => {
  await sendNative({ type: 'open' });
  window.close();
});
document.getElementById('opts').addEventListener('click', () => {
  chrome.runtime.openOptionsPage();
  window.close();
});
