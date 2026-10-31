import {
  applyI18n, msg, getSettings, saveSettings, parseList, parseHosts,
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
$('exts').value = s.exts.join(' ');
$('docExts').value = s.docExts.join(' ');
$('minSize').value = s.minSizeMB;
$('sites').value = s.sites.join('\n');

let timer = 0;
function save() {
  clearTimeout(timer);
  timer = setTimeout(async () => {
    const min = Number($('minSize').value);
    await saveSettings({
      enabled: $('enabled').getAttribute('aria-checked') === 'true',
      exts: parseList($('exts').value),
      docExts: parseList($('docExts').value),
      minSizeMB: Number.isFinite(min) && min >= 0 ? min : 1,
      sites: parseHosts($('sites').value),
    });
    $('saved').textContent = msg('optSaved');
    setTimeout(() => { $('saved').textContent = ''; }, 1500);
  }, 350);
}
for (const id of ['enabled']) $(id).addEventListener('click', () => { flip($(id)); save(); });
for (const id of ['exts', 'docExts', 'minSize', 'sites']) $(id).addEventListener('input', save);
