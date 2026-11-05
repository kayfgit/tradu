// Languages, and matching typed names to them. Kept free of the DOM so tests
// can import it.

/** @typedef {[code: string, name: string, native: string]} Lang */

/** @type {Lang[]} [Google code, English name, native name] */
export const LANGS = [
  ['af', 'Afrikaans', 'Afrikaans'],
  ['sq', 'Albanian', 'Shqip'],
  ['am', 'Amharic', 'አማርኛ'],
  ['ar', 'Arabic', 'العربية'],
  ['hy', 'Armenian', 'Հայերեն'],
  ['as', 'Assamese', 'অসমীয়া'],
  ['ay', 'Aymara', 'Aymar aru'],
  ['az', 'Azerbaijani', 'Azərbaycanca'],
  ['bm', 'Bambara', 'Bamanankan'],
  ['eu', 'Basque', 'Euskara'],
  ['be', 'Belarusian', 'Беларуская'],
  ['bn', 'Bengali', 'বাংলা'],
  ['bho', 'Bhojpuri', 'भोजपुरी'],
  ['bs', 'Bosnian', 'Bosanski'],
  ['bg', 'Bulgarian', 'Български'],
  ['yue', 'Cantonese', '粵語'],
  ['ca', 'Catalan', 'Català'],
  ['ceb', 'Cebuano', 'Cebuano'],
  ['ny', 'Chichewa', 'Chichewa'],
  ['zh-CN', 'Chinese (Simplified)', '简体中文'],
  ['zh-TW', 'Chinese (Traditional)', '繁體中文'],
  ['co', 'Corsican', 'Corsu'],
  ['hr', 'Croatian', 'Hrvatski'],
  ['cs', 'Czech', 'Čeština'],
  ['da', 'Danish', 'Dansk'],
  ['dv', 'Dhivehi', 'ދިވެހި'],
  ['doi', 'Dogri', 'डोगरी'],
  ['nl', 'Dutch', 'Nederlands'],
  ['en', 'English', 'English'],
  ['eo', 'Esperanto', 'Esperanto'],
  ['et', 'Estonian', 'Eesti'],
  ['ee', 'Ewe', 'Eʋegbe'],
  ['tl', 'Filipino', 'Filipino'],
  ['fi', 'Finnish', 'Suomi'],
  ['fr', 'French', 'Français'],
  ['fr-CA', 'French (Canada)', 'Français canadien'],
  ['fy', 'Frisian', 'Frysk'],
  ['gl', 'Galician', 'Galego'],
  ['ka', 'Georgian', 'ქართული'],
  ['de', 'German', 'Deutsch'],
  ['el', 'Greek', 'Ελληνικά'],
  ['gn', 'Guarani', 'Avañeʼẽ'],
  ['gu', 'Gujarati', 'ગુજરાતી'],
  ['ht', 'Haitian Creole', 'Kreyòl ayisyen'],
  ['ha', 'Hausa', 'Hausa'],
  ['haw', 'Hawaiian', 'ʻŌlelo Hawaiʻi'],
  ['iw', 'Hebrew', 'עברית'],
  ['hi', 'Hindi', 'हिन्दी'],
  ['hmn', 'Hmong', 'Hmoob'],
  ['hu', 'Hungarian', 'Magyar'],
  ['is', 'Icelandic', 'Íslenska'],
  ['ig', 'Igbo', 'Igbo'],
  ['ilo', 'Ilocano', 'Ilokano'],
  ['id', 'Indonesian', 'Bahasa Indonesia'],
  ['ga', 'Irish', 'Gaeilge'],
  ['it', 'Italian', 'Italiano'],
  ['ja', 'Japanese', '日本語'],
  ['jw', 'Javanese', 'Basa Jawa'],
  ['kn', 'Kannada', 'ಕನ್ನಡ'],
  ['kk', 'Kazakh', 'Қазақ тілі'],
  ['km', 'Khmer', 'ខ្មែរ'],
  ['rw', 'Kinyarwanda', 'Ikinyarwanda'],
  ['gom', 'Konkani', 'कोंकणी'],
  ['ko', 'Korean', '한국어'],
  ['kri', 'Krio', 'Krio'],
  ['ku', 'Kurdish (Kurmanji)', 'Kurmancî'],
  ['ckb', 'Kurdish (Sorani)', 'کوردی'],
  ['ky', 'Kyrgyz', 'Кыргызча'],
  ['lo', 'Lao', 'ລາວ'],
  ['la', 'Latin', 'Latina'],
  ['lv', 'Latvian', 'Latviešu'],
  ['ln', 'Lingala', 'Lingála'],
  ['lt', 'Lithuanian', 'Lietuvių'],
  ['lg', 'Luganda', 'Luganda'],
  ['lb', 'Luxembourgish', 'Lëtzebuergesch'],
  ['mk', 'Macedonian', 'Македонски'],
  ['mai', 'Maithili', 'मैथिली'],
  ['mg', 'Malagasy', 'Malagasy'],
  ['ms', 'Malay', 'Bahasa Melayu'],
  ['ml', 'Malayalam', 'മലയാളം'],
  ['mt', 'Maltese', 'Malti'],
  ['mi', 'Maori', 'Māori'],
  ['mr', 'Marathi', 'मराठी'],
  ['mni-Mtei', 'Meiteilon (Manipuri)', 'ꯃꯤꯇꯩꯂꯣꯟ'],
  ['lus', 'Mizo', 'Mizo ṭawng'],
  ['mn', 'Mongolian', 'Монгол'],
  ['my', 'Myanmar (Burmese)', 'မြန်မာ'],
  ['ne', 'Nepali', 'नेपाली'],
  ['no', 'Norwegian', 'Norsk'],
  ['or', 'Odia (Oriya)', 'ଓଡ଼ିଆ'],
  ['om', 'Oromo', 'Afaan Oromoo'],
  ['ps', 'Pashto', 'پښتو'],
  ['fa', 'Persian', 'فارسی'],
  ['pl', 'Polish', 'Polski'],
  ['pt', 'Portuguese (Brazil)', 'Português'],
  ['pt-PT', 'Portuguese (Portugal)', 'Português europeu'],
  ['pa', 'Punjabi', 'ਪੰਜਾਬੀ'],
  ['qu', 'Quechua', 'Runasimi'],
  ['ro', 'Romanian', 'Română'],
  ['ru', 'Russian', 'Русский'],
  ['sm', 'Samoan', 'Gagana Samoa'],
  ['sa', 'Sanskrit', 'संस्कृतम्'],
  ['gd', 'Scots Gaelic', 'Gàidhlig'],
  ['nso', 'Sepedi', 'Sepedi'],
  ['sr', 'Serbian', 'Српски'],
  ['st', 'Sesotho', 'Sesotho'],
  ['sn', 'Shona', 'chiShona'],
  ['sd', 'Sindhi', 'سنڌي'],
  ['si', 'Sinhala', 'සිංහල'],
  ['sk', 'Slovak', 'Slovenčina'],
  ['sl', 'Slovenian', 'Slovenščina'],
  ['so', 'Somali', 'Soomaali'],
  ['es', 'Spanish', 'Español'],
  ['su', 'Sundanese', 'Basa Sunda'],
  ['sw', 'Swahili', 'Kiswahili'],
  ['sv', 'Swedish', 'Svenska'],
  ['tg', 'Tajik', 'Тоҷикӣ'],
  ['ta', 'Tamil', 'தமிழ்'],
  ['tt', 'Tatar', 'Татарча'],
  ['te', 'Telugu', 'తెలుగు'],
  ['th', 'Thai', 'ไทย'],
  ['bo', 'Tibetan', 'བོད་སྐད'],
  ['ti', 'Tigrinya', 'ትግርኛ'],
  ['ts', 'Tsonga', 'Xitsonga'],
  ['tr', 'Turkish', 'Türkçe'],
  ['tk', 'Turkmen', 'Türkmençe'],
  ['ak', 'Twi', 'Twi'],
  ['uk', 'Ukrainian', 'Українська'],
  ['ur', 'Urdu', 'اردو'],
  ['ug', 'Uyghur', 'ئۇيغۇرچە'],
  ['uz', 'Uzbek', 'Oʻzbekcha'],
  ['vi', 'Vietnamese', 'Tiếng Việt'],
  ['cy', 'Welsh', 'Cymraeg'],
  ['xh', 'Xhosa', 'isiXhosa'],
  ['yi', 'Yiddish', 'ייִדיש'],
  ['yo', 'Yoruba', 'Yorùbá'],
  ['zu', 'Zulu', 'isiZulu'],
];

// Ranked first when queries tie, and fill the quick-pick list before any recents exist.
export const POPULAR = [
  'en',
  'es',
  'fr',
  'de',
  'zh-CN',
  'ja',
  'ko',
  'pt',
  'ru',
  'it',
  'ar',
  'hi',
  'tr',
  'nl',
  'pl',
  'uk',
  'vi',
  'id',
  'sv',
  'zh-TW',
];

// Codes users (or Google's detector) may use for languages listed under another code.
export const ALIASES = { he: 'iw', jv: 'jw', zh: 'zh-CN', fil: 'tl', nb: 'no', 'pt-BR': 'pt' };

/** @type {Lang} */
export const AUTO = ['auto', 'Detect language', ''];
export const byCode = new Map(LANGS.map(l => [l[0], l]));
/** @param {string} code */
export const canon = code => ALIASES[code] ?? code;
/** @param {string} code */
export const nameOf = code => (code === 'auto' ? 'Detect language' : (byCode.get(canon(code))?.[1] ?? code));

/** Lowercase, without accents, so "francais" finds "Français". */
export const normalize = s => s.normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();

/**
 * How well `q` (normalized) matches a language; 0 is no match.
 * @param {Lang} lang
 * @param {string} q
 */
export function score([code, name, native], q) {
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

/**
 * The languages offered for `query` in the source ("from") or target ("to")
 * picker, best first. `recent` are this side's recently used codes, newest first.
 * @param {'from' | 'to'} which
 * @param {string} query
 * @param {string[]} recent
 */
export function findMatches(which, query, recent) {
  const pool = which === 'from' ? [AUTO, ...LANGS] : LANGS;
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
