import {
  applyI18n, msg, getSettings, saveSettings, getDecisions, saveDecisions, parseList, parseHosts,
} from './lib.js';

applyI18n();
document.title = msg('optTitle');
const $ = id => document.getElementById(id);
const s = await getSettings();

const flip = el => {
  const on = el.getAttribute('aria-checked') !== 'true';
  el.setAttribute('aria-checked', String(on));
  return on;
};
$('enabled').setAttribute('aria-checked', String(s.enabled));
$('ask').setAttribute('aria-checked', String(s.ask));
$('exts').value = s.exts.join(' ');
$('docExts').value = s.docExts.join(' ');
$('minSize').value = s.minSizeMB;
$('sites').value = s.sites.join('\n');

async function showRemembered() {
  const d = Object.entries(await getDecisions());
  $('remembered').textContent = d.length
    ? d.map(([e, v]) => '.' + e + ' → ' + msg(v === 'sush' ? 'choiceSush' : 'choiceChrome')).join('   ')
    : msg('optRememberedNone');
  $('forget').disabled = !d.length;
}

let timer = 0;
function save() {
  clearTimeout(timer);
  timer = setTimeout(async () => {
    const min = Number($('minSize').value);
    await saveSettings({
      enabled: $('enabled').getAttribute('aria-checked') === 'true',
      ask: $('ask').getAttribute('aria-checked') === 'true',
      exts: parseList($('exts').value),
      docExts: parseList($('docExts').value),
      minSizeMB: Number.isFinite(min) && min >= 0 ? min : 1,
      sites: parseHosts($('sites').value),
    });
    $('saved').textContent = msg('optSaved');
    setTimeout(() => { $('saved').textContent = ''; }, 1500);
  }, 350);
}
for (const id of ['enabled', 'ask']) $(id).addEventListener('click', () => { flip($(id)); save(); });
for (const id of ['exts', 'docExts', 'minSize', 'sites']) $(id).addEventListener('input', save);
$('forget').addEventListener('click', async () => { await saveDecisions({}); showRemembered(); });
showRemembered();
