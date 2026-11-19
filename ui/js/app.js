'use strict';
/* Offline Convertor - UI logic. Vanilla JS on top of Tauri 2 (window.__TAURI__). */

const T = window.__TAURI__;
const invoke = (cmd, args) => T.core.invoke(cmd, args);
const uiLog = (msg) => { try { invoke('ui_log', { msg: String(msg) }); } catch {} };
window.addEventListener('error', (e) => uiLog(`error: ${e.message} @ ${e.filename}:${e.lineno}`));
window.addEventListener('unhandledrejection', (e) => uiLog(`rejection: ${e.reason?.message || e.reason}`));
const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

// ── Formatting ───────────────────────────────────────────────────────
function fmtSize(b) {
  if (!b && b !== 0) return '';
  const u = ['Б', 'КБ', 'МБ', 'ГБ', 'ТБ'];
  let i = 0, v = b;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${v >= 100 || i === 0 ? Math.round(v) : v.toFixed(v >= 10 ? 1 : 2).replace('.', ',')} ${u[i]}`;
}
function fmtTime(s) {
  if (!s && s !== 0) return '';
  s = Math.round(s);
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), x = s % 60;
  const p = (n) => String(n).padStart(2, '0');
  return h ? `${h}:${p(m)}:${p(x)}` : `${m}:${p(x)}`;
}
function parseTime(str) {
  if (str == null) return null;
  str = String(str).trim().replace(',', '.');
  if (!str) return null;
  const parts = str.split(':').map(Number);
  if (parts.some((n) => Number.isNaN(n))) return null;
  return parts.reduce((acc, n) => acc * 60 + n, 0);
}
function plural(n, one, few, many) {
  const a = Math.abs(n) % 100, b = a % 10;
  if (a > 10 && a < 20) return many;
  if (b > 1 && b < 5) return few;
  if (b === 1) return one;
  return many;
}

// ── Static data ──────────────────────────────────────────────────────
const KIND_ICON = { video: 'video', audio: 'music', image: 'image', pdf: 'pdf', other: 'file' };
const EXT = {
  video: ['mp4', 'mkv', 'mov', 'avi', 'webm', 'flv', 'wmv', 'm4v', 'mpg', 'mpeg', 'ts', 'mts', 'm2ts', '3gp', '3g2', 'ogv', 'vob', 'mxf', 'f4v', 'asf', 'rm', 'rmvb', 'divx', 'dv'],
  audio: ['mp3', 'wav', 'flac', 'aac', 'm4a', 'ogg', 'oga', 'opus', 'wma', 'aiff', 'aif', 'alac', 'ac3', 'eac3', 'dts', 'amr', 'ape', 'wv', 'mka', 'mp2', 'caf', 'tta', 'weba'],
  image: ['jpg', 'jpeg', 'jpe', 'jfif', 'png', 'webp', 'avif', 'heic', 'heif', 'jxl', 'gif', 'bmp', 'tif', 'tiff', 'ico', 'tga', 'psd', 'qoi', 'pnm', 'ppm', 'pgm', 'hdr', 'dds', 'jp2', 'exr'],
  pdf: ['pdf'],
};

const QUALITY_HINT = 'Выберите качество итогового файла. Чем выше качество, тем больше размер файла. Понижение качества уменьшит размер файла';
// Compression level → the engine's 0..100 quality (calibrated by VMAF/SSIM: 85 = visually lossless)
const LEVEL_Q = { high: 93, balanced: 85, strong: 72 };
const levelOf = (q) => (q >= 90 ? 'high' : q >= 78 ? 'balanced' : 'strong');
const levelField = (hints, show) => ({
  type: 'seg', key: 'level', label: 'Степень сжатия', show,
  options: [['high', 'Слабая'], ['balanced', 'Оптимальная'], ['strong', 'Сильная']], hints,
});
const VIDEO_LEVEL_HINTS = {
  high: 'Максимальное качество, файл меньше в 2-4 раза',
  balanced: 'На глаз как исходник, файл меньше в 3-10 раз',
  strong: 'Самый маленький файл, на мелких деталях возможна потеря чёткости',
};
const AUDIO_LEVEL_HINTS = {
  high: 'Звук не отличить от исходника',
  balanced: 'Без слышимых потерь для музыки и речи',
  strong: 'Самый маленький файл, подходит для речи и подкастов',
};
const SIZE_MODE = (what) => ({
  type: 'seg', key: 'size_mode', label: 'Как сжимать',
  options: [['quality', 'По качеству'], ['size', 'До размера'], ['bitrate', `По битрейту`]],
  hints: {
    quality: `Размер подберётся сам - ${what} будет сжато настолько, насколько можно без заметных потерь`,
    size: 'Точный размер итогового файла',
    bitrate: 'Для тех, кто знает нужный битрейт',
  },
});
const RES_OPTIONS = [[0, 'Как у исходника'], [2160, '4K · 2160p'], [1440, '1440p'], [1080, '1080p'], [720, '720p'], [480, '480p'], [360, '360p']];
const FPS_OPTIONS = [[0, 'Как у исходника'], [60, '60 кадров/с'], [50, '50 кадров/с'], [30, '30 кадров/с'], [25, '25 кадров/с'], [24, '24 кадра/с']];

const TOOLS = {
  home: {
    title: 'Пакетное сжатие', nav: 'Пакетное сжатие', icon: 'layers', tool: 'auto',
    accept: ['video', 'audio', 'image', 'pdf'], formats: 'Видео, фото, аудио и PDF - любое количество',
    defaults: { level: 'balanced' },
    fields: [
      { type: 'kinds' },
      levelField({
        high: 'Максимальное качество, файлы уменьшаются меньше',
        balanced: 'Без заметных потерь - лучший баланс размера и качества',
        strong: 'Самые маленькие файлы, на мелких деталях возможны потери',
      }),
      { type: 'note', text: 'Формат, разрешение и прочее для каждого вида файлов берутся из разделов «Сжатие»' },
    ],
  },
  'compress-video': {
    group: 'compress', title: 'Сжатие видео', nav: 'Видео', icon: 'video', tool: 'video', accept: ['video'],
    formats: 'MP4, MKV, MOV, AVI, WEBM и другие',
    defaults: { mode: 'compress', format: 'same', codec: 'auto', size_mode: 'quality', level: 'balanced', target_mb: null, bitrate_kbps: null, height: 0, fps: 0, audio: 'auto', hw: 'auto', speed: 'balanced' },
    fields: [
      { type: 'reco' },
      SIZE_MODE('видео'),
      levelField(VIDEO_LEVEL_HINTS, (o) => o.size_mode === 'quality'),
      { type: 'unit', key: 'target_mb', label: 'Установить размер файла:', placeholder: '1 - 65536', unit: 'MB', min: 1, max: 65536, step: 'any', show: (o) => o.size_mode === 'size',
        info: 'Размер каждого итогового файла вместе со звуком. Если размер слишком мал для разрешения видео, разрешение уменьшится само, чтобы не было кубиков' },
      { type: 'unit', key: 'bitrate_kbps', label: 'Изменить битрейт видео:', placeholder: '1 - 100000', unit: 'kbps', min: 1, max: 100000, step: 1, show: (o) => o.size_mode === 'bitrate' },
      { type: 'estimate', show: (o) => o.size_mode !== 'quality' },
      { type: 'sep' },
      { type: 'select', key: 'height', label: 'Разрешение', options: RES_OPTIONS, num: true },
      { type: 'select', key: 'fps', label: 'Частота кадров', options: FPS_OPTIONS, num: true },
      { type: 'select', key: 'format', label: 'Контейнер', options: [['same', 'Как у исходника'], ['mp4', 'MP4'], ['mkv', 'MKV'], ['webm', 'WEBM'], ['mov', 'MOV']] },
      { type: 'select', key: 'audio', label: 'Звук', options: [['auto', 'Авто (без перекодирования, если можно)'], ['aac', 'AAC'], ['opus', 'Opus'], ['none', 'Без звука']] },
      { type: 'select', key: 'speed', label: 'Режим кодирования', options: [['balanced', 'Быстро и качественно (рекомендуется)'], ['quality', 'Максимальное сжатие - в 2-3 раза медленнее'], ['fast', 'Максимальная скорость']] },
      { type: 'select', key: 'hw', label: 'Ускорение', options: [['auto', 'Видеокарта (с откатом на процессор)'], ['cpu', 'Только процессор']] },
    ],
  },
  'compress-image': {
    group: 'compress', title: 'Сжатие изображений', nav: 'Изображения', icon: 'image', tool: 'image', accept: ['image'],
    formats: 'JPG, PNG, WEBP, AVIF, HEIC, BMP, TIFF',
    defaults: { mode: 'compress', format: 'same', quality: 85, max_side: 0, keep_metadata: true },
    fields: [
      { type: 'slider', key: 'quality', label: 'Качество', hint: QUALITY_HINT },
      { type: 'sep' },
      { type: 'select', key: 'format', label: 'Формат результата', options: [['same', 'Как у исходника'], ['jpg', 'JPG'], ['webp', 'WEBP - меньше JPG'], ['avif', 'AVIF - самый компактный'], ['png', 'PNG']] },
      { type: 'select', key: 'max_side', label: 'Максимальная сторона', options: [[0, 'Как у исходника'], [7680, '7680 px'], [3840, '3840 px'], [2560, '2560 px'], [1920, '1920 px'], [1280, '1280 px']], num: true },
      { type: 'switch', key: 'keep_metadata', label: 'Сохранять EXIF и цветовой профиль' },
    ],
  },
  'compress-audio': {
    group: 'compress', title: 'Сжатие аудио', nav: 'Аудио', icon: 'music', tool: 'audio', accept: ['audio'],
    formats: 'MP3, WAV, FLAC, M4A, OGG, OPUS, WMA',
    defaults: { mode: 'compress', format: 'same', size_mode: 'quality', level: 'balanced', target_mb: null, bitrate_kbps: null },
    fields: [
      SIZE_MODE('аудио'),
      levelField(AUDIO_LEVEL_HINTS, (o) => o.size_mode === 'quality'),
      { type: 'unit', key: 'target_mb', label: 'Установить размер файла:', placeholder: '1 - 65536', unit: 'MB', min: 1, max: 65536, step: 'any', show: (o) => o.size_mode === 'size' },
      { type: 'unit', key: 'bitrate_kbps', label: 'Изменить битрейт аудио:', placeholder: '8 - 512', unit: 'kbps', min: 8, max: 512, step: 1, show: (o) => o.size_mode === 'bitrate' },
      { type: 'sep' },
      { type: 'select', key: 'format', label: 'Формат результата', options: [['same', 'Как у исходника'], ['opus', 'OPUS - лучшее сжатие'], ['mp3', 'MP3'], ['m4a', 'AAC (M4A)']] },
    ],
  },
  'compress-pdf': {
    group: 'compress', title: 'Сжатие PDF', nav: 'PDF', icon: 'pdf', tool: 'pdf', accept: ['pdf'], formats: 'PDF',
    defaults: { quality: 85, max_side: 2339, grayscale: false },
    fields: [
      { type: 'slider', key: 'quality', label: 'Качество изображений', hint: 'Текст и векторная графика не изменяются - пережимаются только фотографии внутри PDF' },
      { type: 'select', key: 'max_side', label: 'Разрешение изображений', options: [[0, 'Без изменений'], [3508, '300 dpi - печать'], [2339, '200 dpi - оптимально'], [1754, '150 dpi - экран'], [1123, '96 dpi - минимум']], num: true },
      { type: 'switch', key: 'grayscale', label: 'Перевести изображения в оттенки серого' },
    ],
  },
  'convert-video': {
    group: 'convert', title: 'Видео-конвертер', nav: 'Видео', icon: 'video', tool: 'video', accept: ['video'],
    formats: 'В MP4, MKV, WEBM, MOV, AVI, FLV, WMV, MPEG, 3GP, TS, M4V, OGV, GIF',
    defaults: { mode: 'convert', format: 'mp4', codec: 'auto', level: 'high', height: 0, fps: 0, audio: 'auto', trim_start: null, trim_end: null, gif_fps: 15, gif_width: 480, hw: 'auto', speed: 'balanced' },
    fields: [
      { type: 'chips', key: 'format', label: 'Конвертировать в', options: ['mp4', 'mkv', 'webm', 'mov', 'avi', 'flv', 'wmv', 'mpeg', '3gp', 'ts', 'm4v', 'ogv', 'gif'].map((f) => [f, f.toUpperCase()]) },
      { type: 'select', key: 'codec', label: 'Видеокодек', show: (o) => ['mp4', 'mkv', 'webm', 'mov', 'm4v', 'ts'].includes(o.format),
        options: [['auto', 'Авто'], ['h264', 'H.264 - максимальная совместимость'], ['hevc', 'HEVC (H.265)'], ['av1', 'AV1'], ['vp9', 'VP9'], ['copy', 'Без перекодирования (мгновенно)']] },
      levelField(VIDEO_LEVEL_HINTS, (o) => o.format !== 'gif' && o.codec !== 'copy'),
      { type: 'select', key: 'gif_fps', label: 'Кадров в секунду (GIF)', options: [[10, '10'], [15, '15'], [20, '20'], [25, '25'], [30, '30']], num: true, show: (o) => o.format === 'gif' },
      { type: 'select', key: 'gif_width', label: 'Ширина GIF', options: [[320, '320 px'], [480, '480 px'], [640, '640 px'], [800, '800 px'], [0, 'Как у исходника']], num: true, show: (o) => o.format === 'gif' },
      { type: 'sep' },
      { type: 'select', key: 'height', label: 'Разрешение', options: RES_OPTIONS, num: true, show: (o) => o.format !== 'gif' && o.codec !== 'copy' },
      { type: 'select', key: 'fps', label: 'Частота кадров', options: FPS_OPTIONS, num: true, show: (o) => o.format !== 'gif' && o.codec !== 'copy' },
      { type: 'select', key: 'audio', label: 'Звук', show: (o) => o.format !== 'gif', options: [['auto', 'Авто'], ['copy', 'Без перекодирования'], ['aac', 'AAC'], ['opus', 'Opus'], ['mp3', 'MP3'], ['none', 'Без звука']] },
      { type: 'trim' },
    ],
  },
  'convert-audio': {
    group: 'convert', title: 'Аудио-конвертер', nav: 'Аудио', icon: 'music', tool: 'audio', accept: ['audio', 'video'],
    formats: 'В MP3, AAC, M4A, OPUS, OGG, FLAC, WAV, AIFF, ALAC, WMA, AC3 · звук из видео',
    defaults: { mode: 'convert', format: 'mp3', bitrate_kbps: 0, sample_rate: 0, channels: 0, normalize: false, bit_depth: 16, trim_start: null, trim_end: null },
    fields: [
      { type: 'chips', key: 'format', label: 'Конвертировать в', options: ['mp3', 'aac', 'm4a', 'opus', 'ogg', 'flac', 'wav', 'aiff', 'alac', 'wma', 'ac3'].map((f) => [f, f.toUpperCase()]) },
      { type: 'select', key: 'bitrate_kbps', label: 'Битрейт', num: true, show: (o) => !['flac', 'wav', 'aiff', 'alac'].includes(o.format),
        options: [[0, 'Авто (высокое качество)'], [320, '320 кбит/с'], [256, '256 кбит/с'], [192, '192 кбит/с'], [160, '160 кбит/с'], [128, '128 кбит/с'], [96, '96 кбит/с'], [64, '64 кбит/с']] },
      { type: 'select', key: 'bit_depth', label: 'Разрядность', num: true, show: (o) => ['flac', 'wav', 'aiff'].includes(o.format), options: [[16, '16 бит'], [24, '24 бит']] },
      { type: 'select', key: 'sample_rate', label: 'Частота дискретизации', num: true, options: [[0, 'Как у исходника'], [48000, '48 000 Гц'], [44100, '44 100 Гц'], [32000, '32 000 Гц'], [22050, '22 050 Гц']] },
      { type: 'select', key: 'channels', label: 'Каналы', num: true, options: [[0, 'Как у исходника'], [2, 'Стерео'], [1, 'Моно']] },
      { type: 'switch', key: 'normalize', label: 'Выровнять громкость' },
      { type: 'trim' },
    ],
  },
  'convert-image': {
    group: 'convert', title: 'Конвертер изображений', nav: 'Изображения', icon: 'image', tool: 'image', accept: ['image'],
    formats: 'В JPG, PNG, WEBP, AVIF, JXL, GIF, BMP, TIFF, ICO, TGA · из HEIC и PSD тоже',
    defaults: { mode: 'convert', format: 'jpg', quality: 90, max_side: 0, scale: 0, keep_metadata: true, background: '#ffffff' },
    fields: [
      { type: 'chips', key: 'format', label: 'Конвертировать в', options: ['jpg', 'png', 'webp', 'avif', 'jxl', 'gif', 'bmp', 'tiff', 'ico', 'tga'].map((f) => [f, f.toUpperCase()]) },
      { type: 'slider', key: 'quality', label: 'Качество', hint: QUALITY_HINT, show: (o) => ['jpg', 'webp', 'avif', 'jxl', 'png'].includes(o.format) },
      { type: 'sep' },
      { type: 'select', key: 'max_side', label: 'Максимальная сторона', num: true, options: [[0, 'Как у исходника'], [3840, '3840 px'], [2560, '2560 px'], [1920, '1920 px'], [1280, '1280 px'], [800, '800 px'], [512, '512 px'], [256, '256 px']] },
      { type: 'switch', key: 'keep_metadata', label: 'Сохранять EXIF и цветовой профиль', show: (o) => o.format === 'jpg' },
      { type: 'color', key: 'background', label: 'Фон вместо прозрачности', show: (o) => ['jpg', 'bmp'].includes(o.format) },
    ],
  },
  'ocr-image': {
    group: 'ocr', title: 'Изображение в текст', nav: 'Изображение в текст', icon: 'text', tool: 'ocr', accept: ['image'],
    formats: 'JPG, PNG, WEBP, HEIC, BMP, TIFF', preview: true,
    defaults: { lang: 'auto', output: 'txt', preprocess: false, include_images: false, pdf_mode: 'ocr' },
    fields: [
      { type: 'lang' },
      { type: 'seg', key: 'output', label: 'Сохранить как', options: [['txt', 'Текст (TXT)'], ['docx', 'Word (DOCX)']] },
      { type: 'switch', key: 'preprocess', label: 'Улучшить контраст перед распознаванием' },
    ],
  },
  'ocr-scan': {
    group: 'ocr', title: 'Сканировать в текст', nav: 'Скан в текст', icon: 'scan', tool: 'ocr', accept: ['image', 'pdf'],
    formats: 'Сканы и фото документов, PDF-сканы', preview: true,
    defaults: { lang: 'auto', output: 'docx', preprocess: true, include_images: false, pdf_mode: 'ocr' },
    fields: [
      { type: 'lang' },
      { type: 'seg', key: 'output', label: 'Сохранить как', options: [['docx', 'Word (DOCX)'], ['txt', 'Текст (TXT)']] },
      { type: 'switch', key: 'preprocess', label: 'Улучшить скан (контраст, резкость, увеличение)' },
      { type: 'switch', key: 'include_images', label: 'Вставить изображение страницы в Word', show: (o) => o.output === 'docx' },
    ],
  },
  'pdf-word': {
    group: 'ocr', title: 'PDF в Word', nav: 'PDF в Word', icon: 'word', tool: 'ocr', accept: ['pdf'], formats: 'PDF → DOCX',
    defaults: { lang: 'auto', output: 'docx', preprocess: false, include_images: false, pdf_mode: 'auto' },
    fields: [
      { type: 'select', key: 'pdf_mode', label: 'Способ', options: [['auto', 'Авто - текстовый слой, иначе распознавание'], ['ocr', 'Распознавать каждую страницу'], ['text', 'Только текстовый слой']] },
      { type: 'lang' },
      { type: 'switch', key: 'include_images', label: 'Вставить изображение страницы' },
    ],
  },
};
const NAV = [
  { items: ['home'] },
  { label: 'Сжатие', items: ['compress-video', 'compress-image', 'compress-audio', 'compress-pdf'] },
  { label: 'Конвертация', items: ['convert-video', 'convert-audio', 'convert-image'] },
  { label: 'Распознавание текста', items: ['ocr-image', 'ocr-scan', 'pdf-word'] },
];
const THEMES = [
  { id: 'graphite', name: 'Графит', a: '#000', b: '#161617', c: '#2997ff', d: '#f5f5f7' },
  { id: 'midnight', name: 'Полночь', a: '#05070d', b: '#0e1320', c: '#6ea8ff', d: '#eef2fb' },
  { id: 'emerald', name: 'Изумруд', a: '#030806', b: '#0c1611', c: '#3ddc84', d: '#ecf6f0' },
  { id: 'amber', name: 'Янтарь', a: '#0a0806', b: '#17130f', c: '#ffb340', d: '#f7f1ea' },
  { id: 'light', name: 'Светлая', a: '#ffffff', b: '#f5f5f7', c: '#0071e3', d: '#1d1d1f' },
];

// ── State ────────────────────────────────────────────────────────────
const state = {
  current: 'home',
  settings: null,
  sys: null,
  tools: {}, // id → { opts, items: Map, recoAccepted }
  jobs: new Map(), // jobId → { toolId, itemId }
  seq: 0,
};
for (const id of Object.keys(TOOLS)) state.tools[id] = { opts: { ...TOOLS[id].defaults }, items: new Map(), recoOpen: false };

let saveTimer = null;
function saveSettingsSoon() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    const s = state.settings;
    s.tool_options = { _v2: true, _v3: true };
    for (const [id, t] of Object.entries(state.tools)) s.tool_options[id] = t.opts;
    invoke('save_settings', { s }).catch(() => {});
  }, 400);
}

// ── Toasts ───────────────────────────────────────────────────────────
function toast(text, kind = 'info', ms = 4200) {
  const el = document.createElement('div');
  el.className = 'toast';
  el.dataset.kind = kind;
  el.innerHTML = `${icon(kind === 'error' ? 'alert' : kind === 'ok' ? 'check' : 'info')}<span>${esc(text)}</span>`;
  $('#toasts').appendChild(el);
  setTimeout(() => { el.classList.add('out'); setTimeout(() => el.remove(), 220); }, ms);
}

// ── Navigation ───────────────────────────────────────────────────────
function renderNav() {
  const nav = $('#nav');
  nav.innerHTML = NAV.map((g) => `
    <div class="nav-group">
      ${g.label ? `<div class="nav-label">${g.label}</div>` : ''}
      ${g.items.map((id) => `
        <button class="nav-item ui-press ui-press--surface ui-press--on-dark ui-focus" type="button" data-tool="${id}" ${state.current === id ? 'aria-current="page"' : ''}>
          ${icon(TOOLS[id].icon, 20)}<span>${TOOLS[id].nav}</span><span class="badge num" data-badge="${id}"></span>
        </button>`).join('')}
    </div>`).join('');
  nav.onclick = (e) => {
    const b = e.target.closest('[data-tool]');
    if (b) go(b.dataset.tool);
  };
  updateBadges();
}
function updateBadges() {
  for (const id of Object.keys(TOOLS)) {
    const el = $(`[data-badge="${id}"]`);
    if (!el) continue;
    const items = [...state.tools[id].items.values()];
    const active = items.filter((i) => i.state === 'running' || i.state === 'queued').length;
    el.textContent = active ? String(active) : items.length ? String(items.length) : '';
  }
}
function go(id) {
  if (!TOOLS[id]) id = 'home';
  state.current = id;
  $$('#nav [data-tool]').forEach((b) => (b.dataset.tool === id ? b.setAttribute('aria-current', 'page') : b.removeAttribute('aria-current')));
  renderView();
  $('#main').scrollTop = 0;
}

// ── View ─────────────────────────────────────────────────────────────
function acceptExts(def) {
  return def.accept.flatMap((k) => EXT[k] || []);
}

function renderView() {
  const id = state.current;
  const def = TOOLS[id];
  const t = state.tools[id];
  const main = $('#main');
  main.innerHTML = `
    <section class="view" aria-labelledby="viewTitle">
      <header class="view-head">
        <h1 id="viewTitle">${def.title}</h1>
        <span class="formats">${esc(def.formats)}</span>
      </header>
      <div class="workspace">
        <div class="col-main">
          <div class="drop" id="drop">
            <div class="drop-icon">${icon('upload')}</div>
            <div class="drop-text">
              <h2>Перетащите файлы или папки сюда</h2>
              <p>Без ограничений по размеру, длине и количеству. Исходные файлы не изменяются</p>
            </div>
            <div class="drop-hint">Отпустите, чтобы добавить</div>
            <div class="drop-actions">
              <button class="btn btn-primary ui-press ui-focus" type="button" id="addFiles">${icon('plus')}Выбрать файлы</button>
              <button class="btn btn-secondary ui-press ui-press--on-dark ui-focus" type="button" id="addFolder">${icon('folderPlus')}Папку</button>
            </div>
          </div>
          <div class="queue-bar" id="queueBar" hidden>
            <span class="count num" id="qCount"></span>
            <span class="muted num" id="qTotal"></span>
            <span class="spacer"></span>
            <button class="btn btn-sm btn-accent ui-press ui-focus" type="button" id="saveAll" hidden>${icon('download', 16)}<span id="saveAllLabel">Загрузить всё</span></button>
            <button class="btn btn-sm btn-quiet ui-press ui-press--on-dark ui-focus" type="button" id="clearDone">Убрать сохранённые</button>
            <button class="btn btn-sm btn-quiet ui-press ui-press--on-dark ui-focus" type="button" id="clearAll">${icon('trash', 16)}Очистить</button>
            <button class="btn btn-sm btn-danger ui-press ui-press--on-dark ui-focus" type="button" id="cancelAll" hidden>${icon('stop', 16)}Отменить всё</button>
          </div>
          <div class="queue" id="queue"></div>
        </div>
        <aside class="side">
          <div class="panel">
            <h2 class="panel-title">${icon('sliders')}Дополнительные настройки</h2>
            <div class="fields" id="fields"></div>
          </div>
          <div class="panel">
            <div class="fields">
              <div class="field" id="outBox"></div>
              <div class="start-area">
                <button class="btn btn-lg btn-primary btn-block ui-press ui-focus" type="button" id="start">${icon('play')}<span id="startLabel">Начать</span></button>
                <div class="summary num" id="summary"></div>
              </div>
            </div>
          </div>
        </aside>
      </div>
    </section>`;

  $('#addFiles').onclick = () => pickFiles(false);
  $('#addFolder').onclick = () => pickFiles(true);
  $('#start').onclick = start;
  $('#clearDone').onclick = () => clearItems((i) => i.state === 'done' && !isUnsaved(i));
  $('#clearAll').onclick = () => clearItems((i) => i.state !== 'running' && i.state !== 'queued', true);
  $('#cancelAll').onclick = cancelAllInTool;
  $('#saveAll').onclick = saveAll;
  renderFields();
  renderOutDir();
  renderQueue();
}

// ── Fields ───────────────────────────────────────────────────────────
function renderFields() {
  const id = state.current;
  const def = TOOLS[id];
  const t = state.tools[id];
  const o = t.opts;
  const box = $('#fields');
  if (!box) return;
  const html = [];
  for (const f of def.fields) {
    if (f.show && !f.show(o)) continue;
    html.push(fieldHtml(f, o, id));
  }
  box.innerHTML = html.join('');
  // segmented controls
  $$('.seg', box).forEach((el) => BetterUI.segmented(el, {
    onChange: (i, b) => { setOpt(b.dataset.key, b.dataset.val); },
  }));
  bindFields(box);
  updateSliderFill(box);
  updateEstimate();
  updateSummary();
}

function fieldHtml(f, o, toolId) {
  switch (f.type) {
    case 'sep': return '<div class="group-sep" role="presentation"></div>';
    case 'note': return `<p class="hint muted" style="margin:0;font:var(--t-footnote)">${esc(f.text)}</p>`;
    case 'slider': {
      const v = o[f.key] ?? 85;
      return `<div class="field">
        <span class="label">${esc(f.label)}: <span class="value num" data-val="${f.key}">${Math.round(v)}%</span></span>
        ${f.hint ? `<p class="hint">${esc(f.hint)}</p>` : ''}
        <div class="slider">
          <div class="edges"><span>Лучшая компрессия</span><span>Лучшее качество</span></div>
          <input class="range ui-focus" type="range" min="0" max="100" step="1" value="${v}" data-key="${f.key}" data-num="1" aria-label="${esc(f.label)}">
          <div class="ticks" aria-hidden="true"></div>
          <div class="scale num" aria-hidden="true">${[0, 20, 40, 60, 80, 100].map((n) => `<span>${n}%</span>`).join('')}</div>
        </div>
      </div>`;
    }
    case 'unit': {
      const v = o[f.key];
      return `<div class="field">
        <label for="f-${f.key}">${esc(f.label)}${f.info ? ` <span class="info-dot" title="${esc(f.info)}">${icon('info', 16)}</span>` : ''}</label>
        <div class="unit-input">
          <input class="input ui-field num" id="f-${f.key}" type="number" inputmode="decimal" min="${f.min}" max="${f.max}" step="${f.step}" placeholder="${esc(f.placeholder)}" value="${v ?? ''}" data-key="${f.key}" data-num="1" data-nullable="1">
          <span class="unit">${esc(f.unit)}</span>
        </div>
      </div>`;
    }
    case 'select': {
      const v = o[f.key];
      return `<div class="field">
        <label for="f-${f.key}">${esc(f.label)}</label>
        <select class="select ui-field" id="f-${f.key}" data-key="${f.key}" ${f.num ? 'data-num="1"' : ''}>
          ${f.options.map(([val, lab]) => `<option value="${val}" ${String(val) === String(v) ? 'selected' : ''}>${esc(lab)}</option>`).join('')}
        </select>
      </div>`;
    }
    case 'chips': {
      const v = o[f.key];
      return `<div class="field">
        <span class="label">${esc(f.label)}</span>
        <div class="chips" role="group" aria-label="${esc(f.label)}">
          ${f.options.map(([val, lab]) => `<button type="button" class="chip ui-press ui-press--on-dark ui-focus" data-chip="${f.key}" data-val="${val}" aria-pressed="${val === v}">${esc(lab)}</button>`).join('')}
        </div>
      </div>`;
    }
    case 'seg': {
      const v = o[f.key];
      return `<div class="field">
        <span class="label">${esc(f.label)}</span>
        <div class="seg" role="group" aria-label="${esc(f.label)}">
          ${f.options.map(([val, lab]) => `<button type="button" class="ui-focus" data-key="${f.key}" data-val="${val}" aria-pressed="${val === v}">${esc(lab)}</button>`).join('')}
        </div>
        ${f.hints?.[v] ? `<p class="hint">${esc(f.hints[v])}</p>` : ''}
      </div>`;
    }
    case 'switch': {
      const v = !!o[f.key];
      return `<div class="field-row">
        <span class="label" id="l-${f.key}">${esc(f.label)}</span>
        <button type="button" class="ui-switch ui-focus" role="switch" aria-checked="${v}" aria-labelledby="l-${f.key}" data-switch="${f.key}"></button>
      </div>`;
    }
    case 'color': {
      return `<div class="field-row">
        <label class="label" for="f-${f.key}">${esc(f.label)}</label>
        <input id="f-${f.key}" type="color" value="${esc(o[f.key] || '#ffffff')}" data-key="${f.key}" style="width:56px;height:32px;border:0;background:none;cursor:pointer">
      </div>`;
    }
    case 'trim': {
      return `<div class="field">
        <span class="label">Обрезка</span>
        <div class="time-pair">
          <input class="input ui-field" type="text" placeholder="Начало 0:00:00" value="${o.trim_start != null ? fmtTime(o.trim_start) : ''}" data-time="trim_start" aria-label="Начало, чч:мм:сс">
          <input class="input ui-field" type="text" placeholder="Конец 0:00:00" value="${o.trim_end != null ? fmtTime(o.trim_end) : ''}" data-time="trim_end" aria-label="Конец, чч:мм:сс">
        </div>
        <p class="hint">Пусто - весь файл</p>
      </div>`;
    }
    case 'lang': {
      const langs = state.sys?.ocr_languages || [];
      const v = o.lang || 'auto';
      return `<div class="field">
        <label for="f-lang">Язык текста</label>
        <select class="select ui-field" id="f-lang" data-key="lang">
          <option value="auto" ${v === 'auto' ? 'selected' : ''}>Авто (русский + английский)</option>
          ${langs.map((l) => `<option value="${esc(l.tag)}" ${l.tag === v ? 'selected' : ''}>${esc(l.name)}</option>`).join('')}
        </select>
        ${langs.length ? '' : '<p class="hint">Языки распознавания Windows не найдены: добавьте язык в Параметры → Время и язык → Язык</p>'}
      </div>`;
    }
    case 'kinds': {
      return `<div class="field"><span class="label">В очереди</span><div class="kinds num" id="kinds"></div></div>`;
    }
    case 'estimate': return '<div class="estimate" id="estimate" aria-live="polite"></div>';
    case 'reco': return recoHtml(o);
    default: return '';
  }
}

function codecOptions() {
  const e = state.sys?.caps?.encoders || {};
  const gpu = (ok) => (ok ? 'видеокарта' : 'процессор');
  return [
    ['auto', 'Авто'],
    ['av1', `AV1 · ${gpu(e.av1_nvenc || e.av1_qsv || e.av1_amf || e.av1_mf)}`],
    ['hevc', `HEVC · ${gpu(e.hevc_nvenc || e.hevc_qsv || e.hevc_amf || e.hevc_mf)}`],
    ['h264', `H.264 · ${gpu(e.h264_nvenc || e.h264_qsv || e.h264_amf || e.h264_mf)}`],
  ];
}
const CODEC_NAME = { av1: 'AV1', hevc: 'HEVC (H.265)', h264: 'H.264', auto: 'Авто' };

function recoHtml(o) {
  const caps = state.sys?.caps;
  if (!caps) return `<div class="reco"><div class="reco-head">${icon('chip')}Проверяю видеокарту…</div></div>`;
  const rc = caps.recommended_codec;
  const t = state.tools[state.current];
  const accepted = o.codec === 'auto' || o.codec === rc;
  if (!t.recoOpen) {
    return `<div class="reco ${accepted ? 'accepted' : ''}">
      <div class="reco-head">${icon('chip')}${accepted ? `Кодек: ${CODEC_NAME[rc]} - рекомендуемый` : `Кодек: ${CODEC_NAME[o.codec]}`}</div>
      <p>${esc(accepted ? caps.recommended_reason : `Рекомендуется ${CODEC_NAME[rc]}: ${caps.recommended_reason}`)}</p>
      <div class="reco-actions">
        ${accepted ? '' : `<button type="button" class="btn btn-sm btn-primary ui-press ui-focus" data-reco="accept">Принять ${CODEC_NAME[rc]}</button>`}
        <button type="button" class="btn btn-sm btn-secondary ui-press ui-press--on-dark ui-focus" data-reco="other">Выбрать другой</button>
      </div>
    </div>`;
  }
  return `<div class="reco">
    <div class="reco-head">${icon('chip')}Видеокодек</div>
    <div class="seg" role="group" aria-label="Видеокодек">
      ${codecOptions().map(([v, l]) => `<button type="button" class="ui-focus" data-key="codec" data-val="${v}" aria-pressed="${o.codec === v}">${esc(l.split(' · ')[0])}</button>`).join('')}
    </div>
    <p>${esc(codecOptions().slice(1).map(([, l]) => l).join(' · '))}</p>
    <div class="reco-actions">
      <button type="button" class="btn btn-sm btn-primary ui-press ui-focus" data-reco="accept">Вернуть рекомендуемый</button>
      <button type="button" class="btn btn-sm btn-quiet ui-press ui-press--on-dark ui-focus" data-reco="close">Готово</button>
    </div>
  </div>`;
}

function setOpt(key, raw, { rerender = true } = {}) {
  const t = state.tools[state.current];
  t.opts[key] = raw;
  saveSettingsSoon();
  if (rerender) renderFields();
}

function bindFields(box) {
  box.oninput = (e) => {
    const el = e.target;
    if (el.matches('input[type=range]')) {
      const v = Number(el.value);
      state.tools[state.current].opts[el.dataset.key] = v;
      const lbl = $(`[data-val="${el.dataset.key}"]`, box);
      if (lbl) lbl.textContent = `${v}%`;
      updateSliderFill(box);
      updateEstimate();
      saveSettingsSoon();
    } else if (el.matches('input[type=number][data-key]')) {
      // Placeholder behaviour like the site: empty = automatic.
      const s = el.value.trim();
      let v = s === '' ? null : Number(s);
      if (v != null && (Number.isNaN(v) || v <= 0)) v = null;
      state.tools[state.current].opts[el.dataset.key] = v;
      updateEstimate();
      saveSettingsSoon();
    } else if (el.matches('input[type=color]')) {
      state.tools[state.current].opts[el.dataset.key] = el.value;
      saveSettingsSoon();
    }
  };
  box.onchange = (e) => {
    const el = e.target;
    if (el.matches('select[data-key]')) {
      const v = el.dataset.num ? Number(el.value) : el.value;
      setOpt(el.dataset.key, v);
    } else if (el.matches('input[type=number][data-key]')) {
      const f = TOOLS[state.current].fields.find((x) => x.key === el.dataset.key);
      if (f && el.value !== '') {
        let v = Number(el.value);
        if (!Number.isNaN(v)) {
          v = Math.min(Math.max(v, f.min), f.max);
          el.value = String(v);
          state.tools[state.current].opts[el.dataset.key] = v;
          updateEstimate();
          saveSettingsSoon();
        }
      }
    } else if (el.matches('[data-time]')) {
      const v = parseTime(el.value);
      if (el.value.trim() && v == null) {
        BetterUI.shake(el);
        toast('Время указывается как чч:мм:сс, мм:сс или в секундах', 'error');
        return;
      }
      state.tools[state.current].opts[el.dataset.time] = v;
      saveSettingsSoon();
    }
  };
  box.onclick = (e) => {
    const chip = e.target.closest('[data-chip]');
    if (chip) { setOpt(chip.dataset.chip, chip.dataset.val); return; }
    const sw = e.target.closest('[data-switch]');
    if (sw) {
      const v = sw.getAttribute('aria-checked') !== 'true';
      sw.setAttribute('aria-checked', String(v));
      const k = sw.dataset.switch;
      state.tools[state.current].opts[k] = v;
      saveSettingsSoon();
      const def = TOOLS[state.current];
      if (def.fields.some((f) => f.show)) setTimeout(renderFields, 250);
      return;
    }
    const r = e.target.closest('[data-reco]');
    if (r) {
      const t = state.tools[state.current];
      if (r.dataset.reco === 'accept') { t.opts.codec = 'auto'; t.recoOpen = false; }
      if (r.dataset.reco === 'other') t.recoOpen = true;
      if (r.dataset.reco === 'close') t.recoOpen = false;
      saveSettingsSoon();
      renderFields();
    }
  };
}

function updateSliderFill(box) {
  $$('input[type=range]', box).forEach((r) => r.style.setProperty('--p', `${r.value}%`));
}

// Expected quality for a target size / bitrate: bits per pixel vs codec efficiency.
function updateEstimate() {
  const el = $('#estimate');
  if (!el) return;
  const o = state.tools[state.current].opts;
  const items = [...state.tools[state.current].items.values()].filter((i) => i.info?.video && i.info.duration > 0);
  const useSize = o.size_mode === 'size' && o.target_mb > 0;
  const useRate = o.size_mode === 'bitrate' && o.bitrate_kbps > 0;
  if (!useSize && !useRate) {
    el.innerHTML = `<span>Пусто - ${o.size_mode === 'size' ? 'размер' : 'битрейт'} подберётся автоматически</span>`;
    return;
  }
  if (!items.length) {
    el.innerHTML = '<span>Добавьте видео, чтобы увидеть ожидаемое качество</span>';
    return;
  }
  // Same model as the engine: below MIN_BPP the resolution is lowered instead of producing blocks.
  const caps = state.sys?.caps;
  const codec = o.codec === 'auto' ? caps?.recommended_codec || 'av1' : o.codec;
  const eff = { av1: 1, hevc: 0.8, h264: 0.6 }[codec] || 0.8;
  const MIN_BPP = 0.012;
  let worst = Infinity, worstItem = null, worstH = 0, bigger = 0;
  for (const it of items) {
    const v = it.info.video;
    if (!(v.width > 0 && v.height > 0)) continue;
    const fps = o.fps > 0 && (!v.fps || o.fps < v.fps) ? o.fps : v.fps || 30;
    let kbps;
    if (useSize) {
      if (o.target_mb * 1048576 >= it.size && !(o.height > 0)) { bigger++; continue; }
      const total = (o.target_mb * 1048576 * 8) / 1000 / it.info.duration * 0.975;
      const ak = it.info.audio?.length ? (total < 400 ? 48 : total < 1200 ? 96 : 128) * it.info.audio.length : 0;
      kbps = Math.max(50, total - ak);
    } else kbps = o.bitrate_kbps;
    const bppAt = (h) => (kbps * 1000) / (v.width * (h / v.height) * h * fps) * eff;
    let h = o.height > 0 && o.height < v.height ? o.height : v.height;
    if (!(o.height > 0) && bppAt(h) < MIN_BPP) {
      const lower = [1440, 1080, 720, 540, 480, 360].filter((x) => x < v.height);
      h = lower.find((x) => bppAt(x) >= MIN_BPP) ?? Math.min(360, v.height);
    }
    const bpp = bppAt(h);
    if (Number.isFinite(bpp) && bpp < worst) { worst = bpp; worstItem = it; worstH = h < v.height ? h : 0; }
  }
  if (!worstItem) {
    el.innerHTML = `<span>${bigger > 1 ? 'Файлы уже меньше' : 'Файл уже меньше'} заданного размера - будет сжат по качеству</span>`;
    return;
  }
  const levels = [[0.07, 'Отличное', 4], [0.04, 'Хорошее', 3], [0.022, 'Среднее', 2], [-Infinity, 'Заметные потери', 1]];
  const [, name, n] = levels.find(([t]) => worst >= t);
  const bars = [1, 2, 3, 4].map((k) => `<i class="${k <= n ? 'on' : ''}"></i>`).join('');
  const notes = [];
  if (worstH && !(o.height > 0)) notes.push(`Разрешение уменьшится до ${worstH}p, чтобы не было кубиков`);
  if (items.length > 1) notes.push(`Хуже всего у «${esc(worstItem.name)}»`);
  el.innerHTML = `<span>Ожидаемое качество:</span><span class="lvl"><span class="bars" aria-hidden="true">${bars}</span>${name}</span>${notes.map((t) => `<span class="who">${t}</span>`).join('')}`;
}

// ── Output: kept until «Загрузить на диск», or saved automatically ───
const OUT_MODES = [
  ['manual', 'По кнопке «Загрузить на диск»'],
  ['source', 'Сразу рядом с исходными файлами'],
  ['folder', 'Сразу в выбранную папку'],
];
function outMode() {
  const s = state.settings || {};
  if (s.out_mode === 'folder' && !s.out_dir) return 'manual';
  return OUT_MODES.some(([m]) => m === s.out_mode) ? s.out_mode : 'manual';
}
function outBoxHtml(idPrefix) {
  const m = outMode();
  const d = state.settings?.out_dir;
  return `<label class="label" for="${idPrefix}Mode">Сохранять</label>
    <select class="select ui-field" id="${idPrefix}Mode" data-outmode>
      ${OUT_MODES.map(([v, l]) => `<option value="${v}" ${v === m ? 'selected' : ''}>${l}</option>`).join('')}
    </select>
    ${m === 'manual' ? '<p class="hint">Готовые файлы ждут во временной папке и попадают на диск только по кнопке. Несохранённые удаляются при выходе</p>' : ''}
    ${m === 'folder' ? `<div class="outdir">
        <span class="path" title="${esc(d)}"><span>${esc(d)}</span></span>
        <button class="btn btn-sm btn-secondary ui-press ui-press--on-dark ui-focus" type="button" data-pickout>${icon('folder', 16)}Изменить</button>
      </div>` : ''}`;
}
function bindOutBox(box) {
  box.onchange = async (e) => {
    if (!e.target.matches('[data-outmode]')) return;
    const v = e.target.value;
    if (v === 'folder' && !state.settings.out_dir) {
      if (!(await pickOutDir())) { e.target.value = outMode(); return; }
    }
    state.settings.out_mode = v;
    saveSettingsSoon();
    renderOutDir();
    if ($('#settings').open) renderSettings();
  };
  box.onclick = (e) => { if (e.target.closest('[data-pickout]')) pickOutDir(); };
}
function renderOutDir() {
  const box = $('#outBox');
  if (!box) return;
  box.innerHTML = outBoxHtml('out');
  bindOutBox(box);
}
async function pickOutDir() {
  try {
    const dir = await T.dialog.open({ directory: true, multiple: false, title: 'Папка для готовых файлов' });
    if (!dir) return false;
    state.settings.out_dir = Array.isArray(dir) ? dir[0] : dir;
    state.settings.out_mode = 'folder';
    saveSettingsSoon();
    renderOutDir();
    if ($('#settings').open) renderSettings();
    return true;
  } catch (e) { toast(String(e), 'error'); return false; }
}

// ── Saving results ───────────────────────────────────────────────────
const isUnsaved = (it) => it.state === 'done' && it.temp && !it.saved;
const resultPath = (it) => it.saved || it.result?.path;
const baseName = (p) => String(p).split(/[\\/]/).pop();
const dirName = (p) => String(p).replace(/[\\/][^\\/]*$/, '');
function cleanName(p) {
  const n = baseName(p);
  return n.replace(/ \(\d+\)(\.[^.]+)$/, '$1');
}
function samePath(a, b) {
  return String(a).replace(/\//g, '\\').toLowerCase() === String(b).replace(/\//g, '\\').toLowerCase();
}

async function saveOne(it, btn) {
  const name = cleanName(it.result.path);
  const ext = name.includes('.') ? name.split('.').pop() : '';
  let dest;
  try {
    dest = await T.dialog.save({
      title: 'Загрузить на диск',
      defaultPath: `${dirName(it.path)}\\${name}`,
      filters: ext ? [{ name: ext.toUpperCase(), extensions: [ext] }] : [],
    });
  } catch (e) { toast(String(e), 'error'); return; }
  if (!dest) return;
  if (samePath(dest, it.path)) { toast('Исходный файл не заменяется - выберите другое имя', 'error'); return; }
  if (btn) { btn.disabled = true; btn.querySelector('span').textContent = 'Сохранение…'; }
  try {
    it.saved = await invoke('save_result', { path: it.result.path, dest });
    toast(`Сохранено: ${baseName(it.saved)}`, 'ok', 3000);
  } catch (e) {
    toast(String(e), 'error');
  }
  if (state.current === it.toolId) { updateRow(it); updateQueueBar(); }
}

async function saveAll() {
  const items = [...state.tools[state.current].items.values()].filter(isUnsaved);
  if (!items.length) return;
  let folder;
  try { folder = await T.dialog.open({ directory: true, multiple: false, title: 'Куда загрузить готовые файлы' }); } catch (e) { toast(String(e), 'error'); return; }
  if (!folder) return;
  folder = Array.isArray(folder) ? folder[0] : folder;
  const btn = $('#saveAll');
  btn.disabled = true;
  $('#saveAllLabel').textContent = 'Сохранение…';
  const res = await invoke('save_to_folder', { paths: items.map((i) => i.result.path), folder });
  let ok = 0, fail = 0;
  res.forEach((r, k) => {
    if (r.Ok !== undefined) { items[k].saved = r.Ok; ok++; } else { fail++; }
    updateRow(items[k]);
  });
  btn.disabled = false;
  updateQueueBar();
  toast(fail ? `Сохранено ${ok}, ошибок ${fail}` : `Сохранено ${ok} ${plural(ok, 'файл', 'файла', 'файлов')} в ${folder}`, fail ? 'error' : 'ok', 5000);
  if (ok && state.settings?.open_when_done) invoke('open_path', { path: folder });
}

function unsavedCount() {
  let n = 0;
  for (const t of Object.values(state.tools)) for (const i of t.items.values()) if (isUnsaved(i)) n++;
  return n;
}

// ── Adding files ─────────────────────────────────────────────────────
async function pickFiles(folder) {
  const def = TOOLS[state.current];
  try {
    const sel = await T.dialog.open(folder
      ? { directory: true, multiple: true, title: 'Выберите папки' }
      : { multiple: true, title: 'Выберите файлы', filters: [{ name: def.title, extensions: acceptExts(def) }, { name: 'Все файлы', extensions: ['*'] }] });
    if (!sel) return;
    addPaths(Array.isArray(sel) ? sel : [sel]);
  } catch (e) { toast(String(e), 'error'); }
}

async function addPaths(paths) {
  const id = state.current;
  const def = TOOLS[id];
  const t = state.tools[id];
  const entries = await invoke('scan_paths', { paths });
  const existing = new Set([...t.items.values()].map((i) => i.path));
  let added = 0, rejected = 0, dup = 0;
  for (const e of entries) {
    if (!def.accept.includes(e.kind)) { rejected++; continue; }
    if (existing.has(e.path)) { dup++; continue; }
    existing.add(e.path);
    const itemId = `i${++state.seq}`;
    t.items.set(itemId, { id: itemId, ...e, state: 'ready', progress: 0, stage: '', result: null, error: null, info: null, jobId: null });
    added++;
  }
  if (rejected) toast(`Пропущено ${rejected} ${plural(rejected, 'файл', 'файла', 'файлов')}: формат не подходит для «${def.title}»`, 'error');
  if (dup && !added) toast('Эти файлы уже в списке');
  if (added) {
    renderQueue();
    probeItems(id);
  }
  updateBadges();
}

// Lazily read duration/resolution for media files (4 at a time).
async function probeItems(toolId) {
  const t = state.tools[toolId];
  const todo = [...t.items.values()].filter((i) => !i.info && !i.probing && (i.kind === 'video' || i.kind === 'audio'));
  const run = async (it) => {
    it.probing = true;
    try { it.info = await invoke('probe', { path: it.path }); } catch { it.info = { failed: true }; }
    if (state.current === toolId) { updateRow(it); updateEstimate(); }
  };
  const queue = todo.slice();
  const workers = Array.from({ length: 4 }, async () => { while (queue.length) await run(queue.shift()); });
  await Promise.all(workers);
}

// ── Queue rendering ──────────────────────────────────────────────────
function metaHtml(it) {
  const parts = [`<span class="num">${fmtSize(it.size)}</span>`];
  const inf = it.info;
  if (it.state === 'done') {
    // After processing the line explains how the result was made.
    if (inf?.duration) parts.push(`<span class="num">${fmtTime(inf.duration)}</span>`);
    return parts.join('<span class="sep">·</span>');
  }
  if (inf && !inf.failed) {
    if (inf.video) parts.push(`<span class="num">${inf.video.width}×${inf.video.height}</span>`, `<span>${esc(inf.video.codec.toUpperCase())}</span>`);
    else if (inf.audio?.[0]) parts.push(`<span>${esc(inf.audio[0].codec.toUpperCase())}</span>`);
    if (inf.duration) parts.push(`<span class="num">${fmtTime(inf.duration)}</span>`);
  }
  return parts.join('<span class="sep">·</span>');
}

function rowInner(it) {
  let right = '';
  if (it.state === 'ready') {
    right = `<span class="state">Готов к обработке</span>
      <button class="icon-btn ui-press ui-press--on-dark ui-focus" type="button" data-act="remove" aria-label="Убрать из списка">${icon('x', 16)}</button>`;
  } else if (it.state === 'queued') {
    right = `<span class="state">В очереди</span>
      <button class="icon-btn ui-press ui-press--on-dark ui-focus" type="button" data-act="cancel" aria-label="Отменить">${icon('x', 16)}</button>`;
  } else if (it.state === 'running') {
    right = `<span class="state num" data-stage>${esc(stageText(it))}</span>
      <button class="icon-btn ui-press ui-press--on-dark ui-focus" type="button" data-act="cancel" aria-label="Отменить">${icon('stop', 16)}</button>`;
  } else if (it.state === 'done') {
    const r = it.result;
    const def = TOOLS[it.toolId || state.current];
    const isOcr = def.tool === 'ocr';
    const delta = it.size > 0 ? Math.round((1 - r.size / it.size) * 100) : 0;
    const unsaved = isUnsaved(it);
    right = `<span class="result num">${isOcr ? `${icon('check', 16)}Готово` : `${fmtSize(it.size)} <span class="arrow">→</span> ${fmtSize(r.size)} ${delta > 0 ? `<b>−${delta}%</b>` : delta < 0 ? `<span class="muted">+${-delta}%</span>` : ''}`}</span>
      ${isOcr && r.text != null ? `<button class="icon-btn ui-press ui-press--on-dark ui-focus" type="button" data-act="preview" aria-label="Показать текст" aria-expanded="${!!it.showText}">${icon('eye', 16)}</button>
        <button class="icon-btn ui-press ui-press--on-dark ui-focus" type="button" data-act="copy" aria-label="Копировать текст">${icon('copy', 16)}</button>` : ''}
      <button class="icon-btn ui-press ui-press--on-dark ui-focus" type="button" data-act="open" aria-label="Открыть файл">${icon('open', 16)}</button>
      ${unsaved
        ? `<button class="btn btn-sm btn-accent ui-press ui-focus" type="button" data-act="save">${icon('download', 16)}<span>Загрузить на диск</span></button>`
        : `<button class="icon-btn ui-press ui-press--on-dark ui-focus" type="button" data-act="reveal" aria-label="Показать в папке">${icon('folder', 16)}</button>`}
      <button class="icon-btn ui-press ui-press--on-dark ui-focus" type="button" data-act="remove" aria-label="${unsaved ? 'Удалить результат' : 'Убрать из списка'}">${icon('x', 16)}</button>`;
  } else if (it.state === 'error') {
    right = `<span class="state">${icon('alert', 16)}<span>${esc(shortError(it.error))}</span></span>
      <button class="icon-btn ui-press ui-press--on-dark ui-focus" type="button" data-act="remove" aria-label="Убрать из списка">${icon('x', 16)}</button>`;
  } else if (it.state === 'cancelled') {
    right = `<span class="state">Отменено</span>
      <button class="icon-btn ui-press ui-press--on-dark ui-focus" type="button" data-act="remove" aria-label="Убрать из списка">${icon('x', 16)}</button>`;
  }
  let note = it.state === 'done' && it.result?.note ? `<span class="sep">·</span><span class="note" title="${esc(it.result.note)}">${esc(it.result.note)}</span>` : '';
  if (it.state === 'done' && it.saved) note += `<span class="sep">·</span><span class="note saved" title="${esc(it.saved)}">${icon('check', 16)}${esc(baseName(it.saved))}</span>`;
  const preview = it.state === 'done' && it.showText && it.result?.text != null
    ? `<div class="text-preview">${esc(it.result.text || 'Текст не найден')}</div>` : '';
  return `<div class="kind">${icon(it.state === 'done' ? 'check' : it.state === 'error' ? 'alert' : KIND_ICON[it.kind] || 'file', 20)}</div>
    <div class="info"><span class="name" title="${esc(it.path)}">${esc(it.name)}</span><span class="meta" data-meta>${metaHtml(it)}${note}</span></div>
    <div class="actions">${right}</div>
    ${preview}
    <div class="progress" aria-hidden="true"><i style="width:${it.state === 'running' ? Math.round(it.progress * 100) : 0}%"></i></div>`;
}

function stageText(it) {
  const pct = `${Math.round(it.progress * 100)}%`;
  return it.stage ? `${it.stage} · ${pct}` : pct;
}
function shortError(e) {
  if (!e) return 'Ошибка';
  const lines = String(e).split('\n').map((s) => s.trim()).filter(Boolean);
  const l = lines[lines.length - 1] || 'Ошибка';
  return l.length > 160 ? l.slice(0, 157) + '…' : l;
}

function renderQueue() {
  const q = $('#queue');
  if (!q) return;
  const t = state.tools[state.current];
  const items = [...t.items.values()];
  const have = new Set($$('.row', q).map((r) => r.dataset.id));
  // Add new rows (animated), keep existing rows in place.
  for (const it of items) {
    if (!have.has(it.id)) {
      const row = document.createElement('div');
      row.className = 'row';
      row.dataset.id = it.id;
      row.dataset.state = it.state;
      row.innerHTML = rowInner(it);
      q.appendChild(row);
    }
  }
  for (const r of $$('.row', q)) if (!t.items.has(r.dataset.id)) r.remove();
  q.onclick = onRowAction;
  $('#drop').classList.toggle('compact', items.length > 0);
  updateQueueBar();
}

function updateRow(it, full = true) {
  const row = $(`.row[data-id="${it.id}"]`);
  if (!row) return;
  if (!full && it.state === 'running' && row.dataset.state === 'running') {
    const st = $('[data-stage]', row);
    if (st) st.textContent = stageText(it);
    const bar = $('.progress i', row);
    if (bar) bar.style.width = `${Math.round(it.progress * 100)}%`;
    return;
  }
  row.dataset.state = it.state;
  row.innerHTML = rowInner(it);
}

function updateQueueBar() {
  const t = state.tools[state.current];
  const items = [...t.items.values()];
  const bar = $('#queueBar');
  if (!bar) return;
  bar.hidden = items.length === 0;
  const total = items.reduce((s, i) => s + (i.size || 0), 0);
  $('#qCount').textContent = `${items.length} ${plural(items.length, 'файл', 'файла', 'файлов')}`;
  const done = items.filter((i) => i.state === 'done');
  const saved = done.reduce((s, i) => s + Math.max(0, i.size - (i.result?.size || 0)), 0);
  $('#qTotal').textContent = `${fmtSize(total)}${done.length && TOOLS[state.current].tool !== 'ocr' && saved > 0 ? ` · сэкономлено ${fmtSize(saved)}` : ''}`;
  const active = items.some((i) => i.state === 'running' || i.state === 'queued');
  $('#cancelAll').hidden = !active;
  $('#clearDone').hidden = !done.some((i) => !isUnsaved(i));
  const unsaved = items.filter(isUnsaved).length;
  $('#saveAll').hidden = unsaved < 2;
  $('#saveAllLabel').textContent = `Загрузить всё (${unsaved})`;
  updateSummary();
  if (state.current === 'home') updateKinds();
}

function updateKinds() {
  const el = $('#kinds');
  if (!el) return;
  const items = [...state.tools.home.items.values()];
  const c = (k) => items.filter((i) => i.kind === k).length;
  el.innerHTML = [['video', 'видео'], ['image', 'фото'], ['audio', 'аудио'], ['pdf', 'PDF']]
    .map(([k, n]) => `<div class="k"><b>${c(k)}</b><span>${n}</span></div>`).join('');
}

function updateSummary() {
  const el = $('#summary');
  if (!el) return;
  const items = [...state.tools[state.current].items.values()];
  const ready = items.filter((i) => ['ready', 'error', 'cancelled'].includes(i.state)).length;
  const active = items.filter((i) => i.state === 'running' || i.state === 'queued').length;
  const done = items.filter((i) => i.state === 'done').length;
  const again = !ready && !active && done > 0;
  const startBtn = $('#start');
  if (active) el.textContent = `В работе: ${active} из ${items.length}`;
  else if (ready) el.textContent = `К обработке: ${ready} ${plural(ready, 'файл', 'файла', 'файлов')}`;
  else if (again) el.textContent = 'Всё готово. Поменяйте настройки и обработайте заново';
  else el.textContent = 'Добавьте файлы';
  const lbl = $('#startLabel');
  if (lbl) lbl.textContent = again ? 'Обработать заново' : 'Начать';
  const off = !ready && !again;
  startBtn.disabled = off;
  startBtn.setAttribute('aria-disabled', String(off));
}

async function onRowAction(e) {
  const b = e.target.closest('[data-act]');
  if (!b) return;
  const row = b.closest('.row');
  const t = state.tools[state.current];
  const it = t.items.get(row.dataset.id);
  if (!it) return;
  const act = b.dataset.act;
  if (act === 'remove') {
    if (isUnsaved(it)) invoke('discard_result', { path: it.result.path });
    row.classList.add('leaving');
    setTimeout(() => { t.items.delete(it.id); renderQueue(); updateBadges(); updateEstimate(); }, 200);
  } else if (act === 'cancel') {
    if (it.jobId) invoke('cancel_job', { id: it.jobId });
  } else if (act === 'save') {
    saveOne(it, b);
  } else if (act === 'open') {
    invoke('open_path', { path: resultPath(it) });
  } else if (act === 'reveal') {
    invoke('reveal', { path: resultPath(it) });
  } else if (act === 'preview') {
    it.showText = !it.showText;
    updateRow(it);
  } else if (act === 'copy') {
    try { await navigator.clipboard.writeText(it.result.text || ''); toast('Текст скопирован', 'ok', 2000); } catch { toast('Не удалось скопировать', 'error'); }
  }
}

async function clearItems(pred, confirmUnsaved = false) {
  const t = state.tools[state.current];
  const unsaved = [...t.items.values()].filter((v) => pred(v) && isUnsaved(v));
  if (confirmUnsaved && unsaved.length) {
    const ok = await T.dialog.ask(`${unsaved.length} ${plural(unsaved.length, 'готовый файл не сохранён', 'готовых файла не сохранены', 'готовых файлов не сохранены')} на диск и будут удалены`, { title: 'Очистить список', kind: 'warning', okLabel: 'Удалить', cancelLabel: 'Отмена' });
    if (!ok) return;
  }
  for (const v of unsaved) invoke('discard_result', { path: v.result.path });
  for (const [k, v] of t.items) if (pred(v)) t.items.delete(k);
  renderQueue();
  updateBadges();
  updateEstimate();
}

function cancelAllInTool() {
  const t = state.tools[state.current];
  for (const it of t.items.values()) if ((it.state === 'running' || it.state === 'queued') && it.jobId) invoke('cancel_job', { id: it.jobId });
}

// ── Start / jobs ─────────────────────────────────────────────────────
// Empty fields mean "automatic": leave them out so the engine uses its defaults.
const clean = (o) => Object.fromEntries(Object.entries(o).filter(([, v]) => v !== null && v !== undefined && v !== ''));

// UI-only keys turn into what the engine understands: the level becomes a quality value and
// only the field of the chosen «Как сжимать» mode is sent.
function engineOptions(src, level) {
  const o = { ...src };
  const lv = level || o.level;
  if (lv) o.quality = LEVEL_Q[lv] ?? 85;
  if ('size_mode' in o) {
    if (o.size_mode !== 'size') delete o.target_mb;
    if (o.size_mode !== 'bitrate') delete o.bitrate_kbps;
  }
  delete o.level;
  delete o.size_mode;
  return clean(o);
}

function jobOptions(id) {
  const def = TOOLS[id];
  const o = state.tools[id].opts;
  if (def.tool !== 'auto') return engineOptions(o);
  // Batch: each kind uses its own compress settings with the shared compression level;
  // video and audio are compressed by quality (one size target makes no sense for a mixed batch).
  const lv = o.level || 'balanced';
  const pick = (tid, extra = {}) => engineOptions({ ...state.tools[tid].opts, mode: 'compress', ...extra }, lv);
  return {
    video: pick('compress-video', { size_mode: 'quality' }),
    audio: pick('compress-audio', { size_mode: 'quality' }),
    image: pick('compress-image'),
    pdf: pick('compress-pdf'),
  };
}

function start() {
  const id = state.current;
  const def = TOOLS[id];
  const t = state.tools[id];
  let todo = [...t.items.values()].filter((i) => ['ready', 'error', 'cancelled'].includes(i.state));
  if (!todo.length) {
    // Everything is done: run the finished files again with the current settings.
    todo = [...t.items.values()].filter((i) => i.state === 'done');
    for (const it of todo) if (isUnsaved(it)) invoke('discard_result', { path: it.result.path });
  }
  if (!todo.length) { toast('Добавьте файлы для обработки'); return; }
  const opts = jobOptions(id);
  const mode = outMode();
  const temp = mode === 'manual';
  const outDir = mode === 'folder' ? state.settings.out_dir : null;
  const specs = [];
  for (const it of todo) {
    const jobId = `j${++state.seq}`;
    it.jobId = jobId;
    it.state = 'queued';
    it.progress = 0;
    it.stage = '';
    it.error = null;
    it.toolId = id;
    it.temp = temp;
    it.saved = null;
    state.jobs.set(jobId, { toolId: id, itemId: it.id });
    specs.push({ id: jobId, tool: def.tool, path: it.path, options: opts, out_dir: outDir, temp });
    updateRow(it);
  }
  invoke('enqueue', { jobs: specs });
  updateQueueBar();
  updateBadges();
}

function onJobEvent(ev) {
  const ref = state.jobs.get(ev.id);
  if (!ref) return;
  const t = state.tools[ref.toolId];
  const it = t.items.get(ref.itemId);
  if (!it || it.jobId !== ev.id) return;
  const prev = it.state;
  it.state = ev.state;
  if (ev.state === 'running') {
    it.progress = ev.progress || 0;
    if (ev.stage) it.stage = ev.stage;
  } else {
    state.jobs.delete(ev.id);
    if (ev.state === 'done') { it.result = ev.output; it.progress = 1; }
    if (ev.state === 'error') it.error = ev.error;
  }
  if (state.current === ref.toolId) {
    updateRow(it, prev !== ev.state);
    if (prev !== ev.state) updateQueueBar();
  }
  if (prev !== ev.state) {
    updateBadges();
    if (ev.state !== 'running') maybeFinished(ref.toolId, it);
  }
}

function maybeFinished(toolId, last) {
  const items = [...state.tools[toolId].items.values()];
  if (items.some((i) => i.state === 'running' || i.state === 'queued')) return;
  const done = items.filter((i) => i.state === 'done');
  const err = items.filter((i) => i.state === 'error').length;
  if (!done.length && !err) return;
  const def = TOOLS[toolId];
  let text = `${def.title}: готово ${done.length}`;
  if (err) text += `, с ошибкой ${err}`;
  if (def.tool !== 'ocr') {
    const before = done.reduce((s, i) => s + i.size, 0);
    const after = done.reduce((s, i) => s + (i.result?.size || 0), 0);
    if (before > after) text += ` · ${fmtSize(before)} → ${fmtSize(after)}`;
  }
  const waiting = done.filter(isUnsaved).length;
  if (waiting) text += ` · нажмите «Загрузить на диск»`;
  toast(text, err ? 'error' : 'ok', 6000);
  if (state.settings?.open_when_done && last?.result?.path && !isUnsaved(last)) invoke('reveal', { path: resultPath(last) });
}

// ── Drag and drop (native paths, nothing is copied) ──────────────────
function initDragDrop() {
  const wv = T.webview.getCurrentWebview();
  wv.onDragDropEvent((e) => {
    const p = e.payload;
    if ($('#settings').open) return;
    if (p.type === 'enter' || p.type === 'over') document.body.classList.add('dragging');
    else if (p.type === 'leave') document.body.classList.remove('dragging');
    else if (p.type === 'drop') {
      document.body.classList.remove('dragging');
      if (p.paths?.length) addPaths(p.paths);
    }
  });
  // Never let the webview navigate to a dropped file.
  window.addEventListener('dragover', (e) => e.preventDefault());
  window.addEventListener('drop', (e) => e.preventDefault());
}

// ── Settings dialog ──────────────────────────────────────────────────
function renderSettings() {
  const s = state.settings;
  const sys = state.sys;
  const caps = sys?.caps;
  const devs = caps?.devices || [];
  const jobDevs = devs.some((d) => d.kind !== 'mf') ? devs.filter((d) => d.kind !== 'mf') : devs;
  const kindName = { nvenc: 'NVIDIA NVENC', amf: 'AMD AMF', qsv: 'Intel Quick Sync', mf: 'Media Foundation' };
  const devTags = (d) => [
    ['AV1', d.av1], ['HEVC', d.hevc], ['H.264', d.h264], ['10 бит', d.ten_bit],
    ...(d.kind === 'nvenc' ? [['UHQ', d.uhq], ['2 блока NVENC', d.split], ['NVDEC', d.cuda]] : []),
  ];
  const gpuInfo = (d) => (caps?.gpus || []).find((g) => d.name.includes(g.name) || g.name.includes(d.name.replace(' (Media Foundation)', '')));
  $('#settingsBody').innerHTML = `
    <section>
      <h3 class="section-title">Тема</h3>
      <div class="themes" role="radiogroup" aria-label="Тема">
        ${THEMES.map((th) => `<button type="button" class="theme-card ui-press ui-press--surface ui-press--on-dark ui-focus" role="radio" aria-checked="${s.theme === th.id}" data-theme-id="${th.id}"
          style="--a:${th.a};--b:${th.b};--c:${th.c};--d:${th.d}">
          <span class="sw" aria-hidden="true"><i></i><i></i></span>
          <span>${th.name}${s.theme === th.id ? icon('check', 16) : ''}</span>
        </button>`).join('')}
      </div>
    </section>
    <section class="fields">
      <h3 class="section-title" style="margin:0">Обработка</h3>
      <div class="field" id="setOutBox">${outBoxHtml('set')}</div>
      <div class="field">
        <span class="label">Видео одновременно</span>
        <div class="seg" role="group" aria-label="Видео одновременно">
          ${[1, 2, 3, 4].map((n) => `<button type="button" class="ui-focus" data-par="${n}" aria-pressed="${s.video_parallel === n}">${n}</button>`).join('')}
        </div>
        <p class="hint">Видеокарты GeForce кодируют до 8 потоков одновременно; 2 - оптимально для скорости каждого файла</p>
      </div>
      <div class="field">
        <label for="cpuPar">Потоки процессора для фото, аудио и PDF</label>
        <select class="select ui-field" id="cpuPar">
          <option value="0" ${!s.cpu_parallel ? 'selected' : ''}>Все (${caps?.cpu_threads || '…'})</option>
          ${[16, 12, 8, 4, 2, 1].filter((n) => !caps || n < caps.cpu_threads).map((n) => `<option value="${n}" ${s.cpu_parallel === n ? 'selected' : ''}>${n}</option>`).join('')}
        </select>
      </div>
      <div class="field-row">
        <span class="label" id="l-owd">Открывать папку после завершения</span>
        <button type="button" class="ui-switch ui-focus" role="switch" aria-labelledby="l-owd" aria-checked="${!!s.open_when_done}" id="owd"></button>
      </div>
    </section>
    <section class="fields">
      <h3 class="section-title" style="margin:0">Видеокарты</h3>
      ${!caps ? '<p class="hint">Определение…</p>' : !devs.length ? '<p class="hint">Аппаратный кодировщик не найден - видео кодируется на процессоре</p>' : `
      <div class="field">
        <label for="gpuDev">Кодировать видео на</label>
        <select class="select ui-field" id="gpuDev">
          <option value="auto" ${!s.gpu_device || s.gpu_device === 'auto' ? 'selected' : ''}>${jobDevs.length > 1 ? `Всех видеокартах сразу (${jobDevs.length})` : 'Видеокарте (автоматически)'}</option>
          ${jobDevs.map((d) => `<option value="${esc(d.id)}" ${s.gpu_device === d.id ? 'selected' : ''}>${esc(d.name)} · ${kindName[d.kind] || d.kind}</option>`).join('')}
        </select>
        ${jobDevs.length > 1 ? '<p class="hint">Видео из очереди распределяются по видеокартам: каждая берёт следующий файл, когда освобождается</p>' : ''}
      </div>
      ${devs.map((d) => {
        const g = gpuInfo(d);
        return `<div class="device">
          <div class="device-head">${icon('chip', 16)}<b>${esc(d.name)}</b><span class="muted">${[d.name !== kindName[d.kind] ? kindName[d.kind] || d.kind : '', d.kind === 'mf' && !jobDevs.includes(d) ? 'запасной путь для любой видеокарты' : ''].filter(Boolean).join(' · ')}</span></div>
          ${g && (g.generation || g.driver || g.vram_mb) ? `<p class="hint num">${[g.generation, g.driver && `драйвер ${esc(g.driver)}`, g.vram_mb && `${(g.vram_mb / 1024).toFixed(1).replace('.', ',')} ГБ`].filter(Boolean).join(' · ')}</p>` : ''}
          <div class="tags">${devTags(d).map(([n, ok]) => `<span class="tag ${ok ? 'on' : ''}">${n}</span>`).join('')}</div>
        </div>`;
      }).join('')}
      <p class="hint">Рекомендуемый кодек: ${CODEC_NAME[caps.recommended_codec] || ''}. Поддерживаются NVIDIA (NVENC), AMD (AMF), Intel (Quick Sync) и любые видеокарты с аппаратным кодировщиком Windows (Media Foundation)</p>`}
    </section>
    <section>
      <h3 class="section-title">О программе</h3>
      <dl class="kv">
        <dt>Версия</dt><dd class="num">${esc(sys?.version || '')}</dd>
        <dt>Движок</dt><dd>${caps?.ffmpeg_ok ? 'ffmpeg - найден' : 'ffmpeg не найден: положите папку engine рядом с программой'}</dd>
        <dt>Распознавание текста</dt><dd>${(sys?.ocr_languages || []).map((l) => esc(l.name)).join(', ') || 'языки не установлены'}</dd>
      </dl>
    </section>`;
  const body = $('#settingsBody');
  body.onclick = async (ev) => {
    const th = ev.target.closest('[data-theme-id]');
    if (th) { applyTheme(th.dataset.themeId); s.theme = th.dataset.themeId; saveSettingsSoon(); renderSettings(); return; }
    const par = ev.target.closest('[data-par]');
    if (par) { s.video_parallel = Number(par.dataset.par); saveSettingsSoon(); return; }
    const owd = ev.target.closest('#owd');
    if (owd) { s.open_when_done = owd.getAttribute('aria-checked') !== 'true'; owd.setAttribute('aria-checked', String(s.open_when_done)); saveSettingsSoon(); }
  };
  $('#cpuPar').onchange = (ev) => { s.cpu_parallel = Number(ev.target.value); saveSettingsSoon(); };
  const gd = $('#gpuDev');
  if (gd) gd.onchange = (ev) => { s.gpu_device = ev.target.value; saveSettingsSoon(); renderGpuChip(); };
  bindOutBox($('#setOutBox'));
  $$('.seg', body).forEach((el) => BetterUI.segmented(el));
}

function applyTheme(id) {
  document.documentElement.dataset.theme = id;
  try { localStorage.setItem('oc-theme', id); } catch {}
}

function openSettings() {
  renderSettings();
  const d = $('#settings');
  BetterUI.originFrom?.(d, $('#openSettings'));
  d.showModal();
}

function renderGpuChip() {
  const chip = $('#gpuChip');
  const caps = state.sys?.caps;
  if (!caps) return;
  const devs = caps.devices || [];
  const job = devs.some((d) => d.kind !== 'mf') ? devs.filter((d) => d.kind !== 'mf') : devs;
  const sel = job.find((d) => d.id === state.settings?.gpu_device);
  const shown = sel ? [sel] : job;
  const short = (n) => n.replace(/^NVIDIA\s+/i, '').replace(/^GeForce\s+/i, '').replace(/^AMD\s+/i, '').replace(/^Intel\(R\)\s+/i, '').replace(' (Media Foundation)', '');
  if (shown.length) {
    chip.dataset.state = 'gpu';
    const codec = shown.some((d) => d.av1) ? 'AV1' : shown.some((d) => d.hevc) ? 'HEVC' : 'H.264';
    const what = shown.length > 1 ? `${shown.length} видеокарты` : short(shown[0].name);
    chip.querySelector('span').textContent = `${what} · ${codec} на GPU`;
  } else {
    chip.dataset.state = 'cpu';
    chip.querySelector('span').textContent = 'Кодирование на процессоре';
  }
  chip.title = 'Сведения о видеокарте';
}

// ── Boot ─────────────────────────────────────────────────────────────
async function boot() {
  BetterUI.init(document);
  $('#openSettings').innerHTML = `${icon('sliders', 20)}<span>Настройки</span>`;
  $('#closeSettings').innerHTML = icon('x');
  $('#openSettings').onclick = openSettings;
  $('#gpuChip').onclick = openSettings;
  $('#closeSettings').onclick = () => $('#settings').close();
  $('#settings').addEventListener('click', (e) => { if (e.target === $('#settings')) $('#settings').close(); });

  try { state.settings = await invoke('get_settings'); } catch { state.settings = { theme: 'graphite', out_dir: '', video_parallel: 2, cpu_parallel: 0, open_when_done: false, tool_options: {} }; }
  applyTheme(state.settings.theme || 'graphite');
  if (!state.settings.out_mode) state.settings.out_mode = state.settings.out_dir ? 'folder' : 'manual';
  for (const [id, o] of Object.entries(state.settings.tool_options || {})) {
    if (state.tools[id] && o && typeof o === 'object') state.tools[id].opts = { ...TOOLS[id].defaults, ...o };
  }
  // v2: the slow UHQ mode stopped being the default (it was saved as "quality" by v1).
  const cv = state.tools['compress-video'].opts;
  if (!state.settings.tool_options?._v2 && cv.speed === 'quality') cv.speed = 'balanced';
  // v3: quality sliders of video/audio/batch became a compression level, and size/bitrate a mode.
  if (!state.settings.tool_options?._v3) {
    for (const id of ['home', 'compress-video', 'compress-audio', 'convert-video']) {
      const o = state.tools[id].opts;
      if (typeof o.quality === 'number') o.level = levelOf(o.quality);
      delete o.quality;
      if ('size_mode' in TOOLS[id].defaults) o.size_mode = o.target_mb > 0 ? 'size' : o.bitrate_kbps > 0 ? 'bitrate' : 'quality';
    }
  }
  saveSettingsSoon();
  renderNav();
  go('home');
  initDragDrop();
  T.event.listen('job', (e) => onJobEvent(e.payload));

  // Unsaved results disappear with the app: warn before closing.
  try {
    const win = T.window.getCurrentWindow();
    let closing = false;
    win.onCloseRequested(async (e) => {
      const n = unsavedCount();
      if (!n || closing) return;
      e.preventDefault();
      const ok = await T.dialog.ask(`${n} ${plural(n, 'готовый файл не сохранён', 'готовых файла не сохранены', 'готовых файлов не сохранены')} на диск. При выходе ${n === 1 ? 'он будет удалён' : 'они будут удалены'}`, { title: 'Выйти без сохранения?', kind: 'warning', okLabel: 'Выйти', cancelLabel: 'Остаться' });
      if (ok) { closing = true; await win.destroy(); }
    });
  } catch (err) { uiLog(`close handler: ${err}`); }

  invoke('system_info').then((sys) => {
    state.sys = sys;
    renderGpuChip();
    if (TOOLS[state.current].fields.some((f) => f.type === 'reco' || f.type === 'lang')) renderFields();
    if (!sys.caps.ffmpeg_ok) toast('Движок ffmpeg не найден: видео и аудио недоступны. Положите папку engine рядом с программой', 'error', 10000);
  });

  uiLog(`boot ok · dialog=${!!T.dialog} webview=${!!T.webview} event=${!!T.event} theme=${state.settings.theme}`);

  document.addEventListener('keydown', (e) => {
    if (e.ctrlKey && e.key.toLowerCase() === 'o') { e.preventDefault(); pickFiles(false); }
    if (e.ctrlKey && e.key === 'Enter') { e.preventDefault(); start(); }
  });
}

boot();
