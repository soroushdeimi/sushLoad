import { applyI18n, msg, fmtSize, extOf } from './lib.js';

applyI18n();
const $ = id => document.getElementById(id);
const id = Number(new URLSearchParams(location.search).get('id'));
const key = 'offer:' + id;
const offer = (await chrome.storage.session.get(key))[key];
if (!offer) { window.close(); }
else {
  $('name').textContent = offer.name;
  $('name').title = offer.url;
  $('meta').textContent = fmtSize(offer.size) + (offer.host ? ' · ' + msg('offerFrom', [offer.host]) : '');
  const ext = offer.ext || extOf(offer.name);
  $('rememberText').textContent = msg('offerRemember', [ext]);
  document.title = offer.name;
}

let busy = false;
async function choose(choice) {
  if (busy || !offer) return;
  busy = true;
  $('sush').disabled = $('chrome').disabled = true;
  if (choice === 'sush') $('sush').textContent = msg('offerSending');
  const r = await chrome.runtime.sendMessage({ t: 'offer-choice', id, choice, remember: $('remember').checked });
  if (r && r.ok) { window.close(); return; }
  // The browser download was resumed by the worker; say why.
  $('err').hidden = false;
  $('err').textContent = msg('offerFailed', [(r && r.error) || '']);
  $('rememberRow').hidden = true;
  const close = $('chrome');
  close.textContent = msg('offerClose');
  close.disabled = false;
  close.onclick = () => window.close();
  $('sush').hidden = true;
  close.focus();
}
$('sush').addEventListener('click', () => choose('sush'));
$('chrome').addEventListener('click', () => choose('chrome'));
document.addEventListener('keydown', e => { if (e.key === 'Enter' && !busy) choose('sush'); });
$('sush').focus();
