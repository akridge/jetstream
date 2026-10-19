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
  cloudTransfer: (body) => request('POST', '/api/cloud/transfer', body),
  cloudMove: (body) => request('POST', '/api/cloud/move', body),
  stats: () => request('GET', '/api/stats/'),
  pauseJob: (id) => request('POST', `/api/uploads/${encodeURIComponent(id)}/pause`),
  resumeJob: (id) => request('POST', `/api/uploads/${encodeURIComponent(id)}/resume`),
  transferSettings: () => request('GET', '/api/settings/transfer'),
  saveTransferSettings: (body) => request('PUT', '/api/settings/transfer', body),
  jobActivity: (id) => request('GET', `/api/uploads/${encodeURIComponent(id)}/activity`),
  auditDefaults: () => request('GET', '/api/cloud-audit/defaults'),
  importAudit: (body) => request('POST', '/api/cloud-audit/runs/import', body),
  quarantine: (runId, body) => request('POST', `/api/cloud-audit/runs/${encodeURIComponent(runId)}/quarantine`, body),
  uploadTrends: (days = 30) => request('GET', `/api/analytics/upload-trends?days=${days}`),
  successRate: () => request('GET', '/api/analytics/success-rate'),
  performance: () => request('GET', '/api/analytics/performance-metrics'),
  topDestinations: (limit = 6) => request('GET', `/api/analytics/top-destinations?limit=${limit}`),
};

export function eventsUrl() {
  return `${base}/api/system/events`;
}

export function logUrl(id) {
  return `${base}/api/uploads/${encodeURIComponent(id)}/log`;
}

export function csvExportUrl() {
  return `${base}/api/analytics/export-csv`;
}

/** Poll a running job's per-file activity; returns a stop function. */
export function pollActivity(jobId, callback, intervalMs = 1500) {
  let stopped = false;
  let timer;
  const tick = async () => {
    try {
      const data = await api.jobActivity(jobId);
      if (!stopped) callback(data);
    } catch {
      /* transient; try again */
    }
    if (!stopped) timer = setTimeout(tick, intervalMs);
  };
  tick();
  return () => {
    stopped = true;
    clearTimeout(timer);
  };
}
