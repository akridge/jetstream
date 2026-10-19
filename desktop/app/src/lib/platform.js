// Desktop (Tauri) integrations, with browser fallbacks so the UI also runs in a normal
// browser during development (`npm run dev:ui`, then open http://localhost:5173/?api=http://127.0.0.1:8000).

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { isPermissionGranted, requestPermission, sendNotification } from '@tauri-apps/plugin-notification';
import { openUrl as tauriOpenUrl, revealItemInDir } from '@tauri-apps/plugin-opener';

export const isDesktop = typeof window !== 'undefined' && !!window.__TAURI_INTERNALS__;

function browserApiBase() {
  const fromQuery = new URLSearchParams(window.location.search).get('api');
  return (fromQuery || 'http://127.0.0.1:8000').replace(/\/+$/, '');
}

function normalize(status) {
  return {
    phase: status.phase,
    base: status.port ? `http://127.0.0.1:${status.port}` : null,
    message: status.message || null,
  };
}

/** Where the JetStream server is and whether it's ready. */
export async function getServerStatus() {
  if (!isDesktop) return { phase: 'ready', base: browserApiBase(), message: null };
  return normalize(await invoke('server_status'));
}

export async function onServerStatus(callback) {
  if (!isDesktop) return () => {};
  return listen('server-status', (event) => callback(normalize(event.payload)));
}

export async function serverLog() {
  return isDesktop ? invoke('server_log') : '';
}

/** Native folder picker; returns a path or null. */
export async function pickFolder(defaultPath, title = 'Choose a folder to sync') {
  if (!isDesktop) {
    return window.prompt('Folder path on this computer:', defaultPath || '') || null;
  }
  const selected = await openDialog({ directory: true, multiple: false, defaultPath: defaultPath || undefined, title });
  return Array.isArray(selected) ? selected[0] : selected;
}

/** Folders dropped onto the window: onDrop(paths), onHover(bool). */
export async function onFolderDrop(onDrop, onHover) {
  if (!isDesktop) return () => {};
  return getCurrentWebview().onDragDropEvent((event) => {
    const payload = event.payload;
    if (payload.type === 'enter' || payload.type === 'over') onHover(true);
    else if (payload.type === 'leave') onHover(false);
    else if (payload.type === 'drop') {
      onHover(false);
      if (payload.paths?.length) onDrop(payload.paths);
    }
  });
}

export async function ensureNotificationPermission() {
  try {
    if (isDesktop) {
      if (!(await isPermissionGranted())) await requestPermission();
    } else if ('Notification' in window && Notification.permission === 'default') {
      await Notification.requestPermission();
    }
  } catch {
    /* notifications are optional */
  }
}

export async function notify(title, body) {
  try {
    if (isDesktop) {
      if (await isPermissionGranted()) sendNotification({ title, body });
    } else if ('Notification' in window && Notification.permission === 'granted' && document.visibilityState !== 'visible') {
      new Notification(title, { body });
    }
  } catch {
    /* notifications are optional */
  }
}

export async function openUrl(url) {
  if (isDesktop) await tauriOpenUrl(url);
  else window.open(url, '_blank', 'noopener');
}

export async function revealPath(path) {
  if (isDesktop) await revealItemInDir(path);
}

/** Open one of the original JetStream pages (classic window in the desktop app). */
export async function openClassic(path, base) {
  if (isDesktop) await invoke('open_classic', { path });
  else window.open(`${base}/${path.replace(/^\//, '')}`, '_blank');
}

// ----- Rust core (in-process in the desktop app) -------------------------------------

function desktopOnly() {
  return new Error('This runs in the JetStream desktop app.');
}

/** One folder level of gs://bucket/prefix: { bucket, prefix, entries, next_page_token }. */
export async function listFolder(bucket, prefix, pageToken = null, includeFolders = false) {
  if (!isDesktop) throw desktopOnly();
  return invoke('list_folder', { bucket, prefix, pageToken, includeFolders });
}

/** Total objects/bytes under gs://bucket/prefix. */
export async function folderTotals(bucket, prefix) {
  if (!isDesktop) throw desktopOnly();
  return invoke('folder_totals', { bucket, prefix });
}

/** Analyze a local folder; onProgress({files, bytes}) while it runs. Cancel with cancelScan(scanId). */
export function startScan(path, onProgress) {
  if (!isDesktop) return { scanId: null, done: Promise.reject(desktopOnly()) };
  const scanId = `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  const done = (async () => {
    const unlisten = await listen('scan-progress', (event) => {
      if (event.payload.scan_id === scanId) onProgress?.(event.payload);
    });
    try {
      return await invoke('scan_folder', { path, scanId });
    } finally {
      unlisten();
    }
  })();
  return { scanId, done };
}

export async function cancelScan(scanId) {
  if (isDesktop && scanId) await invoke('cancel_scan', { scanId });
}

export async function defaultJunkPatterns() {
  if (!isDesktop) throw desktopOnly();
  return invoke('default_junk_patterns');
}

/** Find junk under gs://bucket/prefix; onProgress({scanned, found}). Cancel with cancelAudit(auditId). */
export function startAudit(bucket, prefix, patterns, onProgress) {
  if (!isDesktop) return { auditId: null, done: Promise.reject(desktopOnly()) };
  const auditId = `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  const done = (async () => {
    const unlisten = await listen('audit-progress', (event) => {
      if (event.payload.audit_id === auditId) onProgress?.(event.payload);
    });
    try {
      return await invoke('audit_bucket', { bucket, prefix, patterns, auditId });
    } finally {
      unlisten();
    }
  })();
  return { auditId, done };
}

export async function cancelAudit(auditId) {
  if (isDesktop && auditId) await invoke('cancel_audit', { auditId });
}

// ----- Cloud file manager (Rust core) ------------------------------------------------------

export const newOpId = () => `${Date.now()}-${Math.random().toString(36).slice(2)}`;

/** Bucket settings and permissions: { name, soft_delete_seconds, hierarchical_namespace, autoclass, known, permissions }. */
export async function bucketAccess(bucket) {
  if (!isDesktop) throw desktopOnly();
  return invoke('bucket_access', { bucket });
}

/**
 * Plan an operation without changing anything. request is one of
 *   { kind: 'copy' | 'move', bucket, paths, dst_bucket, dst_prefix }
 *   { kind: 'rename', bucket, path, new_name }
 *   { kind: 'delete', bucket, paths }
 * Folder paths end with "/". Returns the plan summary; run it with runCloudOp(opId).
 */
export async function planCloudOp(opId, request) {
  if (!isDesktop) throw desktopOnly();
  return invoke('plan_cloud_op', { opId, request });
}

async function withOpProgress(opId, onProgress, call) {
  const unlisten = await listen('cloud-op-progress', (event) => {
    if (event.payload.op_id === opId) onProgress?.(event.payload);
  });
  try {
    return await call();
  } finally {
    unlisten();
  }
}

/** Run a planned operation; onProgress({done, skipped, failed, bytes}). */
export function runCloudOp(opId, onProgress) {
  return withOpProgress(opId, onProgress, () => invoke('run_cloud_op', { opId }));
}

/** Reverse a finished operation; runs as a new operation newOpId. */
export function undoCloudOp(opId, newOpId, onProgress) {
  return withOpProgress(newOpId, onProgress, () => invoke('undo_cloud_op', { opId, newOpId }));
}

export async function cancelCloudOp(opId) {
  if (isDesktop && opId) await invoke('cancel_cloud_op', { opId });
}

/** objects: [{ name, generation }] from listDeleted. */
export function restoreObjects(opId, bucket, objects, onProgress) {
  if (!isDesktop) return Promise.reject(desktopOnly());
  return withOpProgress(opId, onProgress, () => invoke('restore_objects', { opId, bucket, objects }));
}

/** Soft-deleted objects under a folder: { objects, next_page_token }. */
export async function listDeleted(bucket, prefix, pageToken = null) {
  if (!isDesktop) throw desktopOnly();
  return invoke('list_deleted', { bucket, prefix, pageToken });
}

export async function newFolder(bucket, parent, name) {
  if (!isDesktop) throw desktopOnly();
  return invoke('new_folder', { bucket, parent, name });
}

/** Compare a local folder with gs://destination: same / different / local_only / cloud_only. */
export async function compareFolder(path, destination, { excludePatterns = [], excludeFolders = [], recursive = true } = {}) {
  if (!isDesktop) throw desktopOnly();
  return invoke('compare_folder', { path, destination, excludePatterns, excludeFolders, recursive });
}

/** Download files/folders (paths ending "/") into destDir; onProgress({total, done, ...}). */
export function downloadCloud(opId, bucket, paths, destDir, onProgress) {
  if (!isDesktop) return Promise.reject(desktopOnly());
  return withOpProgress(opId, onProgress, () => invoke('download_cloud', { opId, bucket, paths, destDir }));
}

/** Upload individual local files into gs://bucket/prefix/. */
export function uploadLocalFiles(opId, bucket, prefix, files, onProgress) {
  if (!isDesktop) return Promise.reject(desktopOnly());
  return withOpProgress(opId, onProgress, () => invoke('upload_local_files', { opId, bucket, prefix, files }));
}

/** Speed limit (Mbit/s, 0 = none) for downloads and file uploads run by the app. */
export async function setTransferLimit(maxMbps) {
  if (isDesktop) await invoke('set_transfer_limit', { maxMbps: Number(maxMbps) || 0 });
}

/** [[path, isFolder], ...] for dropped paths. */
export async function classifyPaths(paths) {
  if (!isDesktop) return paths.map((p) => [p, true]);
  return invoke('classify_paths', { paths });
}
