'use strict';

const $ = id => document.getElementById(id);
const el = {
  history: $('history'),
  prompt: $('prompt'), input: $('input'),
  mark: $('mark'), output: $('output'), meta: $('meta'),
  pair: $('pair'), info: $('info'), hints: $('hints'),
  picker: $('picker'), query: $('query'), matches: $('matches'),
};

// ---------------------------------------------------------------- backend

const api = window.__TAURI__ ? tauriApi() : browserApi();

function tauriApi() {
  const { invoke } = window.__TAURI__.core;
  return {
    translate: (text, from, to, { pair, asTyped }) => invoke('translate', { text, from, to, pair, asTyped }),
    copy: text => invoke('copy', { text }),
    paste: text => invoke('paste', { text }),
    hide: () => invoke('hide'),
    config: () => invoke('ui_config'),
    on: (event, fn) => window.__TAURI__.event.listen(event, fn),
  };
}

// Lets the UI run in a plain browser (serve src/) to iterate on it without rebuilding.
// It skips the app's extras: input-method conversion and the rate-limit cooldown.
function browserApi() {
  return {
    async translate(text, from, to) {
      const params = new URLSearchParams({ client: 'gtx', sl: from, tl: to, hl: 'en', dj: '1' });
      ['t', 'bd', 'rm', 'qca'].forEach(dt => params.append('dt', dt));
      const res = await fetch(`https://translate.googleapis.com/translate_a/single?${params}`, {
        method: 'POST',
        body: new URLSearchParams({ q: text }),
      });
      if (!res.ok) throw { message: `Translation failed: HTTP ${res.status}` };
      const json = await res.json();
      const s = json.sentences ?? [];
      const spell = json.spell?.spell_res;
      return {
        text: s.map(x => x.trans ?? '').join(''),
        detected: json.src || null,
        translit: s.find(x => x.translit)?.translit || null,
        src_translit: s.find(x => x.src_translit)?.src_translit || null,
        dict: (json.dict ?? []).map(d => ({ pos: d.pos ?? '', terms: d.terms ?? [], base: d.base_form || null })),
        correction: spell && spell !== text
          ? { text: spell, kind: json.spell.correction_type?.includes(6) ? 'script' : 'spelling' }
          : null,
      };
    },
    copy: text => navigator.clipboard.writeText(text),
    paste: text => navigator.clipboard.writeText(text),
    hide: async () => {},
    config: async () => ({ start_in: 'input', debounce_ms: 400, hotkey: '', hotkey_error: null }),
    on: () => {},
  };
}

// ---------------------------------------------------------------- state

let config = { start_in: 'input', debounce_ms: 400, hotkey: '', hotkey_error: null };

const HISTORY_MAX = 50;

const store = Object.assign(
  { from: 'auto', to: 'en', recent: { from: [], to: [] }, history: [] },
  readStore(),
);

function readStore() {
  try { return JSON.parse(localStorage.getItem('tradu')) ?? {}; } catch { return {}; }
}
function saveStore() {
  try { localStorage.setItem('tradu', JSON.stringify(store)); } catch {}
}

const AUTO = ['auto', 'Detect language', ''];
const byCode = new Map(LANGS.map(l => [l[0], l]));
const canon = code => ALIASES[code] ?? code;
const nameOf = code => (code === 'auto' ? 'Detect language' : byCode.get(canon(code))?.[1] ?? code);
// Google's legacy codes aren't valid BCP 47; the lang attribute picks CJK glyph variants.
const htmlLang = code => ({ iw: 'he', jw: 'jv', auto: '' })[code] ?? code;

let current = null;      // last result shown
let dictTerms = [];      // alternatives in Alt+1..9 order

// ---------------------------------------------------------------- translation

const cache = new Map();
let pending = null;
let timer = 0;
let pickedKey = null; // input + languages right after an explicit language pick
let requestSeq = 0;   // order of translation requests…
let shownSeq = 0;     // …and of the one on screen, so a slow reply never replaces a newer one
let asTyped = false;  // Ctrl+O: skip spelling/script corrections
let lastRequestAt = 0;

// Spaces at the ends and runs of spaces don't change a translation, so they
// shouldn't cost a request (every one counts against Google's rate limit).
const requestText = () => el.input.value.replace(/[ \t]+/g, ' ').trim();
const keyOf = () => `${store.from}\n${store.to}\n${asTyped}\n${requestText()}`;

/** The user's languages, most relevant first: they hint at what romanized input is. */
function languagePair() {
  const langs = [store.from, store.to, ...store.recent.to, ...store.recent.from];
  return [...new Set(langs)].filter(c => c !== 'auto').slice(0, 4);
}

/** Translates the current input right away (or reuses a cached/in-flight result). */
function translateNow() {
  clearTimeout(timer);
  if (commandMode) return Promise.resolve(null);
  const text = requestText();
  const key = keyOf();
  if (!text) {
    shownSeq = ++requestSeq;
    show(null);
    return Promise.resolve(null);
  }
  if (cache.has(key)) {
    shownSeq = ++requestSeq;
    show(cache.get(key));
    return Promise.resolve(cache.get(key));
  }
  if (pending?.key === key) return pending.promise;

  const seq = ++requestSeq;
  const langs = `${store.from}\n${store.to}\n`;
  lastRequestAt = Date.now();
  const promise = api.translate(text, store.from, store.to, { pair: languagePair(), asTyped }).then(
    result => {
      remember(key, result);
      if (keyOf() !== key) {
        // Still typing: show this in-between result (dimmed) unless a newer one is up.
        if (seq > shownSeq && keyOf().startsWith(langs)) {
          shownSeq = seq;
          show(result, { stale: true });
        }
        return result;
      }
      // Typed in the target language: flip to the other side of the user's pair
      // (but not for the text that was on screen when they picked the target).
      const flip = store.from === 'auto' && key !== pickedKey
        && canon(result.detected ?? '') === store.to && alternateTarget();
      if (flip) {
        store.to = flip;
        saveStore();
        return translateNow();
      }
      shownSeq = seq;
      show(result);
      return result;
    },
    err => {
      if (pending?.key === key) pending = null; // let the next attempt retry
      if (keyOf() === key) showError(err, key);
      throw err;
    },
  );
  pending = { key, promise };
  return promise;
}

function remember(key, result) {
  cache.set(key, result);
  if (cache.size > 300) cache.delete(cache.keys().next().value);
}

/** The translation of exactly what's on screen, waiting for it if needed. */
async function settled() {
  try { return await translateNow(); } catch { return null; }
}

// While the user keeps typing, an in-between translation goes out at the end of
// a word, at most this often. Pausing (config.debounce_ms) always translates.
const IN_BETWEEN_MS = 1500;

function scheduleTranslation() {
  clearTimeout(timer);
  if (!requestText()) return translateNow();
  document.body.classList.add('stale');
  const wordEnded = /[\s.,!?;:、。，！？]$/.test(el.input.value);
  const due = wordEnded && Date.now() - lastRequestAt >= IN_BETWEEN_MS;
  timer = setTimeout(() => translateNow().catch(() => {}), due ? 0 : config.debounce_ms);
}

let retryTimer = 0;
let countdown = 0;

/** Shows a failed translation; while rate-limited, counts down and retries. */
function showError(err, key) {
  clearTimeout(retryTimer);
  clearInterval(countdown);
  const retryIn = err?.retry_in_ms;
  if (!retryIn) return setStatus(err?.message ?? String(err), 'error');
  const retryAt = Date.now() + retryIn;
  const tick = () => {
    setStatus(`google rate limit, retrying in ${Math.max(1, Math.ceil((retryAt - Date.now()) / 1000))}s`, 'error');
  };
  tick();
  countdown = setInterval(tick, 1000);
  retryTimer = setTimeout(() => {
    clearInterval(countdown);
    setStatus('');
    if (keyOf() === key) translateNow().catch(() => {});
  }, retryIn + 50);
}

/** `stale`: the result is for an earlier version of the input. */
function show(result, { stale = false } = {}) {
  current = result;
  document.body.classList.toggle('stale', stale);
  el.output.classList.remove('error');
  el.output.value = result?.text ?? '';
  el.mark.hidden = !result?.text;
  renderMeta(result);
  renderPair();
  fit(el.output);
  if (result) {
    clearTimeout(retryTimer);
    clearInterval(countdown);
    if (statusKind === 'error' && !config.hotkey_error) setStatus('');
  }
  setHints();
}

/** Alternatives for a single word, minus the translation itself. */
function alternatives(result) {
  const main = (result?.text ?? '').trim().toLowerCase();
  const terms = (result?.dict ?? []).flatMap(d => d.terms);
  return [...new Set(terms)].filter(t => t.toLowerCase() !== main);
}

/** "noun · Unterlage": the word's part of speech and dictionary form. */
function grammar(result, text) {
  const entry = result?.dict?.[0];
  if (!entry) return '';
  const base = entry.base && entry.base.toLowerCase() !== text.toLowerCase() ? entry.base : '';
  return [entry.pos, base].filter(Boolean).join(' · ');
}

/** Lines under the live translation: what was corrected, then romanization and alternatives. */
function renderMeta(result) {
  el.meta.replaceChildren();
  dictTerms = [];
  if (!result) return;
  const line = (...parts) => {
    const div = document.createElement('div');
    div.append(...parts);
    el.meta.append(div);
    return div;
  };
  const span = (text, className, lang) => {
    const s = document.createElement('span');
    s.textContent = text;
    if (className) s.className = className;
    if (lang) s.lang = lang;
    return s;
  };

  const correction = result.correction;
  if (correction || asTyped) {
    const label = asTyped
      ? (correction ? 'as typed, not' : 'as typed')
      : (correction.kind === 'script' ? 'read as' : 'corrected to');
    const row = line(span(label));
    if (correction) row.append(span(correction.text, 'value', htmlLang(result.detected ?? '')));
    const key = span('', 'key');
    key.append(Object.assign(document.createElement('b'), { textContent: '^O' }), asTyped ? ' use corrections' : ' as typed');
    row.append(key);
  }

  // Romanizing text that was converted from romaji just echoes it back, oddly
  // ("Korehana ndesu ka"), so only show the input's reading for typed scripts.
  const parts = [];
  const inputReading = correction?.kind === 'script' && !asTyped ? null : result.src_translit;
  if (inputReading) parts.push(span(inputReading));
  if (result.translit) parts.push(Object.assign(document.createElement('i'), { textContent: result.translit }));
  dictTerms = alternatives(result).slice(0, 9);
  if (dictTerms.length) parts.push(span(dictTerms.slice(0, 6).join(' · ')));
  const info = grammar(result, requestText());
  if (info) parts.push(span(info, 'grammar'));
  if (parts.length) line(...parts);
}

// ---------------------------------------------------------------- history

let recall = 0;   // 0: the line being typed; n: the nth newest history entry
let draft = '';
let helpShown = false;

/**
 * Moves what's on screen into the history. `action` ('copied'/'pasted') marks
 * the entry whose translation is now on the clipboard.
 */
function archive(action) {
  const text = requestText();
  const result = cache.get(keyOf());
  if (!text || !result || commandMode) return;
  const same = e => e.text === text && e.from === store.from && e.to === store.to;
  store.history = store.history.filter(e => !same(e));
  if (action) store.history.forEach(e => delete e.action);
  store.history.push({ text, from: store.from, to: store.to, result, ...(action && { action }) });
  store.history = store.history.slice(-HISTORY_MAX);
  saveStore();
  el.input.value = '';
  recall = 0;
  fit(el.input, 124);
  show(null);
  renderHistory();
}

function renderHistory() {
  helpShown = false;
  const entries = store.history;
  if (!entries.length) {
    const empty = document.createElement('p');
    empty.className = 'empty';
    empty.textContent = 'Type to translate. Earlier translations show up here: ↑ brings them back, :help lists everything else.';
    el.history.replaceChildren(empty);
    renderInfo();
    return;
  }
  el.history.replaceChildren(...entries.map((entry, i) => {
    const n = entries.length - i;
    const div = document.createElement('div');
    div.className = n === recall ? 'entry recalled' : 'entry';
    const lang = htmlLang(entry.result.detected ?? entry.from);
    const num = Object.assign(document.createElement('span'), { className: 'n', textContent: `[${n}]` });
    const src = Object.assign(document.createElement('span'), { className: 'src', textContent: `› ${entry.text}`, lang });
    const dst = document.createElement('div');
    dst.className = 'dst';
    dst.lang = htmlLang(entry.to);
    const words = [entry.result.text, ...alternatives(entry.result).slice(0, 2)].join(' · ');
    dst.append(Object.assign(document.createElement('span'), { textContent: words }));
    if (entry.action) dst.append(Object.assign(document.createElement('span'), { className: 'badge', textContent: entry.action }));
    div.append(num, src, dst);
    const info = grammar(entry.result, entry.text) || entry.result.translit;
    if (info) div.append(Object.assign(document.createElement('span'), { className: 'info', textContent: info }));
    return div;
  }));
  el.history.scrollTop = el.history.scrollHeight;
  renderInfo();
}

/** ↑/↓ through earlier translations, like a shell. Returns whether it moved. */
function recallEntry(step) {
  const n = recall + step;
  if (n < 0 || n > store.history.length) return false;
  if (recall === 0) draft = el.input.value;
  recall = n;
  if (n === 0) {
    el.input.value = draft;
  } else {
    const entry = store.history.at(-n);
    store.from = entry.from;
    store.to = entry.to;
    saveStore();
    el.input.value = entry.text;
    asTyped = false;
    remember(keyOf(), entry.result);
  }
  fit(el.input, 124);
  el.input.setSelectionRange(el.input.value.length, el.input.value.length);
  renderHistory();
  translateNow().catch(() => {});
  return true;
}

function clearHistory() {
  store.history = [];
  saveStore();
  renderHistory();
}

const HELP = [
  ['Translating'],
  ['⏎', 'copy the translation and close'],
  ['^⏎', 'paste it into the app you came from'],
  ['^⇧C', 'copy it and keep going'],
  ['alt 1-9', 'copy an alternative'],
  ['↑ ↓', 'step through history'],
  ['tab', 'select part of the translation'],
  ['^S', 'swap languages'],
  ['^L ^F ^T', 'pick both languages, the source, the target'],
  ['^O', 'translate as typed, without corrections'],
  ['^P ^⇧P', 'listen to the translation, the input'],
  ['Commands'],
  [':ja', 'translate into Japanese'],
  [':de en', 'German into English (auto for detect)'],
  [':swap :clear', 'swap languages, clear history'],
];

function showHelp() {
  const div = document.createElement('div');
  div.className = 'help';
  for (const [key, text] of HELP) {
    if (!text) {
      div.append(Object.assign(document.createElement('span'), { className: 'head', textContent: key }));
      continue;
    }
    div.append(Object.assign(document.createElement('b'), { textContent: key }), Object.assign(document.createElement('span'), { textContent: text }));
  }
  el.history.replaceChildren(div);
  el.history.scrollTop = el.history.scrollHeight;
  helpShown = true;
}

// ---------------------------------------------------------------- commands

let commandMode = false;

function enterCommand() {
  commandMode = true;
  document.body.classList.add('command');
  el.input.value = '';
  previewCommand();
  renderPair();
  setHints();
}

function exitCommand() {
  commandMode = false;
  document.body.classList.remove('command');
  el.input.value = '';
  fit(el.input, 124);
  show(null);
}

const resolveLang = (word, which) => findMatches(which, word)[0]?.[0] ?? null;

/** Reads a command line into { label, run } or { error }. */
function parseCommand(line) {
  const words = line.trim().toLowerCase().split(/[\s>→,]+/).filter(Boolean);
  if (!words.length) return { label: 'a language (ja), two languages (de en), swap, clear or help' };
  const [first] = words;
  if (words.length === 1 && (first === 'swap' || first === 's')) return { label: 'swap languages', run: swap };
  if (words.length === 1 && first === 'clear') {
    return { label: `clear history (${store.history.length})`, run: clearHistory };
  }
  if (words.length === 1 && ['help', 'h', '?'].includes(first)) return { label: 'list keys and commands', run: showHelp };
  if (words.length > 2) return { error: 'give one language (target) or two (source and target)' };
  const langs = words.map((w, i) => resolveLang(w, words.length === 2 && i === 0 ? 'from' : 'to'));
  const unknown = words.find((_, i) => !langs[i]);
  if (unknown) return { error: `no language matches "${unknown}"` };
  if (langs.length === 1) return { label: `translate into ${nameOf(langs[0])}`, run: () => setLang('to', langs[0]) };
  return { label: `${nameOf(langs[0])} → ${nameOf(langs[1])}`, run: () => setPair(langs[0], langs[1]) };
}

function previewCommand() {
  const cmd = parseCommand(el.input.value);
  el.mark.hidden = false;
  el.output.value = cmd.error ?? cmd.label;
  el.output.classList.toggle('error', !!cmd.error);
  el.meta.replaceChildren();
  document.body.classList.remove('stale');
  fit(el.output);
}

function runCommand() {
  const cmd = parseCommand(el.input.value);
  if (!cmd.run) return;
  exitCommand();
  cmd.run();
  if (cmd.run !== showHelp) renderHistory();
}

// ---------------------------------------------------------------- languages

/** The DE→EN block and the input's prompt. */
function renderPair() {
  const detected = store.from === 'auto' ? current?.detected : null;
  const source = canon(detected ?? store.from);
  el.pair.textContent = `${source}→${store.to}`.toUpperCase();
  el.prompt.textContent = commandMode ? ':' : `${source}›`;
  el.input.lang = htmlLang(source);
  el.output.lang = htmlLang(store.to);
  renderInfo();
}

function setLang(which, code) {
  const other = which === 'from' ? 'to' : 'from';
  // Picking the language on the other side swaps instead of translating X → X.
  if (code === store[other]) {
    const previous = store[which] === 'auto' ? current?.detected : store[which];
    if (previous && canon(previous) !== code) store[other] = canon(previous);
  }
  store[which] = code;
  addRecent(which, code);
  pickedKey = keyOf();
  saveStore();
  renderPair();
  translateNow().catch(() => {});
}

function setPair(from, to) {
  store.from = from;
  store.to = to;
  addRecent('from', from);
  addRecent('to', to);
  pickedKey = keyOf();
  saveStore();
  renderPair();
  translateNow().catch(() => {});
}

function addRecent(which, code) {
  store.recent[which] = [code, ...store.recent[which].filter(c => c !== code)].slice(0, 9);
}

/** The most recently used language other than the current target, if any. */
function alternateTarget() {
  const system = [navigator.language, navigator.language.split('-')[0]].map(canon).find(c => byCode.has(c));
  return [...store.recent.to, ...store.recent.from, system, 'en'].find(c => c && c !== 'auto' && c !== store.to);
}

async function swap() {
  const result = await settled();
  const from = store.from === 'auto' ? result?.detected : store.from;
  if (!from) return setStatus('type something first so the language can be detected', 'error');
  store.from = store.to;
  store.to = canon(from);
  if (result?.text) el.input.value = result.text;
  saveStore();
  fit(el.input, 124);
  renderPair();
  translateNow().catch(() => {});
}

// ---------------------------------------------------------------- picker

let picking = null; // { which, chain, items, sel }

/** `chain`: after picking the source, go on to the target (the Ctrl+L flow). */
function openPicker(which, chain) {
  if (commandMode) exitCommand();
  picking = { which, chain, items: [], sel: 0 };
  el.picker.hidden = false;
  el.query.value = '';
  el.query.placeholder = which === 'from' ? 'translate from…' : 'translate to…';
  renderMatches();
  el.query.focus();
  setHints();
}

function closePicker() {
  picking = null;
  el.picker.hidden = true;
}

/** Picks `code` (or keeps the current language when null) and moves focus on. */
function choose(code) {
  const { which, chain } = picking;
  if (code) setLang(which, code);
  closePicker();
  if (chain && which === 'from') openPicker('to', true);
  else focusInput();
}

const normalize = s => s.normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();

function score([code, name, native], q) {
  const n = normalize(name);
  const nat = normalize(native);
  const c = code.toLowerCase();
  if (c === q || canon(q) === code) return 100;
  if (n.startsWith(q)) return 80;
  if (nat.startsWith(q)) return 75;
  if (n.split(/[^a-z]+/).some(w => w.startsWith(q))) return 60;
  if (c.startsWith(q)) return 50;
  if (n.includes(q) || nat.includes(q)) return 40;
  let i = 0;
  for (const ch of n) if (ch === q[i]) i++;
  return i === q.length ? 10 : 0;
}

function findMatches(which, query) {
  const pool = which === 'from' ? [AUTO, ...LANGS] : LANGS;
  const recent = store.recent[which];
  const q = normalize(query.trim());
  if (!q) {
    // Quick-pick list: recents first, topped up with popular languages.
    const head = which === 'from' ? [...recent, 'auto'] : recent;
    return [...new Set([...head, ...POPULAR])]
      .map(c => (c === 'auto' ? AUTO : byCode.get(c)))
      .filter(Boolean)
      .slice(0, 9);
  }
  const bonus = code => {
    const r = recent.indexOf(code);
    const p = POPULAR.indexOf(code);
    return (r >= 0 ? 6 - r * 0.1 : 0) + (p >= 0 ? 3 - p * 0.05 : 0);
  };
  return pool
    .map(lang => ({ lang, s: score(lang, q) }))
    .filter(m => m.s > 0)
    .sort((a, b) => b.s + bonus(b.lang[0]) - (a.s + bonus(a.lang[0])))
    .slice(0, 8)
    .map(m => m.lang);
}

function renderMatches() {
  const quick = !el.query.value.trim();
  picking.items = findMatches(picking.which, el.query.value);
  picking.sel = 0;
  el.matches.replaceChildren(
    ...picking.items.map(([code, name, native], i) => {
      const li = document.createElement('li');
      li.innerHTML = '<span class="n"></span><span class="code"></span><span></span><span class="native"></span>';
      li.children[0].textContent = quick ? i + 1 : '';
      li.children[1].textContent = code;
      li.children[2].textContent = name;
      li.children[3].textContent = native !== name ? native : '';
      li.children[3].lang = htmlLang(code);
      li.addEventListener('mousedown', e => { e.preventDefault(); choose(code); });
      return li;
    }),
  );
  if (!picking.items.length) {
    el.matches.innerHTML = '<li class="none">no language matches</li>';
  }
  highlight();
}

function highlight() {
  [...el.matches.children].forEach((li, i) => li.classList.toggle('sel', i === picking.sel));
}

function pickerKey(e) {
  if (e.isComposing) return;
  const ctrl = e.ctrlKey || e.metaKey;
  const k = e.key.toLowerCase();
  const move = d => {
    const n = picking.items.length;
    if (n) picking.sel = (picking.sel + d + n) % n;
    highlight();
  };
  if (e.key === 'ArrowDown' || (ctrl && (k === 'n' || k === 'j'))) move(1);
  else if (e.key === 'ArrowUp' || (ctrl && (k === 'p' || k === 'k'))) move(-1);
  else if (e.key === 'Enter' || (e.key === 'Tab' && !e.shiftKey)) {
    const item = picking.items[picking.sel];
    choose(item ? item[0] : null);
  } else if (e.key === 'Tab' && e.shiftKey) {
    if (picking.which === 'to') openPicker('from', true);
  } else if (e.key === 'Escape') choose(null);
  else if (/^[1-9]$/.test(e.key) && !el.query.value && !ctrl && !e.altKey) {
    const item = picking.items[+e.key - 1];
    if (!item) return;
    choose(item[0]);
  } else return;
  e.preventDefault();
  e.stopPropagation();
}

el.query.addEventListener('keydown', pickerKey);
el.query.addEventListener('input', renderMatches);
el.query.addEventListener('blur', () => {
  if (!picking) return;
  closePicker();
  setHints();
});
el.pair.addEventListener('click', () => openPicker('from', true));

// ---------------------------------------------------------------- actions

/**
 * copy: copy and close · paste: close and paste into the previous window ·
 * keep: copy and stay open for the next one. `text` defaults to the translation.
 */
async function commit(mode, text) {
  text ??= (await settled())?.text;
  if (!text) return;
  // Archive first: closing the popup blurs it, which archives without the action.
  archive(mode === 'paste' ? 'pasted' : 'copied');
  try {
    if (mode === 'keep') return await api.copy(text);
    audio.pause();
    if (mode === 'paste') await api.paste(text);
    else {
      await api.copy(text);
      await api.hide();
    }
  } catch (err) {
    setStatus(err?.message ?? String(err), 'error');
  }
}

function hide() {
  audio.pause();
  closePicker();
  if (commandMode) exitCommand();
  archive(null);
  api.hide();
}

const audio = new Audio();

function speak(text, lang) {
  if (!text?.trim() || !lang || lang === 'auto') return;
  const params = new URLSearchParams({ client: 'gtx', ie: 'UTF-8', tl: lang, q: text.slice(0, 200) });
  audio.src = `https://translate.googleapis.com/translate_tts?${params}`;
  audio.play().catch(() => setStatus("couldn't play audio", 'error'));
}

function selectedOutput() {
  const { selectionStart: a, selectionEnd: b, value } = el.output;
  return a !== b ? value.slice(a, b) : undefined;
}

// ---------------------------------------------------------------- keyboard

el.input.addEventListener('input', () => {
  fit(el.input, 124);
  recall = 0;
  if (helpShown) renderHistory();
  if (commandMode) return previewCommand();
  if (!requestText()) asTyped = false;
  scheduleTranslation();
});

const onFirstLine = () => !el.input.value.slice(0, el.input.selectionStart).includes('\n');
const onLastLine = () => !el.input.value.slice(el.input.selectionEnd).includes('\n');

el.input.addEventListener('keydown', e => {
  if (e.isComposing || e.keyCode === 229) return; // IME is still composing
  if (commandMode) {
    if (e.key === 'Enter') { e.preventDefault(); runCommand(); }
    else if (e.key === 'Escape' || (e.key === 'Backspace' && !el.input.value)) { e.preventDefault(); exitCommand(); }
    return;
  }
  if (e.key === ':' && !el.input.value) {
    e.preventDefault();
    enterCommand();
  } else if (e.key === 'Enter' && !e.shiftKey && !e.altKey) {
    e.preventDefault();
    commit(e.ctrlKey || e.metaKey ? 'paste' : 'copy');
  } else if (e.key === 'Escape') {
    e.preventDefault();
    hide();
  } else if (e.key === 'ArrowUp' && !e.shiftKey && onFirstLine()) {
    if (recallEntry(1)) e.preventDefault();
  } else if (e.key === 'ArrowDown' && !e.shiftKey && recall > 0 && onLastLine()) {
    if (recallEntry(-1)) e.preventDefault();
  } else if (e.key === 'Tab' && !e.shiftKey && el.output.value) {
    e.preventDefault();
    el.output.focus();
    el.output.select();
  } else if (e.key === 'Tab') {
    e.preventDefault();
  }
});

el.output.addEventListener('keydown', e => {
  if (e.key === 'Enter') {
    e.preventDefault();
    commit(e.ctrlKey || e.metaKey ? 'paste' : 'copy', selectedOutput());
  } else if (e.key === 'Tab' || e.key === 'Escape') {
    e.preventDefault();
    focusInput();
  }
});

el.input.addEventListener('focus', () => setHints());
el.output.addEventListener('focus', () => setHints());

document.addEventListener('keydown', e => {
  const ctrl = e.ctrlKey || e.metaKey;
  const k = e.key.length === 1 ? e.key.toLowerCase() : e.key;

  // Keep the webview's browser shortcuts (reload, find, print, caret browsing…) out of the way.
  if (['F3', 'F5', 'F7'].includes(k) || (ctrl && 'rfgpuojnhdte'.includes(k) && k.length === 1)) {
    e.preventDefault();
  }
  if (picking) return;

  if (ctrl && !e.shiftKey && k === 's') { e.preventDefault(); swap(); }
  else if (ctrl && k === 'l') { e.preventDefault(); openPicker('from', true); }
  else if (ctrl && k === 'f') openPicker('from', false);
  else if (ctrl && k === 't') openPicker('to', false);
  else if (ctrl && e.shiftKey && k === 'c') { e.preventDefault(); commit('keep', selectedOutput()); }
  else if (ctrl && k === 'o') {
    asTyped = !asTyped;
    translateNow().catch(() => {});
  } else if (ctrl && k === 'p') {
    if (e.shiftKey) speak(el.input.value, store.from === 'auto' ? current?.detected : store.from);
    else speak(el.output.value, store.to);
  } else if (e.altKey && !ctrl && /^Digit[1-9]$/.test(e.code)) {
    e.preventDefault();
    const term = dictTerms[+e.code.slice(5) - 1];
    if (term) commit('copy', term);
  }
});

document.addEventListener('contextmenu', e => {
  if (!e.target.matches('textarea, input')) e.preventDefault();
});

// Closing by clicking away (the app hides on blur) files the translation too.
window.addEventListener('blur', () => archive(null));

// ---------------------------------------------------------------- chrome

const HINTS = {
  input: [['↑', 'recall'], ['⏎', 'copy'], ['^⏎', 'paste'], ['^S', 'swap'], [':', 'command']],
  dict: [['⏎', 'copy'], ['alt 1-9', 'alternative'], ['^⏎', 'paste'], ['^S', 'swap'], [':', 'command']],
  output: [['⇧ ←→', 'select'], ['⏎', 'copy'], ['^⏎', 'paste'], ['tab', 'back']],
  picker: [['↑↓', 'move'], ['⏎', 'pick'], ['1-9', 'quick pick'], ['esc', 'keep'], ['⇧tab', 'back']],
  command: [['⏎', 'run'], ['esc', 'cancel']],
};

function setHints() {
  const which = picking ? 'picker'
    : commandMode ? 'command'
    : document.activeElement === el.output ? 'output'
    : dictTerms.length ? 'dict' : 'input';
  el.hints.replaceChildren(...HINTS[which].map(([key, label]) => {
    const span = document.createElement('span');
    span.append(Object.assign(document.createElement('b'), { textContent: key }), label);
    return span;
  }));
}

let statusText = '';
let statusKind = '';

/** A message for the status bar; empty brings back the history count. */
function setStatus(text, kind = '') {
  statusText = text;
  statusKind = kind;
  renderInfo();
}

function renderInfo() {
  let text = statusText;
  let kind = statusKind;
  if (!text && config.hotkey_error) [text, kind] = [config.hotkey_error, 'error'];
  if (!text) {
    const parts = [`history ${store.history.length}`];
    if (store.from === 'auto') parts.push('auto-detect');
    text = parts.join('   ');
  }
  el.info.textContent = text;
  el.info.title = text;
  el.info.className = kind;
}

function focusInput() {
  el.input.focus();
  setHints();
}

function fit(textarea, max = Infinity) {
  textarea.style.height = 'auto';
  textarea.style.height = `${Math.min(textarea.scrollHeight, max)}px`;
}

let touchedSinceSummon = false;
document.addEventListener('keydown', () => { touchedSinceSummon = true; }, true);
document.addEventListener('mousedown', () => { touchedSinceSummon = true; }, true);

/** `selection`: text that was selected in the app the hotkey was pressed in. */
function onSummon(selection) {
  touchedSinceSummon = false;
  asTyped = false;
  recall = 0;
  closePicker();
  if (commandMode) exitCommand();
  renderHistory();
  if (selection) return useSelection(selection);
  if (config.start_in === 'source') return openPicker('from', true);
  focusInput();
  el.input.select(); // an unfinished line stays; typing replaces it
}

function useSelection(text) {
  closePicker();
  el.input.value = text.trim();
  fit(el.input, 124);
  focusInput();
  el.input.select();
  translateNow().catch(() => {});
}

async function loadConfig() {
  config = { ...config, ...(await api.config().catch(() => ({}))) };
  renderInfo();
}

(async () => {
  await loadConfig();
  renderPair();
  renderHistory();
  show(null);
  api.on('summon', e => onSummon(e.payload));
  // The app took longer than the popup to copy its selection.
  api.on('selection', e => touchedSinceSummon || useSelection(e.payload));
  api.on('config-changed', loadConfig);
  onSummon();
})();
