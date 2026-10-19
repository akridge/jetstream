// JetStream server HTTP API.

let base = '';

export function setApiBase(value) {
  base = value || '';
}

export function apiBase() {
  return base;
}

function errorText(data, fallback) {
  if (!data) return fallback;
  if (typeof data === 'string') return data;
  const detail = data.detail ?? data.message;
  if (typeof detail === 'string') return detail;
  if (Array.isArray(detail)) {
    return detail.map((d) => (d.msg || '').replace(/^Value error, /, '')).filter(Boolean).join('; ') || fallback;
  }
  return fallback;
}

async function request(method, path, body) {
  const response = await fetch(base + path, {
    method,
    headers: body ? { 'Content-Type': 'application/json' } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await response.text();
  let data = null;
  try {
    data = text ? JSON.parse(text) : null;
  } catch {
    data = text;
  }
  if (!response.ok) throw new Error(errorText(data, `${response.status} ${response.statusText}`));
  return data;
}

export const api = {
  version: () => request('GET', '/api/version'),
  jobs: (limit = 200) => request('GET', `/api/uploads/?limit=${limit}&include_cleared=true`),
  job: (id) => request('GET', `/api/uploads/${encodeURIComponent(id)}`),
  createUpload: (payload) => request('POST', '/api/uploads/', payload),
  cancelJob: (id) => request('POST', `/api/uploads/${encodeURIComponent(id)}/cancel`),
  retryJob: (id, removeDryRun) =>
    request('POST', `/api/uploads/${encodeURIComponent(id)}/retry${removeDryRun ? '?remove_dry_run=true' : ''}`),
  deleteJob: (id) => request('DELETE', `/api/uploads/${encodeURIComponent(id)}`),
  clearFinished: () => request('POST', '/api/uploads/clear-completed'),
  presets: () => request('GET', '/api/presets/'),
  savePreset: (preset) => request('POST', '/api/presets/', preset),
  deletePreset: (id) => request('DELETE', `/api/presets/${encodeURIComponent(id)}`),
  authStatus: (refresh = false) => request('GET', `/api/system/auth-status${refresh ? '?refresh=true' : ''}`),
  signIn: () => request('POST', '/api/system/auth-login'),
  uploadTools: () => request('GET', '/api/system/upload-tools'),
  analyzeFolder: (path) => request('POST', '/api/folders/analyze', { path, recursive: true }),
};

export function eventsUrl() {
  return `${base}/api/system/events`;
}

export function logUrl(id) {
  return `${base}/api/uploads/${encodeURIComponent(id)}/log`;
}
