// npm run test:ui
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { releaseNotes } from './changelog.js';
import { checkMilestones } from './milestones.js';

test('release notes from the changelog', () => {
  const md = '# Changes\n\n## 0.3.0\nBetter.\n\n### Uploads\n- **Fast** now\n- `code` here\n\n## 0.2.0\n- old\n';
  const notes = releaseNotes(md, '0.3.0');
  assert.equal(notes.intro, 'Better.');
  assert.equal(notes.groups[0].title, 'Uploads');
  assert.deepEqual(notes.groups[0].items[0], [{ text: 'Fast', bold: true }, { text: ' now' }]);
  assert.deepEqual(notes.groups[0].items[1], [{ text: 'code here' }]);
  assert.equal(releaseNotes(md, '9.9.9'), null);
  assert.equal(releaseNotes(md, '0.2.0').groups[0].items.length, 1);
  // The real changelog has notes for the app's version.
  const version = JSON.parse(readFileSync(new URL('../../../src-tauri/tauri.conf.json', import.meta.url))).version;
  const real = releaseNotes(readFileSync(new URL('../../../CHANGELOG.md', import.meta.url), 'utf8'), version);
  assert.ok(real && real.groups.length >= 3, 'CHANGELOG.md has a section for the current version');
});

test('milestones: quiet the first time, then the biggest new one of each kind', () => {
  const GB = 1024 ** 3;
  const first = checkMilestones({ bytes: 50 * GB, files: 5000 }, null);
  assert.deepEqual(first.celebrate, []);
  assert.ok(first.seen.includes('bytes-10') && first.seen.includes('files-1000'));
  const next = checkMilestones({ bytes: 1500 * GB, files: 5000 }, first.seen);
  assert.deepEqual(next.celebrate.map((m) => m.label), ['1 TB']);
  assert.ok(next.seen.includes('bytes-100') && next.seen.includes('bytes-1024'));
  assert.deepEqual(checkMilestones({ bytes: 1500 * GB, files: 5000 }, next.seen).celebrate, []);
  assert.deepEqual(checkMilestones({ bytes: 0, files: 12000 }, ['files-1000']).celebrate.map((m) => m.label), ['10,000 files']);
});
