import {
  applyI18n, msg, CATEGORY_ORDER, categoryOf, extOf, nameFromUrl, buildItem, handover, describeError,
} from './lib.js';

applyI18n();
const $ = id => document.getElementById(id);
document.title = msg('linksTitle');
const k = 'links:' + new URLSearchParams(location.search).get('k');
const data = (await chrome.storage.session.get(k))[k] || { links: [], page: '' };

const rows = data.links.map(l => {
  const name = nameFromUrl(l.url);
  return { ...l, name, cat: categoryOf(extOf(name)), checked: false };
});

const typeSel = $('type');
typeSel.add(new Option(msg('typeAll'), 'all'));
for (const c of CATEGORY_ORDER) {
  if (rows.some(r => r.cat === c)) typeSel.add(new Option(msg('type' + c[0].toUpperCase() + c.slice(1)), c));
}

let visible = [];
function render() {
  const q = $('q').value.trim().toLowerCase();
  const t = typeSel.value;
  visible = rows.filter(r => (t === 'all' || r.cat === t) && (!q || r.url.toLowerCase().includes(q) || r.text.toLowerCase().includes(q)));
  const list = $('list');
  list.textContent = '';
  if (!visible.length) {
    const e = document.createElement('div');
    e.className = 'empty';
    e.textContent = msg('linksNone');
    list.append(e);
  }
  const frag = document.createDocumentFragment();
  for (const r of visible) {
    const row = document.createElement('label');
    row.className = 'row';
    const cb = document.createElement('input');
    cb.type = 'checkbox';
    cb.checked = r.checked;
    cb.addEventListener('change', () => { r.checked = cb.checked; counts(); });
    const t1 = document.createElement('div');
    t1.className = 't';
    const a = document.createElement('div');
    a.className = 'l1';
    a.dir = 'auto';
    a.textContent = r.text || r.name || r.url;
    const b = document.createElement('div');
    b.className = 'l2';
    b.textContent = r.url;
    t1.append(a, b);
    const tag = document.createElement('div');
    tag.className = 'tag';
    tag.textContent = extOf(r.name);
    row.append(cb, t1, tag);
    frag.append(row);
  }
  list.append(frag);
  counts();
}
function counts() {
  const n = rows.filter(r => r.checked).length;
  $('count').textContent = msg('linksCount', [String(visible.length), String(rows.length)]);
  $('go').textContent = msg('linksDownload', [String(n)]);
  $('go').disabled = n === 0;
}

$('q').addEventListener('input', render);
typeSel.addEventListener('change', render);
$('all').addEventListener('click', () => { visible.forEach(r => { r.checked = true; }); render(); });
$('none').addEventListener('click', () => { visible.forEach(r => { r.checked = false; }); render(); });
$('go').addEventListener('click', async () => {
  const chosen = rows.filter(r => r.checked);
  if (!chosen.length) return;
  $('go').disabled = true;
  const cache = new Map();
  const items = [];
  for (const r of chosen) {
    items.push(await buildItem({ url: r.url, referer: data.page, incognito: data.incognito, cache }));
  }
  const res = await handover(items);
  const m = $('msg');
  if (res.ok) {
    m.className = 'small ok';
    m.textContent = msg('linksSent', [String(res.added)]);
    setTimeout(() => window.close(), 1200);
  } else {
    m.className = 'small err';
    m.textContent = describeError(res);
    $('go').disabled = false;
  }
});

render();
