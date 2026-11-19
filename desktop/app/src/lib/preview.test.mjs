// Tests for the preview pane's helpers: npm run test:ui
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseCsv, sniffDelimiter, previewKind, prettyText, objectFacts } from './preview.js';

test('preview helpers', () => {
  let r = parseCsv('a,b\n1,"x, y"\n2,"say ""hi"""\n3,"two\nlines"\n');
  assert.deepEqual(r.rows, [['a','b'],['1','x, y'],['2','say "hi"'],['3','two\nlines']]); assert.equal(r.more, false);
  r = parseCsv('a,b\r\n1,2\r\n3,4', {});
  assert.deepEqual(r.rows, [['a','b'],['1','2'],['3','4']]);
  r = parseCsv('a,b\n1,2\n3,"cut off', { truncated: true });
  assert.deepEqual(r.rows, [['a','b'],['1','2']]); assert.equal(r.more, true);
  r = parseCsv('a,b\n1,2\n3,4', { truncated: true });
  assert.deepEqual(r.rows, [['a','b'],['1','2']], 'partial last line dropped');
  r = parseCsv(Array.from({length: 500}, (_, i) => `${i},x`).join('\n') + '\n', { maxRows: 200 });
  assert.equal(r.rows.length, 200); assert.equal(r.more, true);
  assert.equal(sniffDelimiter('a;b;c\n1;2;3'), ';'); assert.equal(sniffDelimiter('a,b', 'x.tsv'), '\t'); assert.equal(sniffDelimiter('a\tb\tc'), '\t');
  assert.equal(previewKind({ name: 'IMG.JPG' }), 'image'); assert.equal(previewKind({ name: 'ctd.csv' }), 'table');
  assert.equal(previewKind({ name: 'notes.md' }), 'text'); assert.equal(previewKind({ name: 'sst.nc' }), null);
  assert.equal(previewKind({ name: 'x', content_type: 'application/json' }), 'text'); assert.equal(previewKind({ name: 'f/', is_folder: true }), null);
  assert.equal(prettyText('{"a":1}', { name: 'a.json' }, false), '{\n  "a": 1\n}'); assert.equal(prettyText('{"a":', { name: 'a.json' }, true), '{"a":');
  const f = objectFacts({ contentType: 'text/csv', size: '12', generation: '7', componentCount: 3, metadata: { 'goog-reserved-file-mtime': '1600000000', 'jetstream-file-created': '1500000000', project: 'HA2401' }, timeCreated: 't', updated: 't' });
  assert.equal(f.modified, 1600000000000); assert.equal(f.created, 1500000000000); assert.equal(f.composite, true); assert.deepEqual(f.custom, [['project','HA2401']]); assert.equal(f.updated, null);
  assert.equal(objectFacts({ timeCreated: '2024-01-01T00:00:00.000Z', updated: '2024-01-01T00:00:00.400Z' }).updated, null);
  assert.equal(objectFacts({ timeCreated: '2024-01-01T00:00:00Z', updated: '2024-02-01T00:00:00Z' }).updated, '2024-02-01T00:00:00Z');
});
