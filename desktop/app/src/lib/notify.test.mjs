// npm run test:ui
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { inQuietHours, notifyLevel } from './notify.js';

test('notification level, including older settings', () => {
  assert.equal(notifyLevel({}), 'all');
  assert.equal(notifyLevel({ notifications: false }), 'off');
  assert.equal(notifyLevel({ notifications: true, notify_level: 'problems' }), 'problems');
});

test('quiet hours, across midnight and not', () => {
  const at = (h, m = 0) => new Date(2026, 0, 1, h, m);
  const night = { on: true, from: '22:00', to: '07:00' };
  assert.equal(inQuietHours(night, at(23)), true);
  assert.equal(inQuietHours(night, at(3)), true);
  assert.equal(inQuietHours(night, at(7)), false);
  assert.equal(inQuietHours(night, at(12)), false);
  const lunch = { on: true, from: '12:00', to: '13:30' };
  assert.equal(inQuietHours(lunch, at(13, 15)), true);
  assert.equal(inQuietHours(lunch, at(13, 30)), false);
  assert.equal(inQuietHours({ ...night, on: false }, at(23)), false);
  assert.equal(inQuietHours(undefined, at(23)), false);
  assert.equal(inQuietHours({ on: true, from: '09:00', to: '09:00' }, at(9)), false);
});
