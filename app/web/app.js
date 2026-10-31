(() => {
'use strict';

/* ---------- bootstrap: token, app mode ---------- */
const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const store = {
  get(k) { try { return sessionStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { sessionStorage.setItem(k, v); } catch { /* private mode */ } },
};
const lstore = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch { /* ignore */ } },
};
const params = new URLSearchParams(location.search);
if (params.get('t')) store.set('sushload_token', params.get('t'));
if (params.get('app') === '1') store.set('sushload_app', '1');
if (params.has('t') || params.has('app')) history.replaceState(null, '', location.pathname);
const TOKEN = store.get('sushload_token') || '';
const APP_MODE = store.get('sushload_app') === '1';
if (APP_MODE) document.getElementById('app').classList.add('app');

/* ---------- i18n ---------- */
const I18N = {
  en: {
    all: 'All', downloading: 'Downloading', queued: 'Queued', completed: 'Completed', pausedFailed: 'Paused / Failed',
    categories: 'Categories', video: 'Video', music: 'Music', documents: 'Documents', archives: 'Archives',
    programs: 'Programs', images: 'Images', other: 'Other', settings: 'Settings', filter: 'Filter…',
    pauseAll: 'Pause all', resumeAll: 'Resume all', addUrl: 'Add URL…', resume: 'Resume', pause: 'Pause', remove: 'Remove',
    name: 'Name', size: 'Size', progress: 'Progress', speed: 'Speed', eta: 'ETA', status: 'Status',
    file: 'File', url: 'URL', path: 'Path', connections: 'Connections', resumable: 'Resumable', error: 'Error',
    downloadFolder: 'Download folder', choose: 'Choose…', sortCats: 'Sort into category folders',
    sortCatsHint: 'Video, Music, Documents… each in its own sub-folder.',
    maxConcurrent: 'Concurrent downloads', connsPer: 'Connections per download', speedLimit: 'Speed limit',
    unlimited: 'Unlimited', custom: 'Custom', language: 'Language',
    connecting: 'Connecting', paused: 'Paused', failed: 'Failed',
    idle: 'Idle', limitedTo: 'Limited to {v}', nDownloading: '{n} downloading', nActive: '{n} active',
    nothingActive: 'Nothing active', yes: 'Yes', no: 'No',
    emptyTitle: 'No downloads yet', emptyText: 'Paste a link or press ⌘N to start.',
    emptyFilter: 'Nothing here', emptyFilterText: 'No downloads match this view.',
    addTitle: 'Add downloads', addHint: 'One or more links, one per line.', cancel: 'Cancel', add: 'Add',
    addedN: 'Added {n}', confirmOne: 'Remove “{name}” from the list?', confirmMany: 'Remove {n} downloads from the list?',
    confirmFileOne: 'Remove “{name}” and delete its file?', confirmFileMany: 'Remove {n} downloads and delete their files?',
    open: 'Open', reveal: 'Show in Finder', copyUrl: 'Copy URL', removeWithFile: 'Remove with file',
    selectAll: 'Select all', clearCompleted: 'Clear completed', saved: 'Saved', offline: 'Lost connection to sushLoad. Reopen the app.',
    dropHere: 'Drop link to download', copied: 'Copied', allDone: '{n} total', unknownSize: '—',
    paused_view: 'Paused / Failed', settingsSub: 'Preferences', nItems: '{n} items',
    proxy: 'Proxy', proxyMode: 'Connection', proxySystem: 'System proxy', proxyOff: 'No proxy', proxyCustom: 'Custom',
    proxyType: 'Type', typeHttp: 'HTTP', typeHttps: 'HTTPS', typeSocks5: 'SOCKS5', typeSocks5h: 'SOCKS5 + remote DNS',
    host: 'Host', port: 'Port', username: 'Username', password: 'Password', optional: 'Optional', apply: 'Apply',
    proxyHint: 'Applies to new connections; running downloads keep their connection until paused.', via: 'via {v}',
    browser: 'Browser', extension: 'Extension', extensionStatus: 'Install it once to send downloads from Chrome, Brave and Edge.',
    showExtFolder: 'Show extension folder',
    extStep1: '1. Open chrome://extensions (or brave://extensions, edge://extensions).',
    extStep2: '2. Turn on Developer mode.',
    extStep3: '3. Click “Load unpacked” and pick the folder Finder just showed.',
    fileInfoTitle: 'Download File Info', ofN: '{i} of {n}', fileNameL: 'File name', categoryL: 'Category',
    saveTo: 'Save to', change: 'Change…', rememberFor: 'Remember this folder for {cat}',
    sizeL: 'Size', checking: 'Checking…', resumableYes: 'resumable', noResume: 'no resume', sizeUnknown: 'unknown size',
    probeFailed: 'Couldn’t check the link: {e}. You can still start the download.',
    existsMsg: 'A file with this name exists', renameAuto: 'Rename automatically', overwriteL: 'Overwrite',
    dontAsk: 'Don’t ask again', startDownload: 'Start download', downloadLater: 'Download later',
    startN: 'Start {n} downloads', batchTitle: '{n} files', categoryFolders: 'category folders',
    startedN: 'Started {n}', laterN: 'Added {n} as paused',
    confirmSetting: 'Ask before starting a download',
    confirmSettingHint: 'Shows “Download File Info” for new downloads: name, category and folder.',
    notifySetting: 'Notify when a download finishes', notifySettingHint: 'A macOS notification when a download completes or fails.',
    catFolders: 'Remembered folders', catFoldersNone: 'None yet. Tick “Remember this folder” in the download dialog.',
    siteFolders: 'Site folders', siteFoldersHint: 'Also file downloads by the site they came from, e.g. Documents/University/sharif.edu.',
    siteOff: 'Off', siteSite: 'By site', siteGroupSite: 'By group and site',
    siteAi: 'Use Apple Intelligence for unknown sites', siteAiHint: 'Asked once per site, on this Mac only. Known sites never need it.',
    siteAiMissing: 'Not available: the tgsort-ai helper is missing or Apple Intelligence is off.',
    siteTable: 'Sites', sitesNone: 'None yet. Sites appear here as you download from them. Changes apply to new downloads.',
    siteAuto: 'auto', siteYou: 'you', siteAiBadge: 'AI',
    grp_university: 'University', grp_government: 'Government', grp_software: 'Software', grp_cloud_storage: 'Cloud storage',
    grp_video: 'Video', grp_music: 'Music', grp_news: 'News', grp_shopping: 'Shopping', grp_social: 'Social',
    grp_books_papers: 'Books & papers', grp_other: 'Other',
    reset: 'Reset', logs: 'Log', openLogs: 'Open log folder', logsHint: 'sushload.log, rotated at 1 MiB.',
  },
  fa: {
    all: 'همه', downloading: 'در حال دانلود', queued: 'در صف', completed: 'کامل‌شده', pausedFailed: 'متوقف / ناموفق',
    categories: 'دسته‌ها', video: 'ویدیو', music: 'موسیقی', documents: 'اسناد', archives: 'فشرده',
    programs: 'برنامه‌ها', images: 'تصاویر', other: 'سایر', settings: 'تنظیمات', filter: 'جستجو…',
    pauseAll: 'توقف همه', resumeAll: 'ادامه همه', addUrl: 'افزودن لینک…', resume: 'ادامه', pause: 'توقف', remove: 'حذف',
    name: 'نام', size: 'حجم', progress: 'پیشرفت', speed: 'سرعت', eta: 'زمان باقی‌مانده', status: 'وضعیت',
    file: 'فایل', url: 'نشانی', path: 'مسیر', connections: 'اتصال‌ها', resumable: 'قابل ادامه', error: 'خطا',
    downloadFolder: 'پوشه دانلود', choose: 'انتخاب…', sortCats: 'مرتب‌سازی در پوشه‌های دسته‌بندی',
    sortCatsHint: 'ویدیو، موسیقی، اسناد و … هرکدام در پوشهٔ خودشان.',
    maxConcurrent: 'دانلود همزمان', connsPer: 'اتصال به ازای هر دانلود', speedLimit: 'محدودیت سرعت',
    unlimited: 'بدون محدودیت', custom: 'دلخواه', language: 'زبان',
    connecting: 'در حال اتصال', paused: 'متوقف', failed: 'ناموفق',
    idle: 'بیکار', limitedTo: 'محدود به {v}', nDownloading: '{n} دانلود فعال', nActive: '{n} فعال',
    nothingActive: 'چیزی فعال نیست', yes: 'بله', no: 'خیر',
    emptyTitle: 'هنوز دانلودی نیست', emptyText: 'یک لینک بچسبانید یا ⌘N را بزنید.',
    emptyFilter: 'اینجا چیزی نیست', emptyFilterText: 'هیچ دانلودی با این نما مطابقت ندارد.',
    addTitle: 'افزودن دانلود', addHint: 'یک یا چند لینک، هر کدام در یک خط.', cancel: 'انصراف', add: 'افزودن',
    addedN: '{n} مورد اضافه شد', confirmOne: '«{name}» از فهرست حذف شود؟', confirmMany: '{n} دانلود از فهرست حذف شوند؟',
    confirmFileOne: '«{name}» و فایل آن حذف شود؟', confirmFileMany: '{n} دانلود و فایل‌هایشان حذف شوند؟',
    open: 'باز کردن', reveal: 'نمایش در Finder', copyUrl: 'کپی نشانی', removeWithFile: 'حذف همراه فایل',
    selectAll: 'انتخاب همه', clearCompleted: 'پاک‌کردن کامل‌شده‌ها', saved: 'ذخیره شد', offline: 'ارتباط با sushLoad قطع شد. برنامه را دوباره باز کنید.',
    dropHere: 'لینک را اینجا رها کنید', copied: 'کپی شد', allDone: '{n} مورد', unknownSize: '—',
    paused_view: 'متوقف / ناموفق', settingsSub: 'ترجیحات', nItems: '{n} مورد',
    proxy: 'پروکسی', proxyMode: 'اتصال', proxySystem: 'پروکسی سیستم', proxyOff: 'بدون پروکسی', proxyCustom: 'دلخواه',
    proxyType: 'نوع', typeHttp: 'HTTP', typeHttps: 'HTTPS', typeSocks5: 'SOCKS5', typeSocks5h: 'SOCKS5 + DNS از راه دور',
    host: 'میزبان', port: 'درگاه', username: 'نام کاربری', password: 'گذرواژه', optional: 'اختیاری', apply: 'اعمال',
    proxyHint: 'برای اتصال‌های جدید اعمال می‌شود؛ دانلودهای در حال اجرا تا زمان توقف، اتصال قبلی را نگه می‌دارند.', via: 'از طریق {v}',
    browser: 'مرورگر', extension: 'افزونه', extensionStatus: 'یک بار نصبش کنید تا دانلودهای Chrome، Brave و Edge به sushLoad بیایند.',
    showExtFolder: 'نمایش پوشهٔ افزونه',
    extStep1: '۱. صفحهٔ chrome://extensions را باز کنید (یا brave://extensions، edge://extensions).',
    extStep2: '۲. حالت Developer mode را روشن کنید.',
    extStep3: '۳. روی «Load unpacked» بزنید و همان پوشه‌ای را که Finder نشان داد انتخاب کنید.',
    fileInfoTitle: 'اطلاعات فایل دانلود', ofN: '{i} از {n}', fileNameL: 'نام فایل', categoryL: 'دسته',
    saveTo: 'ذخیره در', change: 'تغییر…', rememberFor: 'این پوشه را برای «{cat}» به خاطر بسپار',
    sizeL: 'حجم', checking: 'در حال بررسی…', resumableYes: 'قابل ادامه', noResume: 'بدون قابلیت ادامه', sizeUnknown: 'حجم نامشخص',
    probeFailed: 'بررسی لینک ممکن نشد: {e}. همچنان می‌توانید دانلود را شروع کنید.',
    existsMsg: 'فایلی با این نام وجود دارد', renameAuto: 'تغییر نام خودکار', overwriteL: 'جایگزینی',
    dontAsk: 'دیگر نپرس', startDownload: 'شروع دانلود', downloadLater: 'بعداً دانلود کن',
    startN: 'شروع {n} دانلود', batchTitle: '{n} فایل', categoryFolders: 'پوشه‌های دسته‌بندی',
    startedN: '{n} دانلود شروع شد', laterN: '{n} مورد متوقف اضافه شد',
    confirmSetting: 'پرسش پیش از شروع دانلود',
    confirmSettingHint: 'برای دانلودهای تازه پنجرهٔ «اطلاعات فایل دانلود» با نام، دسته و پوشه نشان داده می‌شود.',
    notifySetting: 'اعلان پایان دانلود', notifySettingHint: 'هنگام کامل شدن یا ناموفق ماندن دانلود، یک اعلان macOS نشان داده می‌شود.',
    catFolders: 'پوشه‌های ذخیره‌شده', catFoldersNone: 'هنوز چیزی نیست. در پنجرهٔ دانلود «این پوشه را به خاطر بسپار» را بزنید.',
    siteFolders: 'پوشه‌بندی بر اساس سایت', siteFoldersHint: 'دانلودها را بر اساس سایتی که از آن آمده‌اند هم جدا می‌کند، مثلاً Documents/University/sharif.edu.',
    siteOff: 'خاموش', siteSite: 'بر اساس سایت', siteGroupSite: 'بر اساس گروه و سایت',
    siteAi: 'استفاده از Apple Intelligence برای سایت‌های ناشناس', siteAiHint: 'برای هر سایت فقط یک بار و فقط روی همین مک پرسیده می‌شود. سایت‌های شناخته‌شده به آن نیاز ندارند.',
    siteAiMissing: 'در دسترس نیست: ابزار tgsort-ai نصب نیست یا Apple Intelligence خاموش است.',
    siteTable: 'سایت‌ها', sitesNone: 'هنوز چیزی نیست. با دانلود از هر سایت، همین‌جا اضافه می‌شود. تغییرات برای دانلودهای تازه اعمال می‌شود.',
    siteAuto: 'خودکار', siteYou: 'شما', siteAiBadge: 'هوش مصنوعی',
    grp_university: 'دانشگاه', grp_government: 'دولتی', grp_software: 'نرم‌افزار', grp_cloud_storage: 'فضای ابری',
    grp_video: 'ویدیو', grp_music: 'موسیقی', grp_news: 'اخبار', grp_shopping: 'خرید', grp_social: 'شبکه اجتماعی',
    grp_books_papers: 'کتاب و مقاله', grp_other: 'سایر',
    reset: 'بازنشانی', logs: 'گزارش', openLogs: 'باز کردن پوشهٔ گزارش', logsHint: 'sushload.log، با چرخش در ۱ مگابایت.',
  },
};
let lang = lstore.get('sushload_lang') === 'fa' ? 'fa' : 'en';
const t = (k, v) => {
  let s = (I18N[lang] && I18N[lang][k]) || I18N.en[k] || k;
  if (v) for (const [a, b] of Object.entries(v)) s = s.replace('{' + a + '}', b);
  return s;
};

/* ---------- formatting ---------- */
// In Persian (RTL) the Latin number+unit must stay one left-to-right run.
const iso = s => lang === 'fa' ? '\u2066' + s + '\u2069' : s;
function fmtBytes(n, trim) {
  if (n == null) return '—';
  if (n < 1024) return iso(Math.round(n) + ' B');
  const u = ['KB', 'MB', 'GB', 'TB'];
  let v = n / 1024, i = 0;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  const s = trim ? String(+v.toFixed(2)) : v.toFixed(v >= 100 ? 0 : v >= 10 ? 1 : 2);
  return iso(s + ' ' + u[i]);
}
const fmtSpeed = (n, trim) => iso(fmtBytes(n, trim).replace(/[\u2066\u2069]/g, '') + '/s');
function fmtEta(s) {
  if (s == null) return '';
  s = Math.round(s);
  if (s < 60) return iso(s + 's');
  if (s < 3600) return iso(Math.floor(s / 60) + 'm ' + String(s % 60).padStart(2, '0') + 's');
  if (s < 86400) return iso(Math.floor(s / 3600) + 'h ' + String(Math.floor((s % 3600) / 60)).padStart(2, '0') + 'm');
  return iso(Math.floor(s / 86400) + 'd ' + Math.floor((s % 86400) / 3600) + 'h');
}
const pctOf = d => d.status === 'completed' ? 100 : (d.total ? Math.min(100, d.received / d.total * 100) : 0);

/* ---------- api ---------- */
let offline = false;
async function api(path, body) {
  const opt = { headers: { 'X-Sushload-Token': TOKEN } };
  if (body !== undefined) {
    opt.method = 'POST';
    opt.headers['Content-Type'] = 'application/json';
    opt.body = JSON.stringify(body);
  }
  const r = await fetch(path, opt);
  let j = null;
  try { j = await r.json(); } catch { /* empty */ }
  if (!r.ok) throw new Error((j && j.error) || ('HTTP ' + r.status));
  return j;
}

/* ---------- state ---------- */
let state = { active: false, downloads: [], settings: null, pending: [], default_dirs: {} };
let byId = new Map();
let view = 'all';
let search = '';
let selected = new Set();
let anchor = null;
const recs = new Map(); // id -> row record

const STATUS_GROUP = {
  downloading: d => d.status === 'downloading' || d.status === 'connecting',
  queued: d => d.status === 'queued',
  completed: d => d.status === 'completed',
  paused: d => d.status === 'paused' || d.status === 'failed',
};
function inView(d, v) {
  if (v === 'all') return true;
  if (v.startsWith('cat:')) return d.category === v.slice(4);
  return STATUS_GROUP[v] ? STATUS_GROUP[v](d) : true;
}
function visibleList() {
  const q = search.trim().toLowerCase();
  return state.downloads.filter(d => inView(d, view) &&
    (!q || d.file_name.toLowerCase().includes(q) || d.url.toLowerCase().includes(q)));
}

/* ---------- polling ---------- */
let timer = null, inflight = false, again = false;
const wantPoll = () => state.active && !offline && document.visibilityState === 'visible';
function schedule() {
  clearTimeout(timer); timer = null;
  if (wantPoll()) timer = setTimeout(refresh, 500);
}
async function refresh() {
  clearTimeout(timer); timer = null;
  if (inflight) { again = true; return; }
  inflight = true;
  try {
    const s = await api('/api/state');
    offline = false;
    state = s;
    byId = new Map(s.downloads.map(d => [d.id, d]));
    render();
    maybeShowPending();
  } catch (e) {
    if (!offline) toast(t('offline'));
    offline = true;
  } finally {
    inflight = false;
  }
  if (again) { again = false; return refresh(); }
  schedule();
}
document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible') { offline = false; refresh(); } else { clearTimeout(timer); timer = null; }
});

/* ---------- rendering ---------- */
const tbody = $('#tbody');
const catIcons = {};
for (const c of ['video', 'music', 'documents', 'archives', 'programs', 'images', 'other']) {
  catIcons[c] = $('[data-view="cat:' + c + '"] svg').outerHTML;
}

function setText(el, key, rec, val) {
  if (rec.last[key] !== val) { rec.last[key] = val; el.textContent = val; }
}

function makeRow(d) {
  const el = document.createElement('div');
  el.className = 'row';
  el.dataset.id = d.id;
  el.innerHTML = '<div class="c-name"><span class="ic"></span><span class="nm"></span></div>' +
    '<div class="c-size"></div><div class="c-prog"><div class="bar"></div><span class="pct"></span></div>' +
    '<div class="c-speed"></div><div class="c-eta"></div><div class="c-status"><span class="pill"></span></div>';
  const rec = {
    el, last: {},
    ic: $('.ic', el), nm: $('.nm', el), size: $('.c-size', el), bar: $('.bar', el), pct: $('.pct', el),
    speed: $('.c-speed', el), eta: $('.c-eta', el), pill: $('.pill', el),
  };
  return rec;
}

function paintBar(bar, d, rec) {
  const cls = 'bar ' + (bar.dataset.big ? 'big ' : '') + d.status + (d.total == null && d.status !== 'completed' ? ' unknown' : '');
  if (bar.className !== cls) bar.className = cls;
  let parts = [];
  if (d.status === 'completed') parts = [[0, 100]];
  else if (d.total && d.segments && d.segments.length > 1) {
    for (const s of d.segments) {
      const len = s.end - s.start + 1;
      parts.push([s.start / d.total * 100, Math.min(s.received, len) / d.total * 100]);
    }
  } else if (d.total) parts = [[0, Math.min(100, d.received / d.total * 100)]];
  const sig = parts.map(p => p[0].toFixed(2) + ':' + p[1].toFixed(2)).join('|') + cls;
  if (rec.barSig === sig) return;
  rec.barSig = sig;
  while (bar.children.length > parts.length) bar.lastChild.remove();
  while (bar.children.length < parts.length) bar.appendChild(document.createElement('i'));
  parts.forEach((p, i) => {
    const s = bar.children[i].style;
    s.insetInlineStart = p[0] + '%';
    s.width = p[1] + '%';
  });
}

const statusLabel = s => t(s);

function updateRow(rec, d) {
  const l = rec.last;
  if (l.cat !== d.category) { l.cat = d.category; rec.ic.innerHTML = catIcons[d.category] || catIcons.other; }
  setText(rec.nm, 'nm', rec, d.file_name);
  rec.nm.title = d.file_name;
  setText(rec.size, 'size', rec, d.total != null ? fmtBytes(d.total) : (d.received ? fmtBytes(d.received) : '—'));
  setText(rec.pct, 'pct', rec, d.total || d.status === 'completed' ? Math.floor(pctOf(d)) + '%' : '');
  paintBar(rec.bar, d, rec);
  setText(rec.speed, 'speed', rec, d.status === 'downloading' && d.speed ? fmtSpeed(d.speed) : '');
  setText(rec.eta, 'eta', rec, d.status === 'downloading' ? fmtEta(d.eta_secs) : '');
  setText(rec.pill, 'pill', rec, statusLabel(d.status));
  if (l.pillCls !== d.status) { l.pillCls = d.status; rec.pill.className = 'pill pill-' + d.status; }
  rec.pill.title = d.error || '';
}

function renderTable(list) {
  const keep = new Set(list.map(d => d.id));
  for (const [id, rec] of recs) {
    if (!keep.has(id)) { rec.el.remove(); recs.delete(id); }
  }
  let ref = tbody.firstChild;
  for (const d of list) {
    let rec = recs.get(d.id);
    if (!rec) { rec = makeRow(d); recs.set(d.id, rec); }
    updateRow(rec, d);
    if (rec.el === ref) ref = ref.nextSibling;
    else tbody.insertBefore(rec.el, ref);
  }
  paintSelection();
}

function paintSelection() {
  for (const [id, rec] of recs) rec.el.classList.toggle('sel', selected.has(id));
}

const NAV_KEYS = ['all', 'downloading', 'queued', 'completed', 'paused', 'cat:video', 'cat:music', 'cat:documents', 'cat:archives', 'cat:programs', 'cat:images', 'cat:other'];
function renderCounts() {
  for (const k of NAV_KEYS) {
    const n = state.downloads.reduce((a, d) => a + (inView(d, k) ? 1 : 0), 0);
    const el = $('[data-count="' + k + '"]');
    const s = n ? String(n) : '';
    if (el.textContent !== s) el.textContent = s;
  }
}

function viewTitle() {
  if (view === 'settings') return t('settings');
  if (view === 'paused') return t('pausedFailed');
  if (view.startsWith('cat:')) return t(view.slice(4));
  return t(view);
}

function renderHeader(list) {
  $('#title').textContent = viewTitle();
  if (view === 'settings') { $('#subtitle').textContent = t('settingsSub'); return; }
  const dl = state.downloads.filter(d => d.status === 'downloading' || d.status === 'connecting');
  const total = state.downloads.reduce((a, d) => a + (d.speed || 0), 0);
  $('#subtitle').textContent = dl.length
    ? t('nDownloading', { n: dl.length }) + ' · ' + fmtSpeed(total)
    : t('nItems', { n: list.length });
}

function renderStatus() {
  const total = state.downloads.reduce((a, d) => a + (d.speed || 0), 0);
  const act = state.downloads.filter(d => d.status === 'downloading' || d.status === 'connecting' || d.status === 'queued').length;
  const running = state.downloads.some(d => d.status === 'downloading' || d.status === 'connecting');
  const limit = state.settings ? state.settings.speed_limit : 0;
  $('#speedBig').textContent = fmtSpeed(total);
  const note = $('#proxyNote');
  const pv = state.settings && state.settings.proxy_mode === 'custom' ? parseProxy(state.settings.proxy_url) : null;
  if (pv) { note.hidden = false; note.textContent = t('via', { v: PROXY_LABEL[pv.scheme] + ' ' + pv.host + ':' + pv.port }); }
  else note.hidden = true;
  $('#activeText').textContent = act ? t('nActive', { n: act }) : t('nothingActive');
  const pill = $('#statePill');
  let txt, cls;
  if (limit > 0) { txt = t('limitedTo', { v: fmtSpeed(limit, true) }); cls = 'pill-limited'; }
  else if (running) { txt = t('downloading'); cls = 'pill-downloading'; }
  else { txt = t('idle'); cls = 'pill-idle'; }
  if (pill.textContent !== txt) pill.textContent = txt;
  const full = 'pill ' + cls;
  if (pill.className !== full) pill.className = full;
  $('#btnPauseAll').disabled = !state.downloads.some(d => d.status === 'queued' || d.status === 'connecting' || d.status === 'downloading');
  $('#btnResumeAll').disabled = !state.downloads.some(d => d.status === 'paused' || d.status === 'failed');
}

function selectedList() { return [...selected].map(id => byId.get(id)).filter(Boolean); }
const canPause = d => d.status === 'queued' || d.status === 'connecting' || d.status === 'downloading';
const canResume = d => d.status === 'paused' || d.status === 'failed';

function renderButtons() {
  const sel = selectedList();
  $('#btnResume').disabled = !sel.some(canResume);
  $('#btnPause').disabled = !sel.some(canPause);
  $('#btnRemove').disabled = sel.length === 0;
}

function renderDrawer() {
  const sel = selectedList();
  const drawer = $('#drawer');
  if (sel.length !== 1) { drawer.hidden = true; return; }
  const d = sel[0];
  drawer.hidden = false;
  const set = (id, v) => { const el = $(id); if (el.textContent !== v) el.textContent = v; };
  set('#dFile', d.file_name);
  set('#dUrl', d.url); $('#dUrl').title = d.url;
  set('#dPath', d.path); $('#dPath').title = d.path;
  set('#dConn', String(d.connections));
  set('#dRes', d.resumable ? t('yes') : t('no'));
  set('#dErr', d.error || '');
  $('#dErr').hidden = $('#dErrK').hidden = !d.error;
  const rec = drawerRec;
  paintBar($('#dBar'), d, rec);
}
const drawerRec = { barSig: '' };

function renderEmpty(list) {
  const empty = list.length === 0;
  $('#empty').hidden = !empty;
  $('#tbody').hidden = empty;
  if (empty) {
    const none = state.downloads.length === 0;
    $('#emptyTitle').textContent = none ? t('emptyTitle') : t('emptyFilter');
    $('#emptyText').textContent = none ? t('emptyText') : t('emptyFilterText');
    $('#btnAdd2').hidden = !none;
  }
}

function render() {
  for (const id of [...selected]) if (!byId.has(id)) selected.delete(id);
  const list = visibleList();
  renderCounts();
  renderHeader(list);
  renderStatus();
  renderEmpty(list);
  renderTable(list);
  renderButtons();
  renderDrawer();
  renderSettingsIfIdle();
}

/* ---------- navigation ---------- */
function setView(v) {
  view = v;
  $$('.nav-item').forEach(b => b.classList.toggle('active', b.dataset.view === v));
  const settings = v === 'settings';
  $('#page-downloads').hidden = settings;
  $('#page-settings').hidden = !settings;
  $('#filter').hidden = settings;
  if (settings) { fillSettings(true); loadSites(); loadAiStatus(); }
  render();
}
$('#nav').addEventListener('click', e => { const b = e.target.closest('.nav-item'); if (b) setView(b.dataset.view); });
$('.sidebar-bottom').addEventListener('click', e => { const b = e.target.closest('.nav-item'); if (b) setView(b.dataset.view); });
$('#filter').addEventListener('input', e => { search = e.target.value; render(); });

/* ---------- selection ---------- */
tbody.addEventListener('click', e => {
  const row = e.target.closest('.row');
  if (!row) return;
  const id = Number(row.dataset.id);
  const order = visibleList().map(d => d.id);
  if (e.shiftKey && anchor != null && order.includes(anchor)) {
    const a = order.indexOf(anchor), b = order.indexOf(id);
    const [lo, hi] = a < b ? [a, b] : [b, a];
    if (!(e.metaKey || e.ctrlKey)) selected.clear();
    for (let i = lo; i <= hi; i++) selected.add(order[i]);
  } else if (e.metaKey || e.ctrlKey) {
    if (selected.has(id)) selected.delete(id); else selected.add(id);
    anchor = id;
  } else {
    selected = new Set([id]);
    anchor = id;
  }
  paintSelection(); renderButtons(); renderDrawer();
});
$('#tableWrap').addEventListener('click', e => {
  if (!e.target.closest('.row') && !e.target.closest('button')) {
    selected.clear(); paintSelection(); renderButtons(); renderDrawer();
  }
});
tbody.addEventListener('dblclick', e => {
  const row = e.target.closest('.row');
  if (!row) return;
  const d = byId.get(Number(row.dataset.id));
  if (d && d.status === 'completed') post('/api/open', { id: d.id });
});
tbody.addEventListener('contextmenu', e => {
  const row = e.target.closest('.row');
  if (!row) return;
  e.preventDefault();
  const id = Number(row.dataset.id);
  if (!selected.has(id)) { selected = new Set([id]); anchor = id; paintSelection(); renderButtons(); renderDrawer(); }
  showMenu(e.clientX, e.clientY, rowMenu());
});

function selectAll() {
  selected = new Set(visibleList().map(d => d.id));
  paintSelection(); renderButtons(); renderDrawer();
}

/* ---------- actions ---------- */
async function post(path, body) {
  try {
    const r = await api(path, body === undefined ? {} : body);
    refresh();
    return r;
  } catch (e) { toast(e.message); return null; }
}
async function each(path, list, extra) {
  try {
    for (const d of list) await api(path, Object.assign({ id: d.id }, extra));
  } catch (e) { toast(e.message); }
  refresh();
}
const doResume = () => each('/api/resume', selectedList().filter(canResume));
const doPause = () => each('/api/pause', selectedList().filter(canPause));

function toggleSelection() {
  const sel = selectedList();
  if (sel.some(canPause)) doPause(); else doResume();
}

async function confirmRemove(withFile) {
  const sel = selectedList();
  if (!sel.length) return;
  const msg = sel.length === 1
    ? t(withFile ? 'confirmFileOne' : 'confirmOne', { name: sel[0].file_name })
    : t(withFile ? 'confirmFileMany' : 'confirmMany', { n: sel.length });
  const ok = await confirmSheet(msg, withFile ? t('removeWithFile') : t('remove'));
  if (!ok) return;
  await each('/api/remove', sel, { delete_file: !!withFile });
  selected.clear();
}

async function copyText(s) {
  try { await navigator.clipboard.writeText(s); }
  catch {
    const ta = document.createElement('textarea');
    ta.value = s; ta.style.position = 'fixed'; ta.style.opacity = '0';
    document.body.appendChild(ta); ta.select();
    try { document.execCommand('copy'); } catch { /* ignore */ }
    ta.remove();
  }
  toast(t('copied'));
}

$('#btnAdd').onclick = $('#btnAdd2').onclick = () => addDialog();
$('#btnResume').onclick = doResume;
$('#btnPause').onclick = doPause;
$('#btnRemove').onclick = () => confirmRemove(false);
$('#btnPauseAll').onclick = () => post('/api/pause_all');
$('#btnResumeAll').onclick = () => post('/api/resume_all');
$('#btnMore').onclick = e => {
  e.stopPropagation();
  const r = e.currentTarget.getBoundingClientRect();
  const sel = selectedList();
  showMenu(lang === 'fa' ? r.right - 190 : r.left, r.bottom + 4, [
    { label: t('selectAll'), fn: selectAll },
    { label: t('copyUrl'), fn: () => copyText(sel.map(d => d.url).join('\n')), off: !sel.length },
    { sep: true },
    { label: t('clearCompleted'), fn: () => each('/api/remove', state.downloads.filter(d => d.status === 'completed'), { delete_file: false }) },
  ]);
};

/* ---------- menus ---------- */
function rowMenu() {
  const sel = selectedList();
  const one = sel.length === 1 ? sel[0] : null;
  return [
    { label: t('open'), fn: () => post('/api/open', { id: one.id }), off: !one || one.status !== 'completed' },
    { label: t('reveal'), fn: () => post('/api/reveal', { id: one.id }), off: !one },
    { sep: true },
    { label: t('pause'), fn: doPause, off: !sel.some(canPause) },
    { label: t('resume'), fn: doResume, off: !sel.some(canResume) },
    { sep: true },
    { label: t('copyUrl'), fn: () => copyText(sel.map(d => d.url).join('\n')) },
    { label: t('remove'), fn: () => confirmRemove(false) },
    { label: t('removeWithFile'), fn: () => confirmRemove(true) },
  ];
}
const ctx = $('#ctx');
function showMenu(x, y, items) {
  ctx.innerHTML = '';
  for (const it of items) {
    if (it.sep) { ctx.appendChild(document.createElement('hr')); continue; }
    const b = document.createElement('button');
    b.textContent = it.label; b.disabled = !!it.off;
    b.onclick = () => { hideMenu(); it.fn(); };
    ctx.appendChild(b);
  }
  ctx.hidden = false;
  const w = ctx.offsetWidth, h = ctx.offsetHeight;
  ctx.style.left = Math.max(4, Math.min(x, innerWidth - w - 4)) + 'px';
  ctx.style.top = Math.max(4, Math.min(y, innerHeight - h - 4)) + 'px';
}
function hideMenu() { ctx.hidden = true; }
document.addEventListener('mousedown', e => { if (!ctx.hidden && !ctx.contains(e.target)) hideMenu(); });
window.addEventListener('blur', hideMenu);
document.addEventListener('scroll', hideMenu, true);

/* ---------- sheets ---------- */
const overlay = $('#overlay'), sheet = $('#sheet');
let sheetClose = null, sheetModal = false;
function openSheet(build, modal) {
  return new Promise(resolve => {
    sheet.innerHTML = '';
    sheet.className = 'sheet' + (modal ? ' wide' : '');
    sheet.onkeydown = null;
    sheetModal = !!modal;
    const close = v => {
      overlay.hidden = true; sheet.innerHTML = ''; sheetClose = null; sheetModal = false; resolve(v);
      setTimeout(maybeShowPending, 0);
    };
    sheetClose = close;
    build(sheet, close);
    overlay.hidden = false;
  });
}
// A click outside dismisses a plain sheet; the download dialog only closes with its buttons or Esc.
overlay.addEventListener('mousedown', e => { if (e.target === overlay && sheetClose && !sheetModal) sheetClose(null); });

function el(tag, cls, text) {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text != null) n.textContent = text;
  return n;
}

function confirmSheet(msg, okLabel) {
  return openSheet((s, close) => {
    s.appendChild(el('div', null, msg));
    const btns = el('div', 'btns');
    const c = el('button', 'btn', t('cancel')); c.onclick = () => close(false);
    const ok = el('button', 'btn danger', okLabel); ok.onclick = () => close(true);
    btns.append(c, ok); s.appendChild(btns);
    ok.focus();
    s.onkeydown = e => { if (e.key === 'Enter') { e.preventDefault(); close(true); } };
  });
}

async function addDialog(prefill) {
  let initial = prefill || '';
  if (!initial) {
    try {
      const txt = (await navigator.clipboard.readText()).trim();
      if (/^https?:\/\/\S+/i.test(txt)) initial = txt;
    } catch { /* clipboard refused */ }
  }
  if (sheetClose) return;
  await openSheet((s, close) => {
    s.appendChild(el('h2', null, t('addTitle')));
    s.appendChild(el('div', 'muted', t('addHint')));
    const ta = el('textarea', 'field');
    ta.placeholder = 'https://…'; ta.spellcheck = false; ta.value = initial; ta.dir = 'ltr';
    s.appendChild(ta);
    const errs = el('div', 'errs'); errs.hidden = true; s.appendChild(errs);
    const btns = el('div', 'btns');
    const cancel = el('button', 'btn', t('cancel')); cancel.onclick = () => close(null);
    const add = el('button', 'btn primary', t('add'));
    const submit = async () => {
      const text = ta.value.trim();
      if (!text) return;
      add.disabled = true;
      try {
        const r = await api('/api/add', { urls: text });
        await refresh();
        if (r.errors.length) {
          const bad = text.split('\n').map(x => x.trim()).filter(x => x && r.errors.some(e => e.startsWith(x + ': ')));
          ta.value = bad.join('\n');
          errs.textContent = r.errors.join('\n'); errs.hidden = false;
          add.disabled = false;
        } else close(true);
      } catch (e) { errs.textContent = e.message; errs.hidden = false; add.disabled = false; }
    };
    add.onclick = submit;
    btns.append(cancel, add); s.appendChild(btns);
    ta.focus(); ta.select();
    s.onkeydown = e => {
      if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); submit(); }
    };
  });
}

let toastTimer = null;
function toast(msg) {
  const el = $('#toast');
  el.textContent = msg; el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { el.hidden = true; }, 3500);
}

async function quickAdd(text) {
  const urls = text.split(/\s+/).filter(x => /^https?:\/\/\S+/i.test(x));
  if (!urls.length) return;
  try {
    const r = await api('/api/add', { urls: urls.join('\n') });
    if (r.queued) { await refresh(); return; }
    toast(r.errors.length ? r.errors[0] : t('addedN', { n: r.added }));
    refresh();
  } catch (e) { toast(e.message); }
}

/* ---------- download confirmation ("Download File Info") ---------- */
const EXT_CAT = {};
for (const [cat, list] of Object.entries({
  video: 'mp4 mkv avi mov webm m4v flv wmv ts 3gp', music: 'mp3 m4a aac flac wav ogg opus wma',
  documents: 'pdf doc docx xls xlsx ppt pptx txt rtf epub odt csv md', archives: 'zip rar 7z tar gz tgz bz2 xz zst iso',
  programs: 'dmg pkg app exe msi deb rpm apk ipa', images: 'jpg jpeg png gif webp heic svg bmp tiff',
})) for (const e of list.split(' ')) EXT_CAT[e] = cat;
const CATS = ['video', 'music', 'documents', 'archives', 'programs', 'images', 'other'];
const catOfName = n => EXT_CAT[(/\.([A-Za-z0-9]+)$/.exec(n) || [])[1]?.toLowerCase()] || 'other';
function nameOfUrl(u) {
  try { return decodeURIComponent(new URL(u).pathname.split('/').filter(Boolean).pop() || '') || 'download'; }
  catch { return 'download'; }
}
const midTrunc = (s, n) => s.length <= n ? s : s.slice(0, Math.ceil(n * 0.6)) + '…' + s.slice(s.length - Math.floor(n * 0.4) + 1);
const withSite = (base, rel) => rel ? base.replace(/\/$/, '') + '/' + rel : base;
const defaultDir = cat => (state.default_dirs && state.default_dirs[cat]) || (state.settings ? state.settings.download_dir : '');

let pendingBusy = false, pendingDone = 0;
async function maybeShowPending() {
  if (pendingBusy || sheetClose || !state.pending || !state.pending.length) return;
  pendingBusy = true;
  try {
    while (state.pending.length && !sheetClose) {
      const p = state.pending[0];
      const n = pendingDone + state.pending.length;
      const res = await confirmDialog(p, pendingDone + 1, n);
      if (res === null) { try { await api('/api/discard', { pending_id: p.id }); } catch { /* already gone */ } }
      pendingDone++;
      state.pending = state.pending.filter(x => x.id !== p.id);
      await refresh();
      if (!state.pending.length) pendingDone = 0;
    }
  } finally { pendingBusy = false; }
}

function confirmDialog(p, pos, total) {
  return openSheet((s, close) => {
    const batch = p.batch;
    const head = el('div', 'fi-head');
    head.append(el('h2', null, batch ? t('fileInfoTitle') + ' · ' + t('batchTitle', { n: p.items.length }) : t('fileInfoTitle')));
    if (total > 1) head.append(el('span', 'fi-count', t('ofN', { i: pos, n: total })));
    s.appendChild(head);

    const grid = el('div', 'fi-grid');
    const row = (label, node) => { grid.append(el('div', 'fi-k', label), node); };
    const err = el('div', 'errs'); err.hidden = true;
    let dir = '', dirChosen = false, catChosen = false, nameInput = null, catSel = null;
    const dirText = el('div', 'mono');
    const rememberChk = el('input'); rememberChk.type = 'checkbox';
    const rememberText = el('span');
    const existsBox = el('div', 'fi-sub'); existsBox.hidden = true;
    let overwrite = false;
    const checks = [];

    const paintDir = () => {
      const shown = dir || (batch && state.settings ? state.settings.download_dir : '');
      dirText.textContent = midTrunc(shown, 60) + (batch && !dir && state.settings && state.settings.sort_into_categories ? '  (' + t('categoryFolders') + ')' : '');
      dirText.title = shown;
      rememberText.textContent = t('rememberFor', { cat: t(catSel ? catSel.value : 'other') });
    };
    const recheck = async () => {
      if (batch || !nameInput) return;
      const name = nameInput.value.trim();
      if (!name || !dir) { existsBox.hidden = true; return; }
      try {
        const r = await api('/api/exists', { dir, file_name: name });
        paintExists(r.exists);
      } catch { /* ignore */ }
    };
    const paintExists = exists => {
      existsBox.hidden = !exists;
      existsBox.innerHTML = '';
      if (!exists) { overwrite = false; return; }
      existsBox.append(el('div', 'fi-note', t('existsMsg')));
      const mk = (val, label) => {
        const l = el('label', 'chk');
        const r = el('input'); r.type = 'radio'; r.name = 'fi-ow'; r.checked = overwrite === val;
        r.onchange = () => { overwrite = val; };
        l.append(r, el('span', null, label)); return l;
      };
      existsBox.append(mk(false, t('renameAuto')), mk(true, t('overwriteL')));
    };

    // URL (read-only, middle-truncated, full text on hover)
    if (!batch) {
      const u = el('div', 'fi-url mono', midTrunc(p.items[0].url, 70)); u.title = p.items[0].url;
      row(t('url'), u);
      nameInput = el('input', 'field'); nameInput.spellcheck = false; nameInput.dir = 'ltr';
      nameInput.value = p.items[0].file_name || nameOfUrl(p.items[0].url);
      row(t('fileNameL'), nameInput);
      catSel = el('select', 'field');
      for (const c of CATS) { const o = el('option', null, t(c)); o.value = c; catSel.append(o); }
      catSel.value = catOfName(nameInput.value);
      row(t('categoryL'), catSel);
      dir = withSite(defaultDir(catSel.value), p.items[0].site_rel);
    } else {
      const list = el('div', 'fi-list fi-sub');
      p.items.forEach(it => {
        const l = el('label', 'fi-item');
        const cb = el('input'); cb.type = 'checkbox'; cb.checked = true; cb.onchange = paintButtons;
        const name = it.file_name || nameOfUrl(it.url);
        const nm = el('span', 'nm', name); nm.title = it.url;
        l.append(cb, nm, el('span', 'cat', t(catOfName(name))));
        checks.push({ cb, it, name }); list.append(l);
      });
      grid.append(list);
    }

    const chg = el('button', 'btn', t('change'));
    chg.onclick = async () => {
      try { const r = await api('/api/choose_dir', {}); if (r && r.path) { dir = r.path; dirChosen = true; paintDir(); recheck(); } }
      catch (e) { toast(e.message); }
    };
    const dirRow = el('div', 'fi-dir'); dirRow.append(dirText, chg);
    row(t('saveTo'), dirRow);
    if (!batch) {
      const rem = el('label', 'chk fi-sub'); rem.append(rememberChk, rememberText); grid.append(rem);
    }
    const sizeEl = el('div', 'fi-size muted', batch ? '' : t('checking'));
    if (!batch) row(t('sizeL'), sizeEl);
    grid.append(existsBox);
    s.appendChild(grid);
    s.appendChild(err);

    const askBox = el('label', 'chk'); const ask = el('input'); ask.type = 'checkbox';
    askBox.append(ask, el('span', null, t('dontAsk')));
    s.appendChild(askBox);

    const btns = el('div', 'btns');
    const cancel = el('button', 'btn', t('cancel')); cancel.onclick = () => close(null);
    const later = el('button', 'btn', t('downloadLater'));
    const start = el('button', 'btn primary', t('startDownload'));
    btns.append(cancel, later, start); s.appendChild(btns);

    function paintButtons() {
      if (!batch) return;
      const n = checks.filter(c => c.cb.checked).length;
      start.textContent = t('startN', { n }); start.disabled = later.disabled = n === 0;
    }

    const submit = async mode => {
      start.disabled = later.disabled = cancel.disabled = true;
      err.hidden = true;
      const items = batch
        ? checks.filter(c => c.cb.checked).map(c => ({ index: c.it.index, category: catOfName(c.name), file_name: c.it.file_name || undefined }))
        : [{ index: 0, file_name: nameInput.value.trim(), category: catSel.value, dir, overwrite }];
      try {
        const r = await api('/api/confirm', {
          pending_id: p.id, items, start_paused: mode === 'later', dont_ask: ask.checked,
          remember_dir: !batch && rememberChk.checked, dir: batch && dirChosen ? dir : undefined,
        });
        if (r.errors && r.errors.length) toast(r.errors[0]);
        else toast(t(mode === 'later' ? 'laterN' : 'startedN', { n: r.added }));
        close(mode);
      } catch (e) {
        err.textContent = e.message; err.hidden = false;
        start.disabled = later.disabled = cancel.disabled = false; paintButtons();
      }
    };
    start.onclick = () => submit('start');
    later.onclick = () => submit('later');
    s.onkeydown = e => {
      if (e.key === 'Enter' && !e.isComposing && e.target.tagName !== 'BUTTON' && !start.disabled) { e.preventDefault(); submit('start'); }
    };

    if (!batch) {
      catSel.onchange = () => { catChosen = true; if (!dirChosen) dir = withSite(defaultDir(catSel.value), p.items[0].site_rel); paintDir(); recheck(); };
      nameInput.onchange = () => {
        if (!catChosen) catSel.value = catOfName(nameInput.value);
        if (!dirChosen) dir = withSite(defaultDir(catSel.value), p.items[0].site_rel);
        paintDir(); recheck();
      };
      // One quick look at the link: size, resumability, name from the server, existing file.
      (async () => {
        try {
          const r = await api('/api/probe', { pending_id: p.id, index: 0 });
          if (!p.items[0].file_name && document.activeElement !== nameInput) nameInput.value = r.file_name;
          if (!catChosen) catSel.value = r.category;
          if (!dirChosen) dir = r.dir;
          sizeEl.textContent = (r.total != null ? fmtBytes(r.total) : t('sizeUnknown')) + ' · ' + (r.resumable ? t('resumableYes') : t('noResume'));
          sizeEl.classList.remove('muted');
          paintDir(); recheck();
        } catch (e) {
          sizeEl.textContent = t('probeFailed', { e: e.message });
          sizeEl.classList.add('err');
        }
      })();
    }
    paintDir(); paintButtons(); recheck();
    (nameInput || start).focus();
    if (nameInput) nameInput.select();
  }, true);
}
window.sushloadPending = () => refresh();

/* ---------- drop / paste / keyboard ---------- */
let dragDepth = 0;
const body = document.body;
document.addEventListener('dragenter', e => { e.preventDefault(); if (++dragDepth === 1) { body.dataset.drop = t('dropHere'); body.classList.add('dropping'); } });
document.addEventListener('dragleave', () => { if (--dragDepth <= 0) { dragDepth = 0; body.classList.remove('dropping'); } });
document.addEventListener('dragover', e => e.preventDefault());
document.addEventListener('drop', e => {
  e.preventDefault(); dragDepth = 0; body.classList.remove('dropping');
  const dt = e.dataTransfer;
  const txt = dt.getData('text/uri-list') || dt.getData('text/plain') || dt.getData('URL');
  if (txt) quickAdd(txt);
});
const isEditable = n => n && (n.tagName === 'INPUT' || n.tagName === 'TEXTAREA' || n.tagName === 'SELECT' || n.isContentEditable);
document.addEventListener('paste', e => {
  if (isEditable(e.target) || sheetClose) return;
  const txt = e.clipboardData && e.clipboardData.getData('text');
  if (txt) { e.preventDefault(); quickAdd(txt); }
});
document.addEventListener('keydown', e => {
  if (e.key === 'Escape') {
    if (sheetClose) { sheetClose(null); return; }
    if (!ctx.hidden) { hideMenu(); return; }
  }
  if (sheetClose) return;
  const mod = e.metaKey || e.ctrlKey;
  if (mod && e.key.toLowerCase() === 'n') { e.preventDefault(); addDialog(); return; }
  if (isEditable(e.target)) return;
  if (mod && e.key.toLowerCase() === 'a') { e.preventDefault(); selectAll(); return; }
  if (view === 'settings') return;
  if (e.key === ' ') { e.preventDefault(); toggleSelection(); }
  else if (e.key === 'Backspace' || e.key === 'Delete') { e.preventDefault(); confirmRemove(e.altKey); }
});

/* ---------- proxy helpers ---------- */
const PROXY_LABEL = { http: 'HTTP', https: 'HTTPS', socks5: 'SOCKS5', socks5h: 'SOCKS5' };
const dec = s => { try { return decodeURIComponent(s); } catch { return s; } };
// scheme://[user[:pass]@]host:port  ->  parts (credentials are never shown outside the form)
function parseProxy(url) {
  const m = /^([a-z0-9]+):\/\/(.*)$/i.exec((url || '').trim());
  if (!m) return null;
  let rest = m[2].replace(/\/+$/, '');
  let user = '', pass = '';
  const at = rest.lastIndexOf('@');
  if (at >= 0) {
    const cred = rest.slice(0, at); rest = rest.slice(at + 1);
    const c = cred.indexOf(':');
    user = dec(c >= 0 ? cred.slice(0, c) : cred); pass = c >= 0 ? dec(cred.slice(c + 1)) : '';
  }
  const p = rest.lastIndexOf(':');
  if (p < 0) return null;
  const scheme = m[1].toLowerCase();
  if (!PROXY_LABEL[scheme]) return null;
  return { scheme, host: rest.slice(0, p), port: rest.slice(p + 1), user, pass };
}
function buildProxy() {
  const host = $('#pHost').value.trim(), port = $('#pPort').value.trim();
  if (!host && !port) return '';
  const user = $('#pUser').value, pass = $('#pPass').value;
  let cred = '';
  if (user || pass) cred = encodeURIComponent(user) + (pass ? ':' + encodeURIComponent(pass) : '') + '@';
  return $('#pType').value + '://' + cred + host + ':' + port;
}

/* ---------- settings page ---------- */
const S = {
  confirm: $('#sConfirm'), notify: $('#sNotify'), siteMode: $('#sSiteMode'), siteAi: $('#sSiteAi'),
  dir: $('#sDir'), sort: $('#sSort'), conc: $('#sConc'), conns: $('#sConns'),
  mode: $('#sLimitMode'), val: $('#sLimitVal'), unit: $('#sLimitUnit'), lang: $('#sLang'),
};
let pDirty = false;
function fillSettings(force) {
  const s = state.settings;
  if (!s) return;
  if (!force && $('#page-settings').contains(document.activeElement) && isEditable(document.activeElement)) return;
  S.dir.value = s.download_dir;
  S.sort.setAttribute('aria-checked', String(!!s.sort_into_categories));
  S.confirm.setAttribute('aria-checked', String(s.confirm_new_downloads !== false));
  S.notify.setAttribute('aria-checked', String(s.notify_on_complete !== false));
  S.siteMode.value = s.site_folders || 'off';
  S.siteAi.setAttribute('aria-checked', String(s.ai_site_groups !== false));
  $('#siteMore').hidden = S.siteMode.value === 'off';
  $('#siteAiRow').hidden = S.siteMode.value !== 'group_and_site';
  paintSites();
  paintCatDirs(s.category_dirs || {});
  S.conc.value = s.max_concurrent;
  S.conns.value = s.connections;
  if (s.speed_limit > 0) {
    S.mode.value = 'custom';
    const mb = s.speed_limit % 1048576 === 0;
    S.unit.value = mb ? 'MB' : 'KB';
    S.val.value = +(s.speed_limit / (mb ? 1048576 : 1024)).toFixed(2);
  } else { S.mode.value = 'unlimited'; }
  S.val.disabled = S.unit.disabled = S.mode.value === 'unlimited';
  S.lang.value = lang;
  if (pDirty && !force) return;
  $('#pMode').value = s.proxy_mode || 'system';
  $('#pCustom').hidden = $('#pMode').value !== 'custom';
  const pp = parseProxy(s.proxy_url);
  if (pp) {
    $('#pType').value = pp.scheme; $('#pHost').value = pp.host; $('#pPort').value = pp.port;
    $('#pUser').value = pp.user; $('#pPass').value = pp.pass;
  }
}
function paintCatDirs(map) {
  const box = $('#catDirs');
  const keys = CATS.filter(c => map[c]);
  const sig = keys.map(c => c + '=' + map[c]).join('|') + lang;
  if (box.dataset.sig === sig) return;
  box.dataset.sig = sig;
  box.innerHTML = '';
  $('#catDirsNone').hidden = keys.length > 0;
  for (const c of keys) {
    const r = el('div', 'catdir');
    const path = el('span', 'mono', map[c]); path.title = map[c];
    const b = el('button', 'btn', t('reset'));
    b.onclick = () => { const next = Object.assign({}, state.settings.category_dirs); delete next[c]; saveSettings(next); };
    r.append(el('span', null, t(c)), path, b);
    box.append(r);
  }
}
/* ---------- site folders: the Sites table ---------- */
let sitesList = [], aiChecked = false;
async function loadSites() {
  try { sitesList = (await api('/api/sites')).sites || []; } catch { sitesList = []; }
  paintSites();
}
async function loadAiStatus() {
  if (aiChecked) return;
  aiChecked = true;
  try {
    const r = await api('/api/ai_status');
    $('#siteAiNote').hidden = !!r.available;
    S.siteAi.disabled = !r.available;
    S.siteAi.style.opacity = r.available ? '' : '0.4';
  } catch { aiChecked = false; }
}
async function saveSite(domain, group, folder) {
  try { sitesList = (await api('/api/sites', { domain, group: group || '', folder: folder || '' })).sites || []; }
  catch (e) { toast(e.message); }
  paintSites(true);
}
function paintSites(force) {
  const box = $('#sitesBox');
  const mode = state.settings ? state.settings.site_folders : 'off';
  const sig = JSON.stringify(sitesList) + mode + lang;
  if (!force && box.dataset.sig === sig) return;
  if (!force && box.contains(document.activeElement) && isEditable(document.activeElement)) return;
  box.dataset.sig = sig;
  box.innerHTML = '';
  box.classList.toggle('nogroup', mode !== 'group_and_site');
  $('#sitesNone').hidden = sitesList.length > 0;
  for (const s of sitesList) {
    const r = el('div', 'site');
    const name = el('span', 'mono', s.domain); name.title = s.domain;
    const sel = el('select', 'field');
    for (const g of GROUPS) { const o = el('option', null, t('grp_' + g)); o.value = g; sel.append(o); }
    sel.value = s.group;
    sel.onchange = () => saveSite(s.domain, sel.value, s.user_folder);
    const folder = el('input', 'field'); folder.dir = 'ltr'; folder.spellcheck = false; folder.value = s.folder;
    folder.onchange = () => saveSite(s.domain, s.user_group, folder.value.trim());
    const badge = el('span', 'badge ' + s.origin, t(s.origin === 'you' ? 'siteYou' : s.origin === 'ai' ? 'siteAiBadge' : 'siteAuto'));
    const reset = el('button', 'btn', t('reset'));
    reset.disabled = !s.user_group && !s.user_folder;
    reset.onclick = async () => {
      try { sitesList = (await api('/api/sites_reset', { domain: s.domain })).sites || []; } catch (e) { toast(e.message); }
      paintSites(true);
    };
    r.append(name, sel, folder, badge, reset);
    box.append(r);
  }
}
const GROUPS = ['university', 'government', 'software', 'cloud_storage', 'video', 'music', 'news', 'shopping', 'social', 'books_papers', 'other'];
function renderSettingsIfIdle() { if (view === 'settings') fillSettings(false); }

async function saveSettings(categoryDirs) {
  const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, Math.round(Number(v)) || lo));
  const limited = S.mode.value === 'custom';
  const v = Math.max(0.01, Number(S.val.value) || 1);
  const next = {
    download_dir: S.dir.value.trim(),
    sort_into_categories: S.sort.getAttribute('aria-checked') === 'true',
    max_concurrent: clamp(S.conc.value, 1, 10),
    connections: clamp(S.conns.value, 1, 16),
    speed_limit: limited ? Math.round(v * (S.unit.value === 'MB' ? 1048576 : 1024)) : 0,
    proxy_mode: $('#pMode').value,
    proxy_url: $('#pMode').value === 'custom' ? buildProxy() : (state.settings && state.settings.proxy_url) || '',
    confirm_new_downloads: S.confirm.getAttribute('aria-checked') === 'true',
    notify_on_complete: S.notify.getAttribute('aria-checked') === 'true',
    site_folders: S.siteMode.value,
    ai_site_groups: S.siteAi.getAttribute('aria-checked') === 'true',
    category_dirs: (categoryDirs && typeof categoryDirs === 'object' && !(categoryDirs instanceof Event))
      ? categoryDirs : ((state.settings && state.settings.category_dirs) || {}),
  };
  const msg = $('#sMsg');
  const pmsg = $('#pMsg');
  pmsg.textContent = '';
  try {
    const r = await api('/api/settings', next);
    state.settings = r.settings;
    pDirty = false;
    msg.textContent = t('saved');
    if (!$('#pCustom').hidden) { pmsg.textContent = t('saved'); pmsg.style.color = 'var(--ok)'; }
    setTimeout(() => { if (msg.textContent === t('saved')) msg.textContent = ''; }, 1500);
  } catch (e) {
    msg.textContent = e.message;
    pmsg.textContent = e.message; pmsg.style.color = 'var(--err)';
    refresh(); return;
  }
  fillSettings(true);
  refresh();
}
[S.dir, S.conc, S.conns, S.val].forEach(i => i.addEventListener('change', saveSettings));
[S.mode, S.unit, S.siteMode].forEach(i => i.addEventListener('change', saveSettings));
S.dir.addEventListener('keydown', e => { if (e.key === 'Enter') S.dir.blur(); });
S.sort.addEventListener('click', () => {
  S.sort.setAttribute('aria-checked', String(S.sort.getAttribute('aria-checked') !== 'true'));
  saveSettings();
});
for (const sw of [S.confirm, S.notify, S.siteAi]) {
  sw.addEventListener('click', () => {
    sw.setAttribute('aria-checked', String(sw.getAttribute('aria-checked') !== 'true'));
    saveSettings();
  });
}
$('#bLogs').addEventListener('click', async () => {
  try { await api('/api/open_logs', {}); } catch (e) { toast(e.message); }
});
$('#sChoose').addEventListener('click', async () => {
  try {
    const r = await api('/api/choose_dir', {});
    if (r && r.path) { S.dir.value = r.path; saveSettings(); }
  } catch (e) { toast(e.message); }
});
$('#bShowExt').addEventListener('click', async () => {
  try { await api('/api/reveal_extension', {}); } catch (e) { toast(e.message); }
});
$('#pMode').addEventListener('change', () => {
  const custom = $('#pMode').value === 'custom';
  $('#pCustom').hidden = !custom;
  $('#sMsg').textContent = '';
  // Custom is only saved with Apply (so half-typed hosts are never sent); the others save at once.
  if (!custom || parseProxy(state.settings && state.settings.proxy_url)) saveSettings(); else pDirty = true;
});
$('#pApply').addEventListener('click', saveSettings);
['#pType', '#pHost', '#pPort', '#pUser', '#pPass'].forEach(i => $(i).addEventListener('input', () => { pDirty = true; }));
S.lang.addEventListener('change', () => { lang = S.lang.value === 'fa' ? 'fa' : 'en'; lstore.set('sushload_lang', lang); applyLang(); });

function applyLang() {
  const rtl = lang === 'fa';
  document.documentElement.lang = lang;
  document.documentElement.dir = rtl ? 'rtl' : 'ltr';
  document.body.classList.toggle('rtl', rtl);
  $('#app').classList.toggle('rtl', rtl);
  $$('[data-i18n]').forEach(n => { n.textContent = t(n.dataset.i18n); });
  $$('[data-i18n-ph]').forEach(n => { n.placeholder = t(n.dataset.i18nPh); });
  $('#brand').textContent = 'sushLoad';
  document.title = 'sushLoad';
  // Rows hold cached localized text: rebuild them.
  for (const rec of recs.values()) rec.el.remove();
  recs.clear();
  fillSettings(true);
  render();
}

/* ---------- hooks for the native shell ---------- */
window.sushloadGo = v => setView(v);
window.sushloadSelectAll = () => {
  const a = document.activeElement;
  if (isEditable(a) && a.select) a.select(); else selectAll();
};
if (APP_MODE) {
  const post = m => { try { window.webkit.messageHandlers.sushload.postMessage(m); } catch { /* not in the app */ } };
  const bar = $('#dragbar');
  bar.addEventListener('mousedown', e => { if (e.button === 0) post(e.detail === 2 ? 'zoom' : 'drag'); });
}

/* ---------- start ---------- */
$$('.nav-item')[0].classList.add('active');
applyLang();
refresh();
})();
