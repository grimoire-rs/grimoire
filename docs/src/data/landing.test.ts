import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { entryPaths, routerCards } from './landing.ts';

const distDir = path.resolve(import.meta.dirname, '../../dist');

test('routerCards has 8 entries', () => {
  assert.equal(routerCards.length, 8);
});

test('entryPaths has 3 entries', () => {
  assert.equal(entryPaths.length, 3);
});

test('every href ends with .html', () => {
  for (const { href } of entryPaths) {
    assert.ok(href.endsWith('.html'), `entryPaths href "${href}" must end with .html`);
  }
  for (const { href } of routerCards) {
    assert.ok(href.endsWith('.html'), `routerCards href "${href}" must end with .html`);
  }
});

test('hrefs resolve to built files under docs/dist/', (t) => {
  if (!existsSync(distDir)) {
    t.skip('docs/dist/ absent — run `npm run build` first to check hrefs');
    return;
  }
  const hrefs = [...entryPaths.map((e) => e.href), ...routerCards.map((c) => c.href)];
  for (const href of hrefs) {
    const filePath = path.join(distDir, href.replace(/^\//, ''));
    assert.ok(existsSync(filePath), `expected built file for href "${href}" at ${filePath}`);
  }
});

test('routerCards pain strings are non-empty and unique', () => {
  const pains = routerCards.map((c) => c.pain);
  for (const pain of pains) {
    assert.ok(pain.trim().length > 0, 'pain string must be non-empty');
  }
  assert.equal(new Set(pains).size, pains.length, 'pain strings must be unique');
});
