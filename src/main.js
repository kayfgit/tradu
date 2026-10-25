'use strict';

const $ = id => document.getElementById(id);
const el = {
  from: $('from'), to: $('to'), swap: $('swap'), status: $('status'),
  picker: $('picker'), query: $('query'), matches: $('matches'),
  input: $('input'), srcTranslit: $('src-translit'), correction: $('correction'),
  output: $('output'), translit: $('translit'), dict: $('dict'), hints: $('hints'),
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
        dict: (json.dict ?? []).map(d => ({ pos: d.pos ?? '', terms: d.terms ?? [] })),
        correction: spell && spell !== text
          ? { text: spell, kind: json.spell.correction_type?.includes(6) ? 'script' : 'spelling' }
          : null,
      };
    },
    copy: text => navigator.clipboard.writeText(text),
    paste: text => navigator.clipboard.writeText(text),
    hide: async () => {},
    config: async () => ({ start_in: 'input', debounce_ms: 200, hotkey: '', hotkey_error: null }),
    on: () => {},
  };
}

// ---------------------------------------------------------------- state

let config = { start_in: 'input', debounce_ms: 200, hotkey: '', hotkey_error: null };

const store = Object.assign(
  { from: 'auto', to: 'en', recent: { from: [], to: [] }, text: '' },
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
const nameOf = code => byCode.get(canon(code))?.[1] ?? code;
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
      cache.set(key, result);
      if (cache.size > 300) cache.delete(cache.keys().next().value);
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
    setStatus(`Google rate limit, retrying in ${Math.max(1, Math.ceil((retryAt - Date.now()) / 1000))} s`, 'error');
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
  el.output.value = result?.text ?? '';
  el.translit.textContent = result?.translit ?? '';
  el.srcTranslit.textContent = result?.src_translit ?? '';
  renderCorrection(result);
  renderDict(result?.dict ?? []);
  renderLangs();
  fit(el.output);
  if (result) {
    clearTimeout(retryTimer);
    clearInterval(countdown);
  }
  if (el.status.classList.contains('error') && !config.hotkey_error) setStatus('');
  setHints();
}

/** "Read as ありがとう", or "Corrected to läuft", with the key that undoes it. */
function renderCorrection(result) {
  el.correction.replaceChildren();
  const correction = result?.correction;
  if (!result || (!correction && !asTyped)) return;
  const label = asTyped
    ? (correction ? 'Translating as typed, not ' : 'Translating as typed')
    : (correction.kind === 'script' ? 'Read as ' : 'Corrected to ');
  el.correction.append(label);
  if (correction) {
    const mark = document.createElement('mark');
    mark.textContent = correction.text;
    mark.lang = htmlLang(result.detected ?? '');
    el.correction.append(mark);
  }
  const undo = document.createElement('span');
  undo.className = 'undo';
  const kbd = document.createElement('kbd');
  kbd.textContent = 'Ctrl+O';
  undo.append(kbd, asTyped ? 'use corrections' : 'use what I typed');
  el.correction.append(undo);
}

function renderDict(dict) {
  dictTerms = [];
  el.dict.replaceChildren();
  for (const { pos, terms } of dict.slice(0, 4)) {
    const label = document.createElement('div');
    label.className = 'pos';
    label.textContent = pos || 'other';
    const row = document.createElement('div');
    row.className = 'terms';
    for (const term of terms.slice(0, 8)) {
      const span = document.createElement('span');
      if (dictTerms.length < 9) {
        const n = document.createElement('b');
        n.textContent = dictTerms.length + 1;
        span.append(n);
      }
      span.append(term);
      span.addEventListener('click', () => commit('copy', term));
      dictTerms.push(term);
      row.append(span);
    }
    el.dict.append(label, row);
  }
}

// ---------------------------------------------------------------- languages

function renderLangs() {
  const detected = store.from === 'auto' ? current?.detected : null;
  if (store.from !== 'auto') el.from.textContent = nameOf(store.from);
  else if (detected) el.from.innerHTML = `${escapeHtml(nameOf(detected))}<small>detected</small>`;
  else el.from.textContent = 'Detect language';
  el.to.textContent = nameOf(store.to);
  el.input.lang = htmlLang(detected ?? store.from);
  el.output.lang = htmlLang(store.to);
}

function setLang(which, code) {
  const other = which === 'from' ? 'to' : 'from';
  // Picking the language on the other side swaps instead of translating X → X.
  if (code === store[other]) {
    const previous = store[which] === 'auto' ? current?.detected : store[which];
    if (previous && canon(previous) !== code) store[other] = canon(previous);
  }
  store[which] = code;
  store.recent[which] = [code, ...store.recent[which].filter(c => c !== code)].slice(0, 9);
  pickedKey = keyOf();
  saveStore();
  renderLangs();
  translateNow().catch(() => {});
}

/** The most recently used language other than the current target, if any. */
function alternateTarget() {
  const system = [navigator.language, navigator.language.split('-')[0]].map(canon).find(c => byCode.has(c));
  return [...store.recent.to, ...store.recent.from, system, 'en'].find(c => c && c !== 'auto' && c !== store.to);
}

async function swap() {
  const result = await settled();
  const from = store.from === 'auto' ? result?.detected : store.from;
  if (!from) return setStatus('Type something first so the language can be detected', 'error');
  store.from = store.to;
  store.to = canon(from);
  if (result?.text) el.input.value = result.text;
  store.text = el.input.value;
  saveStore();
  fit(el.input, 120);
  renderLangs();
  translateNow().catch(() => {});
}

// ---------------------------------------------------------------- picker

let picking = null; // { which, chain, items, sel }

/** `chain`: after picking the source, go on to the target (the Ctrl+L flow). */
function openPicker(which, chain) {
  picking = { which, chain, items: [], sel: 0 };
  el.from.classList.toggle('active', which === 'from');
  el.to.classList.toggle('active', which === 'to');
  const chip = which === 'from' ? el.from : el.to;
  el.picker.style.left = `${Math.min(chip.offsetLeft, document.body.clientWidth - 310)}px`;
  el.picker.hidden = false;
  el.query.value = '';
  el.query.placeholder = which === 'from' ? 'Translate from…' : 'Translate to…';
  renderMatches();
  el.query.focus();
  setHints();
}

function closePicker() {
  picking = null;
  el.picker.hidden = true;
  el.from.classList.remove('active');
  el.to.classList.remove('active');
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
      li.innerHTML = `<span class="n">${quick ? i + 1 : ''}</span><span></span><span class="native"></span>`;
      li.children[1].textContent = name;
      li.children[2].textContent = native !== name ? native : '';
      li.children[2].lang = htmlLang(code);
      li.addEventListener('mousedown', e => { e.preventDefault(); choose(code); });
      return li;
    }),
  );
  if (!picking.items.length) {
    el.matches.innerHTML = '<li class="empty">No language matches</li>';
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
el.from.addEventListener('click', () => openPicker('from', false));
el.to.addEventListener('click', () => openPicker('to', false));
el.swap.addEventListener('click', swap);

// ---------------------------------------------------------------- actions

/**
 * copy: copy and close · paste: close and paste into the previous window ·
 * keep: copy and stay open. `text` defaults to the full translation.
 */
async function commit(mode, text) {
  text ??= (await settled())?.text;
  if (!text) return;
  try {
    if (mode === 'keep') {
      await api.copy(text);
      setStatus('Copied', 'done');
      setTimeout(() => el.status.textContent === 'Copied' && setStatus(''), 1200);
      return;
    }
    audio.pause();
    if (mode === 'paste') await api.paste(text);
    else {
      await api.copy(text);
      await api.hide();
    }
  } catch (err) {
    setStatus(String(err), 'error');
  }
}

function hide() {
  audio.pause();
  closePicker();
  api.hide();
}

const audio = new Audio();

function speak(text, lang) {
  if (!text?.trim() || !lang || lang === 'auto') return;
  const params = new URLSearchParams({ client: 'gtx', ie: 'UTF-8', tl: lang, q: text.slice(0, 200) });
  audio.src = `https://translate.googleapis.com/translate_tts?${params}`;
  audio.play().catch(() => setStatus("Couldn't play audio", 'error'));
}

function selectedOutput() {
  const { selectionStart: a, selectionEnd: b, value } = el.output;
  return a !== b ? value.slice(a, b) : undefined;
}

// ---------------------------------------------------------------- keyboard

el.input.addEventListener('input', () => {
  fit(el.input, 120);
  store.text = el.input.value;
  saveStore();
  if (!requestText()) asTyped = false;
  scheduleTranslation();
});

el.input.addEventListener('keydown', e => {
  if (e.isComposing || e.keyCode === 229) return; // IME is still composing
  if (e.key === 'Enter' && !e.shiftKey && !e.altKey) {
    e.preventDefault();
    commit(e.ctrlKey || e.metaKey ? 'paste' : 'copy');
  } else if (e.key === 'Escape') {
    e.preventDefault();
    hide();
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

// ---------------------------------------------------------------- chrome

const HINTS = {
  input: [['Enter', 'copy'], ['Ctrl+Enter', 'paste back'], ['Ctrl+S', 'swap'], ['Ctrl+L', 'languages'], ['Ctrl+P', 'listen'], ['Esc', 'close']],
  dict: [['Enter', 'copy'], ['Alt+1–9', 'copy alternative'], ['Ctrl+Enter', 'paste back'], ['Ctrl+S', 'swap'], ['Ctrl+L', 'languages'], ['Esc', 'close']],
  output: [['Shift+arrows', 'select'], ['Enter', 'copy selection'], ['Ctrl+Enter', 'paste selection'], ['Ctrl+Shift+C', 'copy, stay'], ['Tab', 'back']],
  picker: [['↑↓', 'move'], ['Enter', 'pick'], ['1–9', 'quick pick'], ['Esc', 'keep current'], ['Shift+Tab', 'back']],
};

function setHints() {
  const which = picking ? 'picker'
    : document.activeElement === el.output ? 'output'
    : dictTerms.length ? 'dict' : 'input';
  el.hints.replaceChildren(...HINTS[which].map(([key, label]) => {
    const span = document.createElement('span');
    const kbd = document.createElement('kbd');
    kbd.textContent = key;
    span.append(kbd, label);
    return span;
  }));
}

function setStatus(text, kind = '') {
  if (!text && config.hotkey_error) [text, kind] = [config.hotkey_error, 'error'];
  el.status.textContent = text;
  el.status.className = kind;
  el.status.title = text;
}

function focusInput() {
  el.input.focus();
  setHints();
}

function fit(textarea, max = Infinity) {
  textarea.style.height = 'auto';
  textarea.style.height = `${Math.min(textarea.scrollHeight, max)}px`;
}

function escapeHtml(s) {
  return s.replace(/[&<>"']/g, c => `&#${c.charCodeAt(0)};`);
}

let touchedSinceSummon = false;
document.addEventListener('keydown', () => { touchedSinceSummon = true; }, true);
document.addEventListener('mousedown', () => { touchedSinceSummon = true; }, true);

/** `selection`: text that was selected in the app the hotkey was pressed in. */
function onSummon(selection) {
  touchedSinceSummon = false;
  asTyped = false;
  closePicker();
  if (selection) return useSelection(selection);
  if (config.start_in === 'source') return openPicker('from', true);
  focusInput();
  el.input.select(); // previous text stays visible; typing replaces it
}

function useSelection(text) {
  closePicker();
  el.input.value = text.trim();
  store.text = el.input.value;
  saveStore();
  fit(el.input, 120);
  focusInput();
  el.input.select();
  translateNow().catch(() => {});
}

async function loadConfig() {
  config = { ...config, ...(await api.config().catch(() => ({}))) };
  setStatus('');
}

(async () => {
  await loadConfig();
  el.input.value = store.text;
  fit(el.input, 120);
  renderLangs();
  setHints();
  api.on('summon', e => onSummon(e.payload));
  // The app took longer than the popup to copy its selection.
  api.on('selection', e => touchedSinceSummon || useSelection(e.payload));
  api.on('config-changed', loadConfig);
  if (el.input.value.trim()) translateNow().catch(() => {});
  onSummon();
})();
