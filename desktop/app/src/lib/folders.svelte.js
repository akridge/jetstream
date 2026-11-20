// One place that says what state a synced folder is in, so the folder cards, the list view,
// the filters, the sort order and the summary all agree.

import { app, ACTIVE, TERMINAL, collapseRuns, notVerified } from './state.svelte.js';
import { folderStatus } from './status.svelte.js';
import { api } from './api.js';

/**
 * kind: 'attention' (failed, not verified, couldn't check), 'syncing' (running, starting,
 * verifying), 'new' (new or changed files waiting), 'ok' (up to date), 'unknown' (not
 * synced or checked yet). rank orders them: what needs you first.
 */
export function folderState(preset) {
  const id = preset.preset_id;
  const runs = collapseRuns(app.jobsForPreset(preset));
  const current = runs.find((j) => ACTIVE.has(j.status));
  const lastFinished = runs.find((j) => TERMINAL.has(j.status));
  const lastReal = runs.find((j) => TERMINAL.has(j.status) && !j.dry_run && j.status === 'completed');
  const status = folderStatus.of(preset);
  const receipt = app.receipts[id];
  const unverified = receipt ? notVerified(receipt.totals) : 0;
  const failedNow =
    lastFinished?.status === 'failed' && !(status && !status.error && status.changes === 0 && status.checked_at > Date.parse(lastFinished.completed_at));
  const lastSynced = lastReal?.completed_at ? Date.parse(lastReal.completed_at) : null;
  const base = { current, lastFinished, lastSynced, status, receipt, unverified, changes: status && !status.error ? status.changes || 0 : 0, bytes: status?.bytes || 0 };

  if (current || app.starting[id] || app.verifying[id]) {
    const label = current ? (current.dry_run ? 'Previewing' : 'Syncing') : app.verifying[id] ? 'Verifying' : 'Starting';
    return { ...base, kind: 'syncing', rank: 1, label, progress: current?.progress_percent ?? null };
  }
  if (failedNow) return { ...base, kind: 'attention', rank: 0, label: 'Last sync failed' };
  if (unverified) return { ...base, kind: 'attention', rank: 0, label: `${unverified.toLocaleString()} not verified` };
  if (status?.error) return { ...base, kind: 'attention', rank: 0, label: "Couldn't check for changes" };
  if (base.changes) return { ...base, kind: 'new', rank: 2, label: `${base.changes.toLocaleString()} new or changed` };
  if (status || lastSynced) return { ...base, kind: 'ok', rank: 4, label: 'Up to date' };
  return { ...base, kind: 'unknown', rank: 3, label: lastFinished?.dry_run ? 'Preview ready' : 'Not synced yet' };
}

export const isHidden = (preset) => !!preset.options?.hidden;
export const tagsOf = (preset) => (Array.isArray(preset.options?.tags) ? preset.options.tags : []);

/** Every tag in use, alphabetically, with how many folders have it. */
export function allTags(presets) {
  const counts = new Map();
  for (const p of presets) for (const t of tagsOf(p)) counts.set(t, (counts.get(t) || 0) + 1);
  return [...counts.entries()].sort((a, b) => a[0].localeCompare(b[0], undefined, { sensitivity: 'base' }));
}

/** Change some of a folder's options (hidden, tags, ...) and save it. */
export async function updateOptions(preset, changes) {
  await api.savePreset({ name: preset.name, source_path: preset.source_path, destination: preset.destination, options: { ...preset.options, ...changes } });
}
