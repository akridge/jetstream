<script>
  import X from '@lucide/svelte/icons/x';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import Square from '@lucide/svelte/icons/square';
  import CloudUpload from '@lucide/svelte/icons/cloud-upload';
  import Copy from '@lucide/svelte/icons/copy';
  import FileText from '@lucide/svelte/icons/file-text';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import StatusIcon from './StatusIcon.svelte';
  import ProgressBar from './ProgressBar.svelte';
  import { app, ACTIVE, TERMINAL } from '../lib/state.svelte.js';
  import { api, pollActivity } from '../lib/api.js';
  import { basename, bytes, count, dateTime, duration } from '../lib/format.js';
  import { isDesktop, revealPath } from '../lib/platform.js';

  let { job } = $props();

  let detail = $state(null);
  let busy = $state(false);
  let folder = $derived(app.presetForJob(job));
  let isCloudOp = $derived(job.transfer_direction === 'cloud_to_cloud');
  let speed = $derived(job.status === 'running' ? app.speedOf(job.job_id) : job.transfer_speed || 0);

  const STATUS_TEXT = {
    running: 'Running',
    queued: 'Queued',
    pending: 'Starting',
    scheduled: 'Scheduled',
    completed: 'Completed',
    failed: 'Failed',
    cancelled: 'Cancelled',
  };

  let activity = $state(null);
  let isRunning = $derived(job.status === 'running');

  // While running, poll which files are moving right now.
  $effect(() => {
    if (!isRunning || isCloudOp) return;
    activity = null;
    return pollActivity(job.job_id, (data) => (activity = data));
  });

  // Reload full details (including output) when the job or its status changes.
  $effect(() => {
    const id = job.job_id;
    void job.status;
    api.job(id).then((d) => {
      if (d.job_id === job.job_id) detail = d;
    }).catch(() => {});
  });

  async function act(fn, success) {
    busy = true;
    try {
      await fn();
      if (success) app.toast(success, 'success', { timeout: 3000 });
    } catch (err) {
      app.toast(err.message, 'error', { timeout: 8000 });
    } finally {
      busy = false;
    }
  }

  function copyOutput() {
    navigator.clipboard?.writeText(detail?.upload_output || '');
    app.toast('Output copied', 'info', { timeout: 2000 });
  }

  async function remove() {
    if (!confirm('Remove this job from the history?')) return;
    await act(() => api.deleteJob(job.job_id));
    app.selectedJobId = null;
  }
</script>

<aside class="drawer card">
  <header>
    <div class="title">
      <StatusIcon {job} size={20} />
      <div class="min0">
        <h2 class="truncate">{folder?.name || basename(job.source_path)}</h2>
        <div class="sub">{job.dry_run ? 'Preview (dry run)' : STATUS_TEXT[job.status] || job.status} · {job.friendly_name || job.job_id}</div>
      </div>
    </div>
    <button class="btn ghost icon" aria-label="Close details" onclick={() => (app.selectedJobId = null)}><X size={18} /></button>
  </header>

  <div class="body">
    {#if job.status === 'running'}
      <div class="progress">
        <div class="pct">{Math.round(job.progress_percent || 0)}%</div>
        <ProgressBar value={job.progress_percent || 0} />
        <div class="muted">
          {bytes(job.bytes_uploaded)} of {bytes(job.total_size_bytes)} · {count(job.files_uploaded)} of {count(job.total_files)} files
          {#if speed > 0}· {bytes(speed)}/s{/if}
        </div>
      </div>
    {/if}

    {#if isRunning && activity && (activity.in_flight.length || activity.recent.length)}
      <section class="files" aria-live="polite">
        {#if activity.in_flight.length}
          <div class="label">Uploading now</div>
          <ul>
            {#each activity.in_flight.slice(0, 6) as f (f.path)}
              <li><span class="mono truncate start" title={f.path}><bdi>{f.path}</bdi></span><span class="muted">{f.bytes ? bytes(f.bytes) : ''}{f.seconds >= 5 ? ` · ${duration(f.seconds)}` : ''}</span></li>
            {/each}
          </ul>
        {/if}
        {#if activity.recent.length}
          <div class="label">Recently finished{activity.failed ? ` · ${count(activity.failed)} failed` : ''}</div>
          <ul>
            {#each activity.recent.slice(0, 8) as f, i (f.path + i)}
              <li class:failed={f.error}>
                <span class="mono truncate start" title={f.error ? `${f.path}: ${f.error}` : f.path}><bdi>{f.path}</bdi></span>
                <span class="muted">{f.error ? 'failed' : f.bytes ? bytes(f.bytes) : 'done'}</span>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {/if}

    {#if job.status === 'failed' && job.error_message}
      <div class="callout danger">{job.error_message}</div>
    {:else if job.status === 'completed' && job.dry_run}
      <div class="callout accent">
        This was a preview — nothing was uploaded. Check the output below, then upload for real.
      </div>
    {/if}

    <dl>
      <dt>From</dt>
      <dd class="mono">{job.source_path}</dd>
      <dt>To</dt>
      <dd class="mono">gs://{job.destination_bucket}/{job.destination_path || ''}</dd>
      <dt>Files</dt>
      <dd>{count(job.total_files)} · {bytes(job.total_size_bytes)}</dd>
      <dt>Tool</dt>
      <dd>{{ gcloud: 'gcloud storage rsync', jetstream: 'JetStream engine', gsutil: 'gsutil rsync', 'gcs-sdk': 'Cloud Storage API' }[job.upload_tool] || job.upload_tool}</dd>
      <dt>Started</dt>
      <dd>{dateTime(job.started_at || job.created_at)}</dd>
      {#if job.completed_at}
        <dt>Finished</dt>
        <dd>{dateTime(job.completed_at)}{job.duration_seconds ? ` · ${duration(job.duration_seconds)}` : ''}</dd>
      {/if}
      {#if speed > 0 && TERMINAL.has(job.status)}
        <dt>Average</dt>
        <dd>{bytes(speed)}/s</dd>
      {/if}
      {#if job.retry_count}
        <dt>Retries</dt>
        <dd>{job.retry_count} of {job.max_auto_retries}</dd>
      {/if}
    </dl>

    <div class="actions">
      {#if !isCloudOp && ACTIVE.has(job.status)}
        <button class="btn" disabled={busy} onclick={() => act(() => api.cancelJob(job.job_id), 'Cancelled')}><Square size={14} /> Cancel</button>
      {/if}
      {#if !isCloudOp && job.status === 'completed' && job.dry_run}
        <button class="btn primary" disabled={busy} onclick={() => act(() => api.retryJob(job.job_id, true), 'Upload started')}>
          <CloudUpload size={16} /> Upload for real
        </button>
      {/if}
      {#if !isCloudOp && TERMINAL.has(job.status)}
        <button class="btn" disabled={busy} onclick={() => act(() => api.retryJob(job.job_id, false), 'Started again')}>
          <RefreshCw size={15} /> {job.dry_run ? 'Preview again' : 'Run again'}
        </button>
      {/if}
      {#if isDesktop && detail?.log_path}
        <button class="btn ghost" onclick={() => revealPath(detail.log_path)}><FileText size={15} /> Show log file</button>
      {/if}
      {#if !isCloudOp && TERMINAL.has(job.status)}
        <button class="btn ghost danger" disabled={busy} onclick={remove}><Trash2 size={15} /> Remove</button>
      {/if}
    </div>

    <div class="output-head">
      <span class="label">Output</span>
      {#if detail?.upload_output}
        <button class="btn ghost sm" onclick={copyOutput}><Copy size={14} /> Copy</button>
      {/if}
    </div>
    <pre class="output">{detail?.upload_output || (ACTIVE.has(job.status) ? 'Output appears when the job finishes.' : 'No output.')}</pre>
  </div>
</aside>

<style>
  .drawer {
    width: 440px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow: hidden;
  }
  header {
    display: flex;
    justify-content: space-between;
    gap: 10px;
    padding: 16px 16px 12px;
    border-bottom: 1px solid var(--border);
  }
  .title {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    min-width: 0;
  }
  .min0 {
    min-width: 0;
  }
  h2 {
    font-size: 16px;
  }
  .sub,
  .muted {
    color: var(--muted);
    font-size: 12.5px;
  }
  .body {
    padding: 16px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .progress {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .pct {
    font-size: 28px;
    font-weight: 700;
    letter-spacing: -0.02em;
  }
  .files {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .files ul {
    list-style: none;
    margin: 0 0 6px;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .files li {
    display: flex;
    justify-content: space-between;
    gap: 10px;
    font-size: 12.5px;
  }
  .files li .mono {
    min-width: 0;
  }
  .files li .muted {
    flex-shrink: 0;
  }
  .files li.failed .mono,
  .files li.failed .muted {
    color: var(--danger);
  }
  .callout {
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    font-size: 13px;
  }
  .callout.danger {
    background: var(--danger-soft);
    color: var(--danger);
  }
  .callout.accent {
    background: var(--accent-soft);
    color: var(--accent);
  }
  dl {
    display: grid;
    grid-template-columns: 76px 1fr;
    gap: 7px 12px;
    margin: 0;
  }
  dt {
    color: var(--muted);
    font-size: 12.5px;
  }
  dd {
    margin: 0;
    word-break: break-all;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .output-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .output {
    margin: 0;
    padding: 12px;
    border-radius: var(--radius-sm);
    background: var(--surface-2);
    font-family: var(--mono);
    font-size: 12px;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 340px;
    overflow: auto;
  }
</style>
