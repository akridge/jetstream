<script>
  import StatusIcon from './StatusIcon.svelte';
  import ProgressBar from './ProgressBar.svelte';
  import { app } from '../lib/state.svelte.js';
  import { basename, bytes, count, relativeTime } from '../lib/format.js';

  let { job, selected = false, onselect } = $props();

  let folder = $derived(app.presetForJob(job));
  let title = $derived(
    job.job_type === 'cloud_sync' || job.job_type === 'cloud_move'
      ? `${job.job_type === 'cloud_move' ? 'Move' : 'Cloud sync'} · ${basename(job.source_path)}`
      : folder?.name || basename(job.source_path),
  );
  let kind = $derived(job.dry_run ? 'Preview' : job.job_type === 'cloud_sync' ? 'Cloud sync' : job.job_type === 'cloud_move' ? 'Move' : 'Upload');
</script>

<button class="row" class:selected onclick={() => onselect(job.job_id)}>
  <div class="status"><StatusIcon {job} /></div>
  <div class="main">
    <div class="top">
      <span class="title truncate">{title}</span>
      <span class="pill" class:accent={job.dry_run}>{kind}</span>
      {#if job.upload_tool === 'jetstream'}<span class="pill">engine</span>{/if}
    </div>
    <div class="detail truncate">
      {#if job.status === 'running'}
        {Math.round(job.progress_percent || 0)}% · {bytes(job.bytes_uploaded)} of {bytes(job.total_size_bytes)}
      {:else if job.status === 'queued' || job.status === 'pending'}
        Waiting to start
      {:else if job.status === 'scheduled'}
        Scheduled {job.scheduled_for ? new Date(job.scheduled_for).toLocaleString() : ''}
      {:else if job.status === 'failed'}
        <span class="bad">{job.error_message || 'Failed'}</span>
      {:else if job.status === 'cancelled'}
        Cancelled
      {:else}
        {count(job.total_files)} files · {bytes(job.total_size_bytes)} → gs://{job.destination_bucket}/{job.destination_path || ''}
      {/if}
    </div>
    {#if job.status === 'running'}
      <ProgressBar value={job.progress_percent || 0} />
    {/if}
  </div>
  <div class="when">{relativeTime(job.completed_at || job.started_at || job.created_at)}</div>
</button>

<style>
  .row {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    width: 100%;
    padding: 12px 14px;
    border: none;
    border-bottom: 1px solid var(--border);
    background: transparent;
    text-align: left;
    cursor: pointer;
    color: var(--text);
  }
  .row:hover {
    background: var(--surface-hover);
  }
  .row.selected {
    background: var(--accent-soft);
  }
  .status {
    padding-top: 2px;
  }
  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 7px;
    min-width: 0;
  }
  .title {
    font-weight: 600;
    min-width: 0;
  }
  .pill {
    height: 19px;
    font-size: 11px;
  }
  .detail {
    color: var(--muted);
    font-size: 12.5px;
  }
  .bad {
    color: var(--danger);
  }
  .when {
    color: var(--muted);
    font-size: 12px;
    white-space: nowrap;
    padding-top: 2px;
  }
</style>
