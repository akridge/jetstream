// Shared app state (Svelte 5 runes) and the live job stream.

import { api, eventsUrl, setApiBase } from './api.js';
import { jobDestination, normalizeDestination, basename } from './format.js';
import { ensureNotificationPermission, getServerStatus, listReceipts, makeReceipt, newOpId, notify, onServerStatus, revealPath, setTransferLimit } from './platform.js';
import { cloudOps } from './cloudops.svelte.js';

const PREFS_KEY = 'jetstreamDesktopPrefs';
const DEFAULT_PREFS = { theme: 'system', notifications: true, default_upload_tool: 'gcloud', recent_buckets: [], recent_scans: [] };

function loadPrefs() {
  try {
    return { ...DEFAULT_PREFS, ...JSON.parse(localStorage.getItem(PREFS_KEY) || '{}') };
  } catch {
    return { ...DEFAULT_PREFS };
  }
}

export const TERMINAL = new Set(['completed', 'failed', 'cancelled']);
export const ACTIVE = new Set(['pending', 'queued', 'running', 'scheduled']);

/**
 * Jobs as the user thinks of them: a split upload (one job per subfolder) is one run.
 * A run is a job, or a combined job-like object with `group` = { id, name, parts, done,
 * failed, cancelled, active, jobs, current } for split uploads. Newest first.
 */
export function collapseRuns(jobs) {
  const groups = new Map();
  const runs = [];
  for (const job of jobs) {
    const id = job.filters?.group?.id;
    if (!id) {
      runs.push(job);
      continue;
    }
    if (!groups.has(id)) {
      groups.set(id, []);
      runs.push(id);
    }
    groups.get(id).push(job);
  }
  return runs.map((run) => (typeof run === 'string' ? groupRun(groups.get(run)) : run));
}

function groupRun(jobs) {
  const info = jobs[0].filters.group;
  const sum = (key) => jobs.reduce((total, j) => total + (Number(j[key]) || 0), 0);
  const byStatus = (statuses) => jobs.filter((j) => statuses.includes(j.status));
  const running = byStatus(['running']);
  const active = byStatus(['running', 'queued', 'pending', 'scheduled']);
  const failed = byStatus(['failed']);
  const cancelled = byStatus(['cancelled']);
  const done = byStatus(['completed']);
  const status = running.length ? 'running' : active.length ? active[0].status : failed.length ? 'failed' : cancelled.length ? 'cancelled' : 'completed';
  const total = sum('total_size_bytes');
  const uploaded = sum('bytes_uploaded') + done.reduce((t, j) => t + (j.bytes_uploaded ? 0 : Number(j.total_size_bytes) || 0), 0);
  const times = (key) => jobs.map((j) => j[key]).filter(Boolean).sort();
  const current = running[0] || active[0] || null;
  return {
    ...jobs[0],
    job_id: current?.job_id || failed[0]?.job_id || jobs[0].job_id,
    status,
    source_path: info.source_path,
    destination_bucket: info.destination.split('/')[0],
    destination_path: info.destination.split('/').slice(1).join('/'),
    total_files: sum('total_files'),
    files_uploaded: sum('files_uploaded'),
    total_size_bytes: total,
    bytes_uploaded: Math.min(uploaded, total),
    progress_percent: total ? (Math.min(uploaded, total) / total) * 100 : (done.length / jobs.length) * 100,
    created_at: times('created_at')[0] || jobs[0].created_at,
    started_at: times('started_at')[0] || null,
    completed_at: active.length ? null : times('completed_at').pop() || null,
    duration_seconds: sum('duration_seconds') || null,
    error_message: failed.length ? `${failed.length} of ${jobs.length} folders failed` : null,
    group: { ...info, jobs, done: done.length, failed: failed.length, cancelled: cancelled.length, active: active.length, current },
  };
}

export function lines(text) {
  const items = (text || '').split('\n').map((s) => s.trim()).filter(Boolean);
  return items.length ? items : null;
}

export function commaList(text) {
  const items = (text || '').split(',').map((s) => s.trim()).filter(Boolean);
  return items.length ? items : null;
}

class AppState {
  server = $state({ phase: 'starting', base: null, message: null });
  view = $state('folders');
  jobs = $state({});
  /** Folders (by preset id) whose sync or preview was asked for and hasn't shown up as a job
   * yet: their buttons stay disabled and the card says "Starting…", so a slow start never
   * invites a second click. */
  starting = $state({});
  /** The "Add several folders" dialog: { parent?, paths? } or null. */
  addFoldersDialog = $state(null);
  /** Folders (by preset id) to upload as soon as their running preview finishes. */
  uploadAfterPreview = $state({});
  presets = $state([]);
  auth = $state(null);
  tools = $state([]);
  version = $state('');
  live = $state(false);
  prefs = $state(loadPrefs());
  toasts = $state([]);
  loaded = $state(false);
  selectedJobId = $state(null);
  /** Folder the Insights view should scan next (set by drag-and-drop or other views). */
  scanRequest = $state(null);
  /** Add/edit folder dialog: { preset, previousName } or null. */
  folderDialog = $state(null);
  /** Upload speed limit (Mbit/s, 0 = none), shared with the server. */
  transfer = $state({ max_mbps: 0, parallel_jobs: 1, parallel_parts: true });
  /** Paths dropped on the Cloud storage view, waiting to be uploaded there. */
  cloudDrop = $state(null);
  /** Latest receipt per synced folder (preset_id -> summary), and a counter to reload lists. */
  receipts = $state({});
  receiptsVersion = $state(0);
  /** Synced folder shown on the Compare screen. */
  compareTarget = $state(null);
  /** Last place browsed in Cloud storage, kept while switching views. */
  cloudLocation = null;

  jobList = $derived(
    Object.values(this.jobs).sort((a, b) => (b.created_at || '').localeCompare(a.created_at || '')),
  );
  activeCount = $derived(this.jobList.filter((j) => ACTIVE.has(j.status)).length);
  failedCount = $derived(this.jobList.filter((j) => j.status === 'failed' && !j.cleared).length);

  #events = null;
  #speed = new Map();
  #toastId = 0;
  /** Split uploads already announced as finished (cleared when one runs again). */
  #announcedGroups = new Set();

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
    await Promise.allSettled([
      this.refreshJobs(),
      this.refreshPresets(),
      this.refreshAuth(),
      this.refreshTools(),
      this.#loadVersion(),
      this.#loadTransfer(),
    ]);
    this.loaded = true;
  }

  async #loadTransfer() {
    this.transfer = await api.transferSettings();
    await setTransferLimit(this.transfer.max_mbps);
  }

  async saveTransfer(changes) {
    this.transfer = await api.saveTransferSettings({ ...this.transfer, ...changes });
    await setTransferLimit(this.transfer.max_mbps);
    return this.transfer;
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
    if (job.filters?.group?.id && ACTIVE.has(job.status)) this.#announcedGroups.delete(job.filters.group.id);
    if (job.status === 'running' && previous) {
      this.#sampleSpeed(job);
    }
    this.jobs[job.job_id] = job;
    // A schedule just fired: the folder's "next sync" time has moved on.
    if (!previous && job.filters?.trigger === 'schedule') this.refreshPresets().catch(() => {});
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

  /** Check a synced folder against the cloud and save a receipt (shown in the operations tray). */
  async createReceipt(preset, { auto = false } = {}) {
    const o = preset.options || {};
    const id = newOpId();
    const summary = await cloudOps.start(id, `Verifying ${preset.name}`, 0, 'Verified', (onProgress) =>
      makeReceipt(id, {
        folderName: preset.name,
        path: preset.source_path,
        destination: `gs://${normalizeDestination(preset.destination)}/`,
        excludePatterns: lines(o.exclude_patterns) || [],
        excludeFolders: commaList(o.exclude_folders) || [],
        recursive: o.recursive !== false,
        account: this.auth?.gcloud_account || this.auth?.service_account || null,
        cloudCopy: !!o.receipt_cloud_copy,
      }, onProgress), {
      report: (r) => {
        const t = r.totals || {};
        const problems = (t.mismatch || 0) + (t.missing_in_cloud || 0) + (t.unreadable || 0) + (t.online_only || 0);
        const message = problems
          ? `Receipt for ${preset.name}: ${problems.toLocaleString()} of ${(t.local_files || 0).toLocaleString()} files not verified`
          : `Receipt for ${preset.name}: all ${(t.verified || 0).toLocaleString()} files verified`;
        // An automatic receipt that verified everything shows on the folder card; no toast.
        if (auto && !problems) return;
        this.toast(message, problems ? 'error' : 'success', {
          action: problems ? { label: 'Compare', run: () => this.compareFolder(preset) } : { label: 'Show', run: () => revealPath(r.csv) },
          timeout: auto && !problems ? 6000 : 12000,
        });
      },
    });
    if (summary) {
      this.receipts = { ...this.receipts, [preset.preset_id]: summary };
      this.receiptsVersion++;
    }
    return summary;
  }

  async loadReceipts(preset) {
    const list = (await listReceipts(preset.name).catch(() => [])) || [];
    if (list[0]) this.receipts = { ...this.receipts, [preset.preset_id]: list[0] };
    return list;
  }

  /** After a real sync of a synced folder completes, make its receipt (unless turned off). */
  #autoReceipt(job) {
    const preset = this.presetForJob(job);
    if (!preset || job.dry_run || preset.options?.receipts === false) return;
    this.createReceipt(preset, { auto: true });
  }

  #finishedHooks = [];
  /** Call fn(job, preset) when a job (or a whole split upload) finishes. */
  onJobFinished(fn) {
    this.#finishedHooks.push(fn);
  }

  #jobFinished(info) {
    const job0 = this.jobs[info.job_id];
    const group0 = job0?.filters?.group?.id;
    const lastOfGroup = !group0 || !this.jobList.some((j) => j.filters?.group?.id === group0 && j.job_id !== info.job_id && ACTIVE.has(j.status));
    if (job0 && lastOfGroup) for (const fn of this.#finishedHooks) fn(job0, this.presetForJob(job0));
    const kind = info.dry_run ? 'Preview' : info.job_type === 'cloud_sync' ? 'Cloud sync' : info.job_type === 'cloud_move' ? 'Move' : 'Sync';
    const job = this.jobs[info.job_id];
    const groupId = job?.filters?.group?.id;
    if (groupId) {
      // A split upload: one message when its last subfolder finishes.
      const siblings = this.jobList.filter((j) => j.filters?.group?.id === groupId);
      if (siblings.some((j) => j.job_id !== info.job_id && ACTIVE.has(j.status))) return;
      if (this.#announcedGroups.has(groupId)) return;
      this.#announcedGroups.add(groupId);
      if (info.status === 'completed' && !siblings.some((j) => j.job_id !== info.job_id && j.status !== 'completed')) this.#autoReceipt(job);
      const run = collapseRuns(siblings)[0];
      if (job.dry_run && this.#continueAfterPreview(job, run.status === 'completed' && !run.group.failed)) return;
      const folder = this.presetForJob(job)?.name || run.group.name;
      const failed = run.group.failed;
      const title = failed ? `${kind} finished with ${failed} failed ${failed === 1 ? 'folder' : 'folders'}` : run.status === 'cancelled' ? `${kind} cancelled` : `${kind} finished`;
      if (!failed && run.status === 'completed') this.#announceSuccess(kind, `${folder} (${run.group.parts} folders)`, info.job_id);
      else
        this.toast(`${title}: ${folder} (${run.group.parts} folders)`, failed ? 'error' : run.status === 'cancelled' ? 'info' : 'success', {
          action: { label: 'View', run: () => this.openJob(info.job_id) },
        });
      if (this.prefs.notifications) notify(title, `${folder}\n${info.destination || ''}`);
      return;
    }
    if (info.status === 'completed' && job && !job.filters?.job_type) this.#autoReceipt(job);
    if (info.dry_run && job && this.#continueAfterPreview(job, info.status === 'completed')) return;
    const folder = this.presetForJob(job)?.name || info.name;
    const title =
      info.status === 'completed' ? `${kind} finished` : info.status === 'failed' ? `${kind} failed` : `${kind} cancelled`;
    if (info.status === 'completed') this.#announceSuccess(kind, folder, info.job_id);
    else
      this.toast(`${title}: ${folder}`, info.status === 'failed' ? 'error' : 'info', {
        action: { label: 'View', run: () => this.openJob(info.job_id) },
      });
    if (this.prefs.notifications) notify(title, `${folder}\n${info.destination || ''}`);
  }

  #successes = [];
  #successTimer = null;
  /** "Sync finished" messages close together (Upload all, several auto-uploads) become one. */
  #announceSuccess(kind, folder, jobId) {
    this.#successes.push({ kind, folder, jobId });
    clearTimeout(this.#successTimer);
    this.#successTimer = setTimeout(() => {
      const done = this.#successes;
      this.#successes = [];
      if (done.length === 1) {
        this.toast(`${done[0].kind} finished: ${done[0].folder}`, 'success', { action: { label: 'View', run: () => this.openJob(done[0].jobId) } });
      } else {
        const names = done.slice(0, 3).map((d) => d.folder).join(', ') + (done.length > 3 ? ` and ${done.length - 3} more` : '');
        this.toast(`Finished ${done.length} folders: ${names}`, 'success', { action: { label: 'Activity', run: () => (this.view = 'activity') } });
      }
    }, 2500);
  }

  /** Upload when this preview finishes (or don't). */
  setUploadAfterPreview(preset, on) {
    const { [preset.preset_id]: _, ...rest } = this.uploadAfterPreview;
    this.uploadAfterPreview = on ? { ...rest, [preset.preset_id]: true } : rest;
  }

  /** A preview of a folder marked "upload when done" finished: start the upload. Returns true
   * when it took over (no separate "Preview finished" message). */
  #continueAfterPreview(job, ok) {
    const preset = this.presetForJob(job);
    if (!preset || !this.uploadAfterPreview[preset.preset_id]) return false;
    this.setUploadAfterPreview(preset, false);
    if (!ok) {
      this.toast(`The preview of ${preset.name} didn't finish, so the upload wasn't started.`, 'error', {
        action: { label: 'View', run: () => this.openJob(job.job_id) },
        timeout: 12000,
      });
      return true;
    }
    this.toast(`Preview of ${preset.name} finished. Uploading now…`, 'success', { timeout: 4000 });
    this.runPreset(preset, false);
    return true;
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

  /** Keep a short most-recent-first list in prefs (recent buckets, scanned folders). */
  remember(key, value, limit = 6) {
    const list = [value, ...(this.prefs[key] || []).filter((v) => v !== value)].slice(0, limit);
    this.savePrefs({ [key]: list });
  }

  toolAvailable(id) {
    return !!this.tools.find((t) => t.id === id)?.available;
  }

  // ----- folders (presets) ---------------------------------------------------

  presetForJob(job) {
    if (!job) return null;
    const group = job.filters?.group;
    if (group) {
      // A subfolder of a split upload belongs to the folder that was split.
      const dest = normalizeDestination(group.destination);
      return this.presets.find((p) => p.source_path === group.source_path && normalizeDestination(p.destination) === dest) || null;
    }
    const dest = jobDestination(job);
    return this.presets.find((p) => p.source_path === job.source_path && normalizeDestination(p.destination) === dest) || null;
  }

  jobsForPreset(preset) {
    return this.jobList.filter((j) => this.presetForJob(j)?.preset_id === preset.preset_id);
  }

  async cancelGroup(groupId) {
    try {
      await api.cancelGroup(groupId);
    } catch (err) {
      this.toast(`Couldn't cancel: ${err.message}`, 'error');
    }
  }

  async retryFailedInGroup(groupId) {
    try {
      const result = await api.retryGroup(groupId);
      this.toast(`Running ${result.retried} ${result.retried === 1 ? 'folder' : 'folders'} again`, 'info', { timeout: 3000 });
    } catch (err) {
      this.toast(`Couldn't retry: ${err.message}`, 'error');
    }
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
      upload_tool: tool,
      no_clobber: !!o.no_clobber,
      exclude_patterns: lines(o.exclude_patterns),
      exclude_folders: commaList(o.exclude_folders),
      split_by_folder: !!o.split_by_folder,
      threads: 4,
    };
  }

  /** Sync (or preview) a folder. thenUpload: a preview that starts the upload when it finishes. */
  async runPreset(preset, dryRun, { thenUpload = false, reason = null, quiet = false } = {}) {
    const id = preset.preset_id;
    if (id && this.starting[id]) return null; // already starting
    if (dryRun && thenUpload) this.setUploadAfterPreview(preset, true);
    ensureNotificationPermission();
    if (id) this.starting = { ...this.starting, [id]: dryRun ? 'preview' : 'sync' };
    const done = () => {
      if (!id) return;
      const { [id]: _, ...rest } = this.starting;
      this.starting = rest;
    };
    if (!quiet) this.toast(reason === 'auto' ? `New files in ${preset.name}: uploading them…` : `${dryRun ? 'Starting a preview of' : 'Starting to sync'} ${preset.name}…`, 'info', { timeout: 3000 });
    try {
      const result = await api.createUpload(this.uploadPayload(preset, dryRun));
      if (result?.already_running && !quiet) {
        if (!result.dry_run) this.setUploadAfterPreview(preset, false); // it's uploading already
        const what = result.dry_run ? 'being previewed' : 'syncing';
        this.toast(`${preset.name} is already ${what}, so nothing new was started. Its progress is on the card.`, 'info', { timeout: 6000 });
      }
      // Show the new job before the buttons come back.
      await this.refreshJobs().catch(() => {});
      return result;
    } catch (err) {
      if (dryRun) this.setUploadAfterPreview(preset, false);
      this.toast(`Couldn't start ${preset.name}: ${err.message}`, 'error', { timeout: 10000 });
      return null;
    } finally {
      done();
    }
  }

  /** Add several folders: { parent } (pick from its subfolders) or { paths }. */
  addFolders(dialog = {}) {
    this.view = 'folders';
    this.addFoldersDialog = { ...dialog };
  }

  addFolder(path = '') {
    this.view = 'folders';
    this.folderDialog = { preset: this.newPresetFor(path), previousName: null };
  }

  compareFolder(preset) {
    this.compareTarget = preset;
    this.view = 'compare';
  }

  /** The synced folder whose cloud copy contains gs://bucket/prefix, if any. */
  presetForCloudPath(bucket, prefix) {
    const here = normalizeDestination(`${bucket}/${prefix}`);
    return this.presets.find((p) => {
      const dest = normalizeDestination(p.destination);
      return here === dest || here.startsWith(`${dest}/`);
    }) || null;
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
