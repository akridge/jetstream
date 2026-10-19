// Shared app state (Svelte 5 runes) and the live job stream.

import { api, eventsUrl, setApiBase } from './api.js';
import { jobDestination, normalizeDestination, basename } from './format.js';
import { ensureNotificationPermission, getServerStatus, notify, onServerStatus } from './platform.js';

const PREFS_KEY = 'jetstreamDesktopPrefs';
const DEFAULT_PREFS = { theme: 'system', notifications: true, default_upload_tool: 'gcloud' };

function loadPrefs() {
  try {
    return { ...DEFAULT_PREFS, ...JSON.parse(localStorage.getItem(PREFS_KEY) || '{}') };
  } catch {
    return { ...DEFAULT_PREFS };
  }
}

export const TERMINAL = new Set(['completed', 'failed', 'cancelled']);
export const ACTIVE = new Set(['pending', 'queued', 'running', 'scheduled']);

function lines(text) {
  const items = (text || '').split('\n').map((s) => s.trim()).filter(Boolean);
  return items.length ? items : null;
}

function commaList(text) {
  const items = (text || '').split(',').map((s) => s.trim()).filter(Boolean);
  return items.length ? items : null;
}

class AppState {
  server = $state({ phase: 'starting', base: null, message: null });
  view = $state('folders');
  jobs = $state({});
  presets = $state([]);
  auth = $state(null);
  tools = $state([]);
  version = $state('');
  live = $state(false);
  prefs = $state(loadPrefs());
  toasts = $state([]);
  loaded = $state(false);
  selectedJobId = $state(null);
  /** Add/edit folder dialog: { preset, previousName } or null. */
  folderDialog = $state(null);

  jobList = $derived(
    Object.values(this.jobs).sort((a, b) => (b.created_at || '').localeCompare(a.created_at || '')),
  );
  activeCount = $derived(this.jobList.filter((j) => ACTIVE.has(j.status)).length);
  failedCount = $derived(this.jobList.filter((j) => j.status === 'failed' && !j.cleared).length);

  #events = null;
  #speed = new Map();
  #toastId = 0;

  async start() {
    this.applyTheme();
    await onServerStatus((status) => this.#setServer(status));
    this.#setServer(await getServerStatus());
  }

  #setServer(status) {
    const wasReady = this.server.phase === 'ready';
    this.server = status;
    if (status.phase === 'ready' && !wasReady) {
      setApiBase(status.base);
      this.refreshAll();
      this.#connectEvents();
    }
  }

  async refreshAll() {
    await Promise.allSettled([this.refreshJobs(), this.refreshPresets(), this.refreshAuth(), this.refreshTools(), this.#loadVersion()]);
    this.loaded = true;
  }

  async #loadVersion() {
    this.version = (await api.version()).version || '';
  }

  async refreshJobs() {
    const list = await api.jobs(300);
    const next = {};
    for (const job of list) next[job.job_id] = job;
    this.jobs = next;
  }

  async refreshPresets() {
    this.presets = await api.presets();
  }

  async refreshAuth(force = false) {
    this.auth = await api.authStatus(force);
  }

  async refreshTools() {
    this.tools = await api.uploadTools();
  }

  #connectEvents() {
    this.#events?.close();
    const source = new EventSource(eventsUrl());
    this.#events = source;
    source.addEventListener('hello', () => {
      // Catch up on anything missed while disconnected.
      if (!this.live) this.refreshJobs();
      this.live = true;
    });
    source.addEventListener('job', (e) => this.#upsertJob(JSON.parse(e.data)));
    source.addEventListener('job_removed', (e) => {
      const { job_id } = JSON.parse(e.data);
      const { [job_id]: _removed, ...rest } = this.jobs;
      this.jobs = rest;
    });
    source.addEventListener('job_finished', (e) => this.#jobFinished(JSON.parse(e.data)));
    source.onerror = () => {
      this.live = false;
    };
  }

  #upsertJob(job) {
    const previous = this.jobs[job.job_id];
    if (job.status === 'running' && previous) {
      this.#sampleSpeed(job);
    }
    this.jobs[job.job_id] = job;
  }

  #sampleSpeed(job) {
    const now = Date.now();
    const sample = this.#speed.get(job.job_id);
    if (sample && now > sample.at) {
      const rate = ((job.bytes_uploaded || 0) - sample.bytes) / ((now - sample.at) / 1000);
      if (rate >= 0) sample.rate = sample.rate ? sample.rate * 0.7 + rate * 0.3 : rate;
      sample.bytes = job.bytes_uploaded || 0;
      sample.at = now;
    } else {
      this.#speed.set(job.job_id, { bytes: job.bytes_uploaded || 0, at: now, rate: 0 });
    }
  }

  /** Bytes per second for a running job (smoothed), or 0 if unknown. */
  speedOf(jobId) {
    return this.#speed.get(jobId)?.rate || 0;
  }

  #jobFinished(info) {
    const kind = info.dry_run ? 'Preview' : info.job_type === 'cloud_sync' ? 'Cloud sync' : info.job_type === 'cloud_move' ? 'Move' : 'Sync';
    const folder = this.presetForJob(this.jobs[info.job_id])?.name || info.name;
    const title =
      info.status === 'completed' ? `${kind} finished` : info.status === 'failed' ? `${kind} failed` : `${kind} cancelled`;
    this.toast(`${title}: ${folder}`, info.status === 'completed' ? 'success' : info.status === 'failed' ? 'error' : 'info', {
      action: { label: 'View', run: () => this.openJob(info.job_id) },
    });
    if (this.prefs.notifications) notify(title, `${folder}\n${info.destination || ''}`);
  }

  toast(message, kind = 'info', { action = null, timeout = 6000 } = {}) {
    const id = ++this.#toastId;
    this.toasts = [...this.toasts, { id, message, kind, action }].slice(-3);
    if (timeout) setTimeout(() => this.dismissToast(id), timeout);
  }

  dismissToast(id) {
    this.toasts = this.toasts.filter((t) => t.id !== id);
  }

  openJob(jobId) {
    this.view = 'activity';
    this.selectedJobId = jobId;
  }

  savePrefs(changes) {
    this.prefs = { ...this.prefs, ...changes };
    try {
      localStorage.setItem(PREFS_KEY, JSON.stringify(this.prefs));
    } catch {
      /* ignore */
    }
    this.applyTheme();
  }

  applyTheme() {
    const theme = this.prefs.theme;
    if (theme === 'system') delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = theme;
  }

  toolAvailable(id) {
    return !!this.tools.find((t) => t.id === id)?.available;
  }

  // ----- folders (presets) ---------------------------------------------------

  presetForJob(job) {
    if (!job) return null;
    const dest = jobDestination(job);
    return this.presets.find((p) => p.source_path === job.source_path && normalizeDestination(p.destination) === dest) || null;
  }

  jobsForPreset(preset) {
    const dest = normalizeDestination(preset.destination);
    return this.jobList.filter((j) => j.source_path === preset.source_path && jobDestination(j) === dest);
  }

  async savePreset(preset, previousName) {
    const saved = await api.savePreset(preset);
    if (previousName && previousName !== preset.name) {
      const old = this.presets.find((p) => p.name === previousName);
      if (old) await api.deletePreset(old.preset_id);
    }
    await this.refreshPresets();
    return saved;
  }

  async removePreset(preset) {
    await api.deletePreset(preset.preset_id);
    await this.refreshPresets();
  }

  uploadPayload(preset, dryRun) {
    const o = preset.options || {};
    const tool = o.upload_tool && this.toolAvailable(o.upload_tool) ? o.upload_tool : 'gcloud';
    return {
      source_path: preset.source_path,
      gcs_destination: normalizeDestination(preset.destination),
      dry_run: dryRun,
      recursive: o.recursive !== false,
      split_by_folder: false,
      upload_tool: tool,
      no_clobber: !!o.no_clobber,
      exclude_patterns: lines(o.exclude_patterns),
      exclude_folders: commaList(o.exclude_folders),
      threads: 4,
    };
  }

  async runPreset(preset, dryRun) {
    ensureNotificationPermission();
    try {
      const result = await api.createUpload(this.uploadPayload(preset, dryRun));
      this.toast(`${dryRun ? 'Previewing' : 'Syncing'} ${preset.name}…`, 'info', { timeout: 3000 });
      return result;
    } catch (err) {
      this.toast(`Couldn't start ${preset.name}: ${err.message}`, 'error', { timeout: 10000 });
      return null;
    }
  }

  addFolder(path = '') {
    this.view = 'folders';
    this.folderDialog = { preset: this.newPresetFor(path), previousName: null };
  }

  editFolder(preset) {
    this.folderDialog = { preset: structuredClone($state.snapshot(preset)), previousName: preset.name };
  }

  newPresetFor(path) {
    return {
      name: basename(path),
      source_path: path,
      destination: '',
      options: { recursive: true, no_clobber: false, upload_tool: this.prefs.default_upload_tool, exclude_patterns: '', exclude_folders: '' },
    };
  }
}

export const app = new AppState();
