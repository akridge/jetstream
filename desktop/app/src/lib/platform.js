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
export async function listFolder(bucket, prefix, pageToken = null) {
  if (!isDesktop) throw desktopOnly();
  return invoke('list_folder', { bucket, prefix, pageToken });
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
