import assert from 'node:assert/strict';
import { test } from 'node:test';

import { canon, findMatches, nameOf, score } from '../src/langs.js';

const codes = langs => langs.map(l => l[0]);

test('aliases resolve to the code Google uses', () => {
  assert.equal(canon('he'), 'iw');
  assert.equal(canon('zh'), 'zh-CN');
  assert.equal(canon('ja'), 'ja');
  assert.equal(nameOf('he'), 'Hebrew');
  assert.equal(nameOf('auto'), 'Detect language');
  assert.equal(nameOf('xx'), 'xx');
});

test('score ranks exact codes over name prefixes over fuzzy matches', () => {
  const german = ['de', 'German', 'Deutsch'];
  assert.equal(score(german, 'de'), 100);
  assert.equal(score(german, 'ger'), 80);
  assert.equal(score(german, 'deu'), 75);
  assert.equal(score(german, 'gmn'), 10);
  assert.equal(score(german, 'xyz'), 0);
});

test('matching ignores accents and case', () => {
  assert.equal(findMatches('to', 'francais', [])[0][0], 'fr');
  assert.equal(findMatches('to', 'FRENCH', [])[0][0], 'fr');
});

test('codes, aliases and native names find their language', () => {
  assert.equal(findMatches('to', 'ja', [])[0][0], 'ja');
  assert.equal(findMatches('to', 'zh', [])[0][0], 'zh-CN');
  assert.equal(findMatches('to', 'deutsch', [])[0][0], 'de');
});

test('recent languages win ties', () => {
  assert.deepEqual(codes(findMatches('to', 'chinese', [])).slice(0, 2), ['zh-CN', 'zh-TW']);
  assert.deepEqual(codes(findMatches('to', 'chinese', ['zh-TW'])).slice(0, 2), ['zh-TW', 'zh-CN']);
});

test('an empty query lists recents, then popular languages', () => {
  const to = codes(findMatches('to', '', ['ja', 'ko']));
  assert.deepEqual(to.slice(0, 3), ['ja', 'ko', 'en']);
  assert.equal(to.length, 9);
  assert.equal(new Set(to).size, to.length);
});

test('only the source side offers auto-detect', () => {
  assert.equal(codes(findMatches('from', '', [])).includes('auto'), true);
  assert.equal(codes(findMatches('to', '', [])).includes('auto'), false);
  assert.equal(findMatches('from', 'detect', [])[0][0], 'auto');
});

test('no match gives an empty list', () => {
  assert.deepEqual(findMatches('to', 'qqqq', []), []);
});
