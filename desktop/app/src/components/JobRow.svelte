<script>
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import StatusIcon from './StatusIcon.svelte';
  import ProgressBar from './ProgressBar.svelte';
  import { app } from '../lib/state.svelte.js';
  import { basename, bytes, count, relativeTime } from '../lib/format.js';

  /** job: a job, or a split-upload run (with `group`); child: a subfolder row inside its run. */
  let { job, selected = false, onselect, expanded = false, ontoggle = null, child = false } = $props();

  let folder = $derived(app.presetForJob(job));
  let title = $derived(
    job.job_type === 'cloud_sync' || job.job_type === 'cloud_move'
      ? `${job.job_type === 'cloud_move' ? 'Move' : 'Cloud sync'} · ${basename(job.source_path)}`
      : child
        ? job.filters?.group_part
          ? `${job.filters.group_part}/`
          : 'Files in the main folder'
        : folder?.name || job.group?.name || basename(job.source_path),
  );
  let kind = $derived(job.dry_run ? 'Preview' : job.job_type === 'cloud_sync' ? 'Cloud sync' : job.job_type === 'cloud_move' ? 'Move' : 'Upload');
</script>

<button class="row" class:selected class:child onclick={() => onselect(job.job_id)}>
  {#if ontoggle}
    <span
      class="toggle"
      class:open={expanded}
      role="button"
      tabindex="-1"
      aria-label={expanded ? 'Hide subfolders' : 'Show subfolders'}
      onclick={(e) => { e.stopPropagation(); ontoggle(); }}
      onkeydown={() => {}}
    ><ChevronRight size={15} /></span>
  {/if}
  <div class="status"><StatusIcon {job} /></div>
  <div class="main">
    <div class="top">
      <span class="title truncate">{title}</span>
      <span class="pill" class:accent={job.dry_run}>{kind}</span>
      {#if job.upload_tool === 'jetstream'}<span class="pill">engine</span>{/if}
      {#if job.group}<span class="pill" title="Uploaded as one job per subfolder">{job.group.parts} folders</span>{/if}
      {#if job.filters?.trigger === 'schedule'}<span class="pill" title="Started by the folder's schedule">scheduled</span>{/if}
    </div>
    <div class="detail truncate">
      {#if job.group && job.group.active}
        {Math.round(job.progress_percent || 0)}% · {job.group.done} of {job.group.parts} folders done{job.group.failed ? ` · ${job.group.failed} failed` : ''}
      {:else if job.status === 'running'}
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
    {#if job.status === 'running' || (job.group && job.group.active)}
      <ProgressBar value={job.progress_percent || 0} />
    {/if}
  </div>
  <div class="when">{relativeTime(job.completed_at || job.started_at || job.created_at)}</div>
</button>

<style>
  .toggle {
    display: grid;
    place-items: center;
    width: 18px;
    margin-top: 1px;
    color: var(--muted);
    border-radius: 4px;
    transition: transform 0.12s;
  }
  .toggle:hover {
    color: var(--text);
    background: var(--surface-2);
  }
  .toggle.open {
    transform: rotate(90deg);
  }
  .row.child {
    padding-left: 44px;
    background: color-mix(in srgb, var(--surface-2) 50%, transparent);
  }
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
