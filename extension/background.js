// sushLoad service worker. Event-driven only: no timers, no polling; it sleeps between events.
import {
  msg, getSettings, getDecisions, saveDecisions, isHttp, hostOf, hostMatches, baseName, nameFromUrl,
  classify, buildItem, handover, describeError,
} from './lib.js';

const session = chrome.storage.session;
const attempt = p => Promise.resolve(p).catch(() => undefined);

// A badge is only ever shown for a moment; clear any left over from a worker that was stopped.
attempt(chrome.action.setBadgeText({ text: '' }));

function flash(text, color, tabId) {
  const o = tabId >= 0 ? { tabId } : {};
  attempt(chrome.action.setBadgeBackgroundColor({ color, ...o }));
  attempt(chrome.action.setBadgeText({ text, ...o }));
  setTimeout(() => attempt(chrome.action.setBadgeText({ text: '', ...o })), 3000);
}
const ok = tabId => flash('✓', '#30d158', tabId);
const fail = tabId => flash('!', '#ff453a', tabId);

/* ---------- context menus ---------- */
chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create({ id: 'sl-link', title: msg('menuLink'), contexts: ['link'] });
    chrome.contextMenus.create({ id: 'sl-media', title: msg('menuLink'), contexts: ['image', 'video', 'audio'] });
    chrome.contextMenus.create({ id: 'sl-page', title: msg('menuAll'), contexts: ['page', 'frame'] });
  });
});

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  const tabId = tab ? tab.id : -1;
  try {
    if (info.menuItemId === 'sl-page') return await openLinkPicker(info, tab);
    const url = info.menuItemId === 'sl-link' ? info.linkUrl : info.srcUrl;
    if (!isHttp(url)) return fail(tabId);
    const item = await buildItem({
      url, referer: info.frameUrl || info.pageUrl || (tab && tab.url), pageTitle: tab && tab.title,
      incognito: tab && tab.incognito,
    });
    const r = await handover([item]);
    r.ok ? ok(tabId) : fail(tabId);
  } catch { fail(tabId); }
});

async function openLinkPicker(info, tab) {
  if (!tab) return;
  const [res] = await chrome.scripting.executeScript({ target: { tabId: tab.id }, func: collectLinks });
  const links = (res && res.result) || [];
  const k = Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
  await session.set({ ['links:' + k]: { page: info.pageUrl || tab.url, title: tab.title, incognito: tab.incognito, links } });
  await chrome.windows.create({
    url: chrome.runtime.getURL('links.html?k=' + k), type: 'popup', width: 760, height: 640, focused: true,
  });
}

// Runs inside the page (injected on demand).
function collectLinks() {
  const seen = new Set(), out = [];
  for (const a of document.querySelectorAll('a[href]')) {
    if (!/^https?:$/.test(a.protocol)) continue;
    const u = a.href.split('#')[0];
    if (seen.has(u)) continue;
    seen.add(u);
    out.push({ url: u, text: (a.textContent || a.title || '').trim().replace(/\s+/g, ' ').slice(0, 120) });
    if (out.length >= 3000) break;
  }
  return out;
}

/* ---------- download capture ---------- */
const names = new Map();   // downloadId -> {name, mime} from onDeterminingFilename
const waiters = new Map(); // downloadId -> wake-up

// Only observes (no suggestion is made, so the browser picks the name as usual).
chrome.downloads.onDeterminingFilename.addListener(item => {
  if (names.size > 50) names.clear();
  names.set(item.id, { name: baseName(item.filename), mime: item.mime });
  const w = waiters.get(item.id);
  if (w) w();
});

function waitName(item, ms) {
  if (item.filename) return Promise.resolve({ name: baseName(item.filename), mime: item.mime });
  const take = () => { const g = names.get(item.id); names.delete(item.id); return g; };
  if (names.has(item.id)) return Promise.resolve(take());
  return new Promise(resolve => {
    const t = setTimeout(() => { waiters.delete(item.id); resolve(null); }, ms);
    waiters.set(item.id, () => { clearTimeout(t); waiters.delete(item.id); resolve(take()); });
  });
}

chrome.runtime.onMessage.addListener((m, sender, reply) => {
  if (!m || typeof m !== 'object') return;
  if (m.t === 'alt') {
    attempt(session.set({ altAt: Date.now() }));
  } else if (m.t === 'offer-choice') {
    onChoice(m).then(reply, e => reply({ ok: false, error: String(e) }));
    return true;
  } else if (m.t === 'panel-click') {
    onPanelClick(sender).then(reply, e => reply({ ok: false, error: String(e) }));
    return true;
  }
});

chrome.downloads.onCreated.addListener(item => { attempt(onCreated(item)); });

async function onCreated(item) {
  if (item.incognito || item.byExtensionId || item.state !== 'in_progress') return;
  const url = item.finalUrl || item.url;
  if (!isHttp(url)) return;
  const [s, decisions, { altAt }] = await Promise.all([getSettings(), getDecisions(), session.get('altAt')]);
  if (!s.enabled) return;
  if (altAt && Date.now() - altAt < 4000) return;
  if (hostMatches(hostOf(url), s.sites) || hostMatches(hostOf(item.referrer), s.sites)) return;

  const got = await waitName(item, 1500);
  const [cur] = await chrome.downloads.search({ id: item.id });
  if (!cur || cur.state !== 'in_progress') return;
  const name = (got && got.name) || nameFromUrl(url);
  const mime = (got && got.mime) || cur.mime;
  const size = cur.fileSize > 0 ? cur.fileSize : cur.totalBytes > 0 ? cur.totalBytes : -1;
  const c = classify(name, mime, size, s);
  if (!c.match || decisions[c.ext] === 'chrome') return;

  await attempt(chrome.downloads.pause(item.id));
  let referrer = item.referrer;
  const [t] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
  if (!referrer) referrer = t && isHttp(t.url) ? t.url : '';
  // The tab's title helps sushLoad group the site; only when that tab is the referring page.
  const title = t && t.title && referrer && (t.url || '').split('#')[0] === referrer.split('#')[0] ? t.title : '';
  const offer = { id: item.id, url, name: name || 'download', size, host: hostOf(url), referrer, title, ext: c.ext };
  if (decisions[c.ext] === 'sush' || !s.askFirst) {
    const r = await takeOver(offer);
    if (r.ok) ok(-1);
    else { attempt(chrome.downloads.resume(item.id)); fail(-1); }
    return;
  }
  await session.set({ ['offer:' + item.id]: offer });
  const w = await chrome.windows.create({
    url: chrome.runtime.getURL('offer.html?id=' + item.id), type: 'popup', width: 420, height: 250, focused: true,
  });
  await session.set({ ['offerwin:' + w.id]: item.id });
}

async function takeOver(offer) {
  const item = await buildItem({ url: offer.url, fileName: offer.name, referer: offer.referrer, pageTitle: offer.title });
  const r = await handover([item]);
  if (r.ok) {
    // A small file may have finished before it could be paused: drop that copy too.
    const [cur] = await chrome.downloads.search({ id: offer.id });
    if (cur && cur.state === 'complete' && cur.exists) await attempt(chrome.downloads.removeFile(offer.id));
    await attempt(chrome.downloads.cancel(offer.id));
    await attempt(chrome.downloads.erase({ id: offer.id }));
  }
  return r;
}

async function popOffer(id) {
  const key = 'offer:' + id;
  const o = (await session.get(key))[key];
  await session.remove(key);
  return o;
}

async function onChoice({ id, choice, remember }) {
  const o = await popOffer(id);
  if (!o) return { ok: true };
  if (remember && o.ext) {
    const d = await getDecisions();
    d[o.ext] = choice === 'sush' ? 'sush' : 'chrome';
    await saveDecisions(d);
  }
  if (choice !== 'sush') {
    await attempt(chrome.downloads.resume(id));
    return { ok: true };
  }
  const r = await takeOver(o);
  if (r.ok) return { ok: true };
  await attempt(chrome.downloads.resume(id));
  return { ok: false, error: describeError(r) };
}

// Closing the offer window without choosing = let the browser carry on.
chrome.windows.onRemoved.addListener(async windowId => {
  const key = 'offerwin:' + windowId;
  const id = (await session.get(key))[key];
  if (id === undefined) return;
  await session.remove(key);
  if (await popOffer(id)) await attempt(chrome.downloads.resume(id));
});

/* ---------- video panel ---------- */
const SEGMENT_EXT = /\.(m3u8|mpd|ts|m4s|cmfv|cmfa|aac|vtt)$/;
const SEGMENT_CT = /mpegurl|dash\+xml|mp2t|vnd\.mpeg\.dash/;
const SEGMENT_QUERY = /[?&](range|rn|sq|seg|segment|bytestart|byteend)=/i;
const MEDIA_EXT = /\.(mp4|webm|mkv|mov|m4v|avi|flv|mp3|m4a|ogg|opus|wav|flac)$/;
let mediaQueue = Promise.resolve();

chrome.webRequest.onResponseStarted.addListener(d => {
  mediaQueue = mediaQueue.then(() => onMedia(d)).catch(() => undefined);
}, { urls: ['http://*/*', 'https://*/*'], types: ['media'] }, ['responseHeaders']);

function mediaInfo(d) {
  const h = n => {
    const x = (d.responseHeaders || []).find(r => r.name.toLowerCase() === n);
    return x ? String(x.value || '') : '';
  };
  const ct = h('content-type').toLowerCase().split(';')[0].trim();
  let path = '';
  try { path = new URL(d.url).pathname.toLowerCase(); } catch { return null; }
  if (SEGMENT_EXT.test(path) || SEGMENT_CT.test(ct) || SEGMENT_QUERY.test(d.url)) return null;
  const isMedia = ct.startsWith('video/') || ct.startsWith('audio/') ||
    ((ct === 'application/octet-stream' || ct === 'binary/octet-stream') && MEDIA_EXT.test(path));
  if (!isMedia) return null;
  const range = /\/(\d+)$/.exec(h('content-range'));
  const len = Number(h('content-length'));
  const size = range ? Number(range[1]) : d.statusCode === 200 && len > 0 ? len : -1;
  if (size >= 0 && size < 300 * 1024) return null; // ads, previews, sound effects
  return { url: d.url, size, ct };
}

async function onMedia(d) {
  if (d.tabId < 0) return;
  const info = mediaInfo(d);
  if (!info) return;
  const key = 'media:' + d.tabId;
  const list = (await session.get(key))[key] || [];
  const isNew = !list.some(e => e.url === info.url);
  let tab;
  try { tab = await chrome.tabs.get(d.tabId); } catch { return; }
  if (isNew) {
    list.unshift({ ...info, page: (tab.url || '').split('#')[0], t: Date.now() });
    list.length = Math.min(list.length, 6);
    await session.set({ [key]: list });
    pruneMedia();
  }
  const strings = {
    label: msg('panelDownload'), sent: msg('panelSent'), failed: msg('panelFailed'), hide: msg('panelHide'),
    dir: msg('@@bidi_dir'),
  };
  if (isNew || d.frameId === 0) {
    try {
      await chrome.scripting.executeScript({
        target: { tabId: d.tabId, frameIds: [d.frameId] }, func: showPanel, args: [strings],
      });
    } catch { /* page can't be scripted (chrome://, PDF viewer, frame gone) */ }
  }
}

async function pruneMedia() {
  const all = await session.get(null);
  const keys = Object.keys(all).filter(k => k.startsWith('media:'));
  if (keys.length <= 40) return;
  keys.sort((a, b) => (all[a][0] ? all[a][0].t : 0) - (all[b][0] ? all[b][0].t : 0));
  await session.remove(keys.slice(0, keys.length - 40));
}

async function onPanelClick(sender) {
  const tab = sender.tab;
  if (!tab) return { ok: false };
  const key = 'media:' + tab.id;
  let list = (await session.get(key))[key] || [];
  const here = (tab.url || '').split('#')[0];
  const current = list.filter(e => e.page === here);
  if (current.length) list = current;
  if (!list.length) return { ok: false };
  const best = [...list].sort((a, b) => (b.size - a.size) || (b.t - a.t))[0];
  const item = await buildItem({ url: best.url, referer: tab.url, pageTitle: tab.title, incognito: tab.incognito });
  const r = await handover([item]);
  r.ok ? ok(tab.id) : fail(tab.id);
  return { ok: r.ok, error: r.ok ? undefined : describeError(r) };
}

// Injected into the page that loaded the media file. Must be self-contained.
function showPanel(strings) {
  if (window.__sushloadHidden) return;
  if (window.__sushloadPanel) { window.__sushloadPanel.kick(); return; }

  const host = document.createElement('div');
  host.style.cssText = 'position:fixed;top:0;left:0;width:0;height:0;z-index:2147483647;';
  const root = host.attachShadow({ mode: 'closed' });
  root.innerHTML = '<style>' +
    '.p{position:fixed;display:none;align-items:center;gap:2px;background:#2a2a2d;border:1px solid #38383c;' +
    'border-radius:7px;box-shadow:0 4px 14px rgba(0,0,0,.45);font:13px -apple-system,"SF Pro Text",system-ui,sans-serif;' +
    'color:#f5f5f7;padding:2px;opacity:.92}.p:hover{opacity:1}' +
    'button{font:inherit;color:inherit;background:none;border:0;border-radius:5px;padding:4px 9px;cursor:pointer}' +
    '.go{background:#0a84ff;color:#fff;font-weight:600}.go:hover{background:#2a94ff}' +
    '.x{color:#98989d;padding:4px 7px}.x:hover{background:#38383c;color:#f5f5f7}</style>' +
    '<div class="p" dir="' + strings.dir + '"><button class="go"></button><button class="x" title=""></button></div>';
  const pill = root.querySelector('.p'), go = root.querySelector('.go'), x = root.querySelector('.x');
  go.textContent = strings.label;
  x.textContent = '×';
  x.title = strings.hide;

  const pick = () => {
    let best = null, area = 0;
    for (const v of document.querySelectorAll('video')) {
      const r = v.getBoundingClientRect();
      if (r.width < 160 || r.height < 90 || r.bottom <= 0 || r.top >= innerHeight || r.right <= 0 || r.left >= innerWidth) continue;
      if (r.width * r.height > area) { area = r.width * r.height; best = r; }
    }
    return best;
  };
  let raf = 0;
  const place = () => {
    raf = 0;
    const r = pick();
    if (!r) { pill.style.display = 'none'; return; }
    pill.style.display = 'flex';
    const right = document.documentElement.clientWidth - Math.min(r.right, document.documentElement.clientWidth);
    pill.style.top = Math.max(6, r.top + 8) + 'px';
    pill.style.right = right + 8 + 'px';
  };
  const schedule = () => { if (!raf) raf = requestAnimationFrame(place); };
  const mount = () => {
    const parent = document.fullscreenElement || document.documentElement;
    if (host.parentNode !== parent) parent.appendChild(host);
    schedule();
  };
  const timers = [];
  const cleanup = () => {
    timers.forEach(clearTimeout);
    removeEventListener('scroll', schedule, true);
    removeEventListener('resize', schedule);
    document.removeEventListener('fullscreenchange', mount);
    host.remove();
    window.__sushloadPanel = null;
  };
  addEventListener('scroll', schedule, { capture: true, passive: true });
  addEventListener('resize', schedule);
  document.addEventListener('fullscreenchange', mount);
  // The <video> may appear a moment after the media request starts.
  const kick = () => { mount(); for (const ms of [500, 1500, 4000]) timers.push(setTimeout(schedule, ms)); };
  window.__sushloadPanel = { kick };

  x.addEventListener('click', () => { window.__sushloadHidden = true; cleanup(); });
  go.addEventListener('click', () => {
    go.textContent = '…';
    chrome.runtime.sendMessage({ t: 'panel-click' }, r => {
      const good = !chrome.runtime.lastError && r && r.ok;
      go.textContent = good ? strings.sent : strings.failed;
      setTimeout(() => { go.textContent = strings.label; }, 2500);
    });
  });
  kick();
}
