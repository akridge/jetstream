<script>
  import Folder from '@lucide/svelte/icons/folder';
  import Cloud from '@lucide/svelte/icons/cloud';
  import Play from '@lucide/svelte/icons/play';
  import Eye from '@lucide/svelte/icons/eye';
  import Ellipsis from '@lucide/svelte/icons/ellipsis';
  import Square from '@lucide/svelte/icons/square';
  import CloudUpload from '@lucide/svelte/icons/cloud-upload';
  import Clock from '@lucide/svelte/icons/clock';
  import Pause from '@lucide/svelte/icons/pause';
  import GitCompare from '@lucide/svelte/icons/git-compare-arrows';
  import StatusIcon from './StatusIcon.svelte';
  import ProgressBar from './ProgressBar.svelte';
  import { app, ACTIVE, TERMINAL } from '../lib/state.svelte.js';
  import { api, pollActivity } from '../lib/api.js';
  import { bytes, count, duration, relativeTime, normalizeDestination, scheduleText, upcoming } from '../lib/format.js';
  import { isDesktop, revealPath } from '../lib/platform.js';

  let { preset } = $props();

  let jobs = $derived(app.jobsForPreset(preset));
  let current = $derived(jobs.find((j) => ACTIVE.has(j.status)));
  let lastFinished = $derived(jobs.find((j) => TERMINAL.has(j.status)));
  let lastReal = $derived(jobs.find((j) => TERMINAL.has(j.status) && !j.dry_run));
  let previewReady = $derived(
    lastFinished && lastFinished.dry_run && lastFinished.status === 'completed' && (!lastReal || lastReal.created_at < lastFinished.created_at),
  );
  let tool = $derived(preset.options?.upload_tool === 'jetstream' && app.toolAvailable('jetstream') ? 'JetStream engine' : 'gcloud');
  let speed = $derived(current ? app.speedOf(current.job_id) : 0);
  let remaining = $derived(
    current && speed > 0 ? Math.max(0, (current.total_size_bytes - current.bytes_uploaded) / speed) : null,
  );

  let nowFile = $state('');
  let paused = $state(false);
  let runningId = $derived(current?.status === 'running' ? current.job_id : null);
  $effect(() => {
    const id = runningId;
    nowFile = '';
    paused = false;
    if (!id) return;
    return pollActivity(id, (a) => {
      nowFile = a.in_flight[0]?.path || a.recent[0]?.path || '';
      paused = !!a.paused;
    }, 2500);
  });
  let schedule = $derived(preset.options?.schedule);
  let scheduled = $derived(schedule && schedule.mode && schedule.mode !== 'off');

  let menuOpen = $state(false);
  let busy = $state(false);

  async function run(dryRun) {
    busy = true;
    await app.runPreset(preset, dryRun);
    busy = false;
  }

  function openCloud() {
    menuOpen = false;
    const [bucket, ...rest] = normalizeDestination(preset.destination).split('/');
    app.cloudLocation = { bucket, prefix: rest.length ? `${rest.join('/')}/` : '' };
    app.view = 'cloud';
  }

  async function togglePause() {
    try {
      await (paused ? api.resumeJob(current.job_id) : api.pauseJob(current.job_id));
      paused = !paused;
    } catch (err) {
      app.toast(err.message, 'error');
    }
  }

  async function cancel() {
    try {
      await api.cancelJob(current.job_id);
    } catch (err) {
      app.toast(`Couldn't cancel: ${err.message}`, 'error');
    }
  }

  async function remove() {
    menuOpen = false;
    if (!confirm(`Remove "${preset.name}" from synced folders? Files already uploaded stay in the cloud.`)) return;
    await app.removePreset(preset);
  }
</script>

<svelte:window onclick={() => (menuOpen = false)} />

<article class="card folder" class:running={current}>
  <header>
    <div class="icon"><Folder size={20} /></div>
    <div class="titles">
      <h3 class="truncate" title={preset.name}>{preset.name}</h3>
      <div class="path mono truncate start" title={preset.source_path}><bdi>{preset.source_path}</bdi></div>
    </div>
    <div class="menu-wrap">
      <button class="btn ghost sm icon" aria-label="More actions" onclick={(e) => { e.stopPropagation(); menuOpen = !menuOpen; }}>
        <Ellipsis size={18} />
      </button>
      {#if menuOpen}
        <div class="menu card" role="menu">
          <button role="menuitem" onclick={() => app.editFolder(preset)}>Edit…</button>
          <button role="menuitem" onclick={() => app.compareFolder(preset)}>Compare with cloud</button>
          <button role="menuitem" onclick={() => openCloud()}>Open in Cloud storage</button>
          {#if isDesktop}
            <button role="menuitem" onclick={() => revealPath(preset.source_path)}>Show folder</button>
          {/if}
          {#if jobs.length}
            <button role="menuitem" onclick={() => app.openJob(jobs[0].job_id)}>Latest activity</button>
          {/if}
          <button role="menuitem" class="danger" onclick={remove}>Remove…</button>
        </div>
      {/if}
    </div>
  </header>

  <div class="dest">
    <Cloud size={15} />
    <span class="mono truncate start" title={`gs://${normalizeDestination(preset.destination)}/`}><bdi>gs://{normalizeDestination(preset.destination)}/</bdi></span>
    <span class="pill">{tool}</span>
  </div>

  <div class="status">
    {#if current}
      {#if current.status === 'running'}
        <div class="row">
          <StatusIcon job={current} size={16} />
          <strong>{paused ? 'Paused at' : current.dry_run ? 'Previewing…' : 'Syncing…'} {Math.round(current.progress_percent || 0)}%</strong>
          <span class="muted">
            {bytes(current.bytes_uploaded)} of {bytes(current.total_size_bytes)}
            {#if speed > 0 && !paused}· {bytes(speed)}/s{/if}
            {#if remaining !== null && !paused}· {duration(remaining)} left{/if}
          </span>
        </div>
        <ProgressBar value={current.progress_percent || 0} indeterminate={!current.total_size_bytes} />
        {#if nowFile}<div class="now mono truncate start" title={nowFile}><bdi>{nowFile}</bdi></div>{/if}
      {:else}
        <div class="row">
          <StatusIcon job={current} size={16} />
          <strong>{current.status === 'scheduled' ? 'Scheduled' : 'Waiting to start…'}</strong>
        </div>
        <ProgressBar indeterminate />
      {/if}
    {:else if previewReady}
      <div class="row">
        <StatusIcon job={lastFinished} size={16} />
        <strong>Preview ready</strong>
        <span class="muted">{relativeTime(lastFinished.completed_at)} · check what will upload, then upload</span>
      </div>
    {:else if lastFinished?.status === 'failed'}
      <div class="row">
        <StatusIcon job={lastFinished} size={16} />
        <strong class="danger">Last sync failed</strong>
        <span class="muted">{relativeTime(lastFinished.completed_at)}</span>
      </div>
      <div class="error truncate" title={lastFinished.error_message}>{lastFinished.error_message || 'See activity for details'}</div>
    {:else if lastReal?.status === 'completed'}
      <div class="row">
        <StatusIcon job={lastReal} size={16} />
        <strong>Synced {relativeTime(lastReal.completed_at)}</strong>
        <span class="muted">{count(lastReal.total_files)} files · {bytes(lastReal.total_size_bytes)}{lastReal.duration_seconds ? ` · took ${duration(lastReal.duration_seconds)}` : ''}</span>
      </div>
    {:else if lastFinished}
      <div class="row">
        <StatusIcon job={lastFinished} size={16} />
        <strong>{lastFinished.status === 'cancelled' ? 'Last sync cancelled' : 'Preview finished'}</strong>
        <span class="muted">{relativeTime(lastFinished.completed_at)}</span>
      </div>
    {:else}
      <div class="row muted">Not synced yet — preview first to see what will upload.</div>
    {/if}
  </div>

  {#if scheduled}
    <div class="schedule" title={schedule.preview ? 'Scheduled runs are previews only' : ''}>
      <Clock size={14} />
      <span>{scheduleText(schedule)}{schedule.preview ? ' · preview only' : ''}</span>
      {#if preset.next_run_at}<span class="muted">· next {upcoming(preset.next_run_at)}</span>{/if}
    </div>
  {/if}

  <footer>
    {#if current}
      {#if current.status === 'running' && current.upload_tool === 'jetstream' && !current.dry_run}
        <button class="btn sm" onclick={togglePause}>{#if paused}<Play size={14} /> Resume{:else}<Pause size={14} /> Pause{/if}</button>
      {/if}
      <button class="btn sm" onclick={cancel}><Square size={14} /> Cancel</button>
      <button class="btn ghost sm" onclick={() => app.openJob(current.job_id)}>Details</button>
    {:else if previewReady}
      <button class="btn primary sm" disabled={busy} onclick={() => run(false)}><CloudUpload size={15} /> Upload now</button>
      <button class="btn ghost sm" onclick={() => app.openJob(lastFinished.job_id)}>Review preview</button>
    {:else}
      <button class="btn primary sm" disabled={busy} onclick={() => run(false)}><Play size={14} /> Sync now</button>
      <button class="btn sm" disabled={busy} onclick={() => run(true)}><Eye size={15} /> Preview</button>
      <button class="btn ghost sm" title="What's on this computer vs in the cloud" onclick={() => app.compareFolder(preset)}><GitCompare size={14} /> Compare</button>
      {#if lastFinished?.status === 'failed'}
        <button class="btn ghost sm" onclick={() => app.openJob(lastFinished.job_id)}>View error</button>
      {/if}
    {/if}
  </footer>
</article>

<style>
  .folder {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 16px;
    transition: border-color 0.15s, box-shadow 0.15s;
  }
  .folder:hover {
    border-color: var(--border-strong);
  }
  .folder.running {
    border-color: color-mix(in srgb, var(--accent) 45%, var(--border));
  }
  header {
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }
  .icon {
    width: 38px;
    height: 38px;
    border-radius: 9px;
    background: var(--accent-soft);
    color: var(--accent);
    display: grid;
    place-items: center;
    flex-shrink: 0;
  }
  .titles {
    flex: 1;
    min-width: 0;
  }
  h3 {
    font-size: 15px;
  }
  .path {
    color: var(--muted);
    margin-top: 2px;
  }
  .menu-wrap {
    position: relative;
  }
  .menu {
    position: absolute;
    right: 0;
    top: 32px;
    z-index: 20;
    min-width: 170px;
    padding: 5px;
    display: flex;
    flex-direction: column;
    box-shadow: var(--shadow-lg);
  }
  .menu button {
    border: none;
    background: none;
    text-align: left;
    padding: 7px 10px;
    border-radius: 6px;
    cursor: pointer;
  }
  .menu button:hover {
    background: var(--surface-2);
  }
  .menu .danger {
    color: var(--danger);
  }
  .dest {
    display: flex;
    align-items: center;
    gap: 7px;
    color: var(--text-2);
    min-width: 0;
  }
  .dest .mono {
    flex: 1;
    min-width: 0;
  }
  .status {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-height: 44px;
    padding: 10px 12px;
    border-radius: 8px;
    background: var(--surface-2);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 7px;
    flex-wrap: wrap;
  }
  .muted {
    color: var(--muted);
    font-size: 13px;
  }
  .danger {
    color: var(--danger);
  }
  .error {
    color: var(--danger);
    font-size: 12.5px;
  }
  .now {
    color: var(--muted);
    font-size: 12px;
  }
  .schedule {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-2);
    font-size: 13px;
    margin-top: -4px;
  }
  footer {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
</style>
