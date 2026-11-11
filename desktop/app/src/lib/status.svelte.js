// Every synced folder's status, kept up to date in the background: a quick compare with the
// cloud (sizes only, no file reads) when the app opens, after each sync, and every so often.
// Cards then simply say "Up to date" or "37 new files · Upload". Folders with auto-upload on
// are uploaded once a check finds changes and a second check, a couple of minutes later, finds
// nothing else changing (so a folder still being copied isn't uploaded half-done).

import { app, commaList, lines } from './state.svelte.js';
import { compareFolder, isDesktop } from './platform.js';
import { normalizeDestination } from './format.js';

const STORE_KEY = 'jetstream.folderStatus.v1';
/** Default minutes between background checks (a folder can choose its own; 0 = only on demand). */
export const DEFAULT_CHECK_MINUTES = 15;
/** How long changes must stay the same before an auto-upload starts. */
export const QUIET_MS = globalThis.__jetstreamQuietMs || 2 * 60 * 1000; // shorter in UI tests
const BUSY = new Set(['pending', 'queued', 'running']);

function load() {
  try {
    return JSON.parse(localStorage.getItem(STORE_KEY) || '{}') || {};
  } catch {
    return {};
  }
}

/** What a check depends on: a changed folder, destination or filter means checking again. */
function keyOf(preset) {
  const o = preset.options || {};
  return JSON.stringify([preset.source_path, normalizeDestination(preset.destination), o.exclude_patterns || '', o.exclude_folders || '', o.recursive !== false]);
}

export function checkMinutes(preset) {
  const value = preset.options?.check_minutes;
  return value === undefined || value === null || value === '' ? DEFAULT_CHECK_MINUTES : Number(value);
}

class FolderStatus {
  /** preset_id -> { key, checked_at, changes, bytes, local_only, different, same, cloud_only, verified, elapsed, error } */
  byId = $state(load());
  /** preset_id -> true while being checked. */
  checking = $state({});
  /** preset_id -> time (ms) an auto-upload starts if nothing else changes. */
  autoAt = $state({});

  #queue = [];
  #running = false;
  #started = false;
  #pending = {}; // preset_id -> { signature, at } for auto-upload

  start() {
    if (this.#started || !isDesktop) return;
    this.#started = true;
    // A moment after a run finishes (its final status arrives just before or after).
    app.onJobFinished((job, preset) => preset && setTimeout(() => this.check(preset, { soon: true }), 1500));
    setInterval(() => this.#tick(), 30_000);
    setTimeout(() => this.#tick(), 1500);
    document.addEventListener('visibilitychange', () => document.visibilityState === 'visible' && this.#tick());
  }

  /** Ask for a check (deduplicated; one runs at a time). */
  check(preset, { soon = false } = {}) {
    if (!isDesktop || !preset?.preset_id) return;
    if (this.#queue.some((p) => p.preset_id === preset.preset_id)) return;
    if (soon) this.#queue.unshift(preset);
    else this.#queue.push(preset);
    this.#drain();
  }

  checkAll() {
    for (const preset of app.presets) this.check(preset);
  }

  busy(preset) {
    return app.jobsForPreset(preset).some((j) => BUSY.has(j.status)) || !!app.starting[preset.preset_id];
  }

  #due(preset, now) {
    const status = this.byId[preset.preset_id];
    if (!status || status.key !== keyOf(preset)) return true;
    if (this.busy(preset)) return false;
    const minutes = checkMinutes(preset);
    if (!minutes) return false;
    // Big folders (e.g. a network share that takes minutes to walk) are checked less often.
    const every = Math.max(minutes * 60_000, (status.elapsed || 0) * 20_000);
    return now - (status.checked_at || 0) >= every;
  }

  #tick() {
    const now = Date.now();
    for (const preset of app.presets) if (this.#due(preset, now)) this.check(preset);
  }

  async #drain() {
    if (this.#running) return;
    this.#running = true;
    try {
      while (this.#queue.length) {
        const queued = this.#queue.shift();
        const preset = app.presets.find((p) => p.preset_id === queued.preset_id);
        if (!preset || this.busy(preset)) continue;
        await this.#run(preset);
      }
    } finally {
      this.#running = false;
    }
  }

  async #run(preset) {
    const id = preset.preset_id;
    const o = preset.options || {};
    this.checking = { ...this.checking, [id]: true };
    const key = keyOf(preset);
    let status;
    try {
      const r = await compareFolder(preset.source_path, `gs://${normalizeDestination(preset.destination)}/`, {
        excludePatterns: lines(o.exclude_patterns) || [],
        excludeFolders: commaList(o.exclude_folders) || [],
        recursive: o.recursive !== false,
        lists: false,
      });
      status = {
        key,
        checked_at: Date.now(),
        changes: r.local_only.files + r.different.files,
        bytes: r.local_only.bytes + r.different.bytes,
        local_only: r.local_only.files,
        different: r.different.files,
        same: r.same.files,
        same_bytes: r.same.bytes,
        cloud_only: r.cloud_only.files,
        verified: r.verified,
        local_files: r.local_files,
        online_only: r.online_only,
        elapsed: r.elapsed_seconds,
        error: null,
      };
    } catch (err) {
      const previous = this.byId[id];
      status = { ...(previous?.key === key ? previous : { key }), checked_at: Date.now(), error: String(err?.message || err) };
    } finally {
      const { [id]: _, ...rest } = this.checking;
      this.checking = rest;
    }
    this.byId = { ...this.byId, [id]: status };
    this.#save();
    this.#maybeAutoUpload(preset, status);
  }

  #maybeAutoUpload(preset, status) {
    const id = preset.preset_id;
    const clear = () => {
      delete this.#pending[id];
      const { [id]: _, ...rest } = this.autoAt;
      this.autoAt = rest;
    };
    if (!preset.options?.auto_upload || status.error || !status.changes || this.busy(preset)) {
      clear();
      return;
    }
    const signature = `${status.changes}:${status.bytes}`;
    const pending = this.#pending[id];
    if (pending && pending.signature === signature && Date.now() - pending.at >= QUIET_MS - 5000) {
      clear();
      app.runPreset(preset, false, { reason: "auto" });
      return;
    }
    // First sight of these changes (or still changing): look again after a quiet period.
    this.#pending[id] = { signature, at: Date.now() };
    this.autoAt = { ...this.autoAt, [id]: Date.now() + QUIET_MS };
    setTimeout(() => {
      const current = app.presets.find((p) => p.preset_id === id);
      if (current?.options?.auto_upload) this.check(current, { soon: true });
    }, QUIET_MS);
  }

  /** Forget a folder (removed). */
  forget(id) {
    const { [id]: _, ...rest } = this.byId;
    this.byId = rest;
    delete this.#pending[id];
    this.#save();
  }

  #save() {
    try {
      localStorage.setItem(STORE_KEY, JSON.stringify(this.byId));
    } catch {
      /* ignore */
    }
  }

  /** Changes waiting across all folders (for the "Upload all" bar). */
  get pending() {
    return app.presets
      .map((preset) => ({ preset, status: this.byId[preset.preset_id] }))
      .filter(({ preset, status }) => status && !status.error && status.changes > 0 && status.key === keyOf(preset) && !this.busy(preset));
  }

  /** The status for a folder, if it still matches the folder's settings. */
  of(preset) {
    const status = this.byId[preset.preset_id];
    return status && status.key === keyOf(preset) ? status : null;
  }
}

export const folderStatus = new FolderStatus();
