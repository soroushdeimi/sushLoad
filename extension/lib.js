// Shared by the service worker and the extension pages. No external requests.
export const HOST = 'com.soroushdeimi.sushload';

export const msg = (key, subs) => chrome.i18n.getMessage(key, subs) || key;

/* ---------- file types ---------- */
const split = s => s.split(' ');
export const CATEGORIES = {
  archives: split('zip rar 7z tar gz tgz bz2 xz zst z cab lz lzma'),
  programs: split('exe msi apk ipa deb rpm appimage dmg pkg iso img'),
  video: split('mp4 mkv avi mov wmv flv webm m4v mpg mpeg 3gp'),
  music: split('mp3 flac wav aac m4a ogg opus wma'),
  documents: split('pdf doc docx xls xlsx ppt pptx epub mobi odt ods odp rtf'),
  images: split('jpg jpeg png gif webp svg bmp tif tiff heic ico'),
};
export const CATEGORY_ORDER = ['archives', 'programs', 'video', 'music', 'documents', 'images', 'other'];
const EXT_CATEGORY = new Map(Object.entries(CATEGORIES).flatMap(([c, l]) => l.map(e => [e, c])));
export const categoryOf = ext => EXT_CATEGORY.get(ext) || 'other';

export const DEFAULTS = {
  enabled: true,
  askFirst: false,
  exts: [...CATEGORIES.archives, ...CATEGORIES.programs, ...CATEGORIES.video, ...CATEGORIES.music],
  docExts: [...CATEGORIES.documents],
  minSizeMB: 1,
  sites: [],
};

export async function getSettings() {
  const { settings } = await chrome.storage.local.get('settings');
  return { ...DEFAULTS, ...(settings || {}) };
}
export const saveSettings = settings => chrome.storage.local.set({ settings });
export async function getDecisions() {
  const { decisions } = await chrome.storage.local.get('decisions');
  return decisions || {};
}
export const saveDecisions = decisions => chrome.storage.local.set({ decisions });

export function parseList(text) {
  return [...new Set(String(text).toLowerCase().split(/[\s,;]+/).map(s => s.replace(/^\*?\./, '')).filter(Boolean))];
}
export function parseHosts(text) {
  return [...new Set(String(text).toLowerCase().split(/[\s,;]+/)
    .map(s => s.replace(/^https?:\/\//, '').replace(/[/:?#].*$/, '').replace(/^\*?\./, '')).filter(Boolean))];
}

export const isHttp = u => /^https?:\/\//i.test(u || '');
export function hostOf(u) { try { return new URL(u).hostname.toLowerCase(); } catch { return ''; } }
export const hostMatches = (host, list) => !!host && list.some(h => host === h || host.endsWith('.' + h));
export function extOf(name) {
  const m = /\.([A-Za-z0-9]{1,12})$/.exec(name || '');
  return m ? m[1].toLowerCase() : '';
}
export function baseName(path) { return String(path || '').split(/[\\/]/).pop(); }
export function nameFromUrl(u) {
  try {
    const last = new URL(u).pathname.split('/').pop();
    return last ? decodeURIComponent(last) : '';
  } catch { return ''; }
}

/** Decides whether a browser download is one sushLoad should take. `size` < 0 means unknown. */
export function classify(name, mime, size, s) {
  let ext = extOf(name);
  if (ext && s.exts.includes(ext)) return { match: true, ext };
  if (ext && s.docExts.includes(ext)) {
    return { match: size >= s.minSizeMB * 1048576, ext };
  }
  const m = (mime || '').toLowerCase();
  if (!ext && (m.startsWith('video/') || m.startsWith('audio/'))) {
    ext = m.startsWith('video/') ? 'video' : 'audio';
    return { match: true, ext };
  }
  return { match: false, ext };
}

export function fmtSize(n) {
  if (!(n > 0)) return msg('offerUnknownSize');
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let v = n, i = 0;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return (i === 0 ? String(v) : v.toFixed(v >= 100 ? 0 : v >= 10 ? 1 : 2)) + ' ' + u[i];
}

/* ---------- handing over to the app ---------- */
export function sendNative(message) {
  return new Promise(resolve => {
    try {
      chrome.runtime.sendNativeMessage(HOST, message, reply => {
        const e = chrome.runtime.lastError;
        if (e) resolve({ ok: false, error: e.message, noHost: true });
        else resolve(reply || { ok: false, error: 'empty reply' });
      });
    } catch (e) { resolve({ ok: false, error: String(e) }); }
  });
}

export function describeError(r) {
  if (r && r.noHost) return msg('errNoHost');
  return (r && r.error) || 'unknown error';
}

/** `name=value; …` for the URL (the browser's own jar), or ''. Never logged. */
export async function cookieHeader(url, incognito) {
  try {
    const q = { url };
    if (incognito) {
      const stores = await chrome.cookies.getAllCookieStores();
      const s = stores.find(x => x.id !== '0');
      if (s) q.storeId = s.id;
    }
    const list = await chrome.cookies.getAll(q);
    const h = list.map(c => c.name + '=' + c.value).join('; ');
    return h.length <= 16000 ? h : '';
  } catch { return ''; }
}

export async function buildItem({ url, fileName, referer, incognito, cache }) {
  let cookies = '';
  if (cache) {
    const key = hostOf(url);
    if (!cache.has(key)) cache.set(key, await cookieHeader(url, incognito));
    cookies = cache.get(key);
  } else cookies = await cookieHeader(url, incognito);
  return {
    url,
    file_name: fileName || null,
    referer: referer || null,
    cookies: cookies || null,
    user_agent: navigator.userAgent,
  };
}

/** Sends items to the app in chunks that stay under the 1 MB native-message limit. */
export async function handover(items) {
  const chunks = [];
  let cur = [], size = 0;
  for (const it of items) {
    const n = JSON.stringify(it).length + 2;
    if (cur.length && size + n > 600000) { chunks.push(cur); cur = []; size = 0; }
    cur.push(it); size += n;
  }
  if (cur.length) chunks.push(cur);
  let added = 0, queued = 0, confirm = false, last = null;
  for (const c of chunks) {
    const r = await sendNative({ type: 'add', items: c });
    if (!r.ok) return added ? { ok: true, added, queued, confirm, error: describeError(r) } : { ...r, ok: false };
    added += r.added || 0;
    queued += r.queued || 0;
    confirm = confirm || !!r.confirm;
    last = r;
  }
  // `confirm`: the app is asking the user (file name, folder…) before the downloads start.
  return { ok: true, added, queued, confirm, errors: last && last.errors };
}

/* ---------- page helpers ---------- */
export function applyI18n(root = document) {
  document.documentElement.lang = chrome.i18n.getUILanguage();
  document.documentElement.dir = msg('@@bidi_dir');
  root.querySelectorAll('[data-i18n]').forEach(n => { n.textContent = msg(n.dataset.i18n); });
  root.querySelectorAll('[data-i18n-ph]').forEach(n => { n.placeholder = msg(n.dataset.i18nPh); });
}
