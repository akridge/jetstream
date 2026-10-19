<script>
  import Search from '@lucide/svelte/icons/search';
  import Inbox from '@lucide/svelte/icons/inbox';
  import JobRow from '../components/JobRow.svelte';
  import JobDrawer from '../components/JobDrawer.svelte';
  import { app, ACTIVE } from '../lib/state.svelte.js';

  const FILTERS = [
    { id: 'all', label: 'All' },
    { id: 'active', label: 'In progress' },
    { id: 'failed', label: 'Failed' },
    { id: 'completed', label: 'Completed' },
    { id: 'previews', label: 'Previews' },
  ];

  let filter = $state('all');
  let query = $state('');

  let filtered = $derived(
    app.jobList.filter((job) => {
      if (filter === 'active' && !ACTIVE.has(job.status)) return false;
      if (filter === 'failed' && job.status !== 'failed') return false;
      if (filter === 'completed' && (job.status !== 'completed' || job.dry_run)) return false;
      if (filter === 'previews' && !job.dry_run) return false;
      if (query) {
        const q = query.toLowerCase();
        const name = app.presetForJob(job)?.name || '';
        const hay = `${name} ${job.source_path} ${job.destination_bucket}/${job.destination_path || ''} ${job.friendly_name || ''}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    }),
  );
  let selected = $derived(app.selectedJobId ? app.jobs[app.selectedJobId] : null);
  let counts = $derived({
    active: app.jobList.filter((j) => ACTIVE.has(j.status)).length,
    failed: app.jobList.filter((j) => j.status === 'failed').length,
  });
</script>

<section class="view">
  <header class="view-head">
    <div>
      <h1>Activity</h1>
      <p class="sub">Every upload, preview and cloud operation, updated live.</p>
    </div>
  </header>

  <div class="toolbar">
    <div class="segmented" role="tablist">
      {#each FILTERS as f (f.id)}
        <button role="tab" aria-selected={filter === f.id} class:active={filter === f.id} onclick={() => (filter = f.id)}>
          {f.label}
          {#if f.id === 'active' && counts.active}<span class="count">{counts.active}</span>{/if}
          {#if f.id === 'failed' && counts.failed}<span class="count danger">{counts.failed}</span>{/if}
        </button>
      {/each}
    </div>
    <label class="search">
      <Search size={15} />
      <input placeholder="Search folders, buckets…" bind:value={query} />
    </label>
  </div>

  <div class="split">
    <div class="list card">
      {#each filtered as job (job.job_id)}
        <JobRow {job} selected={app.selectedJobId === job.job_id} onselect={(id) => (app.selectedJobId = id)} />
      {:else}
        <div class="empty">
          <Inbox size={30} />
          <p>{app.jobList.length ? 'Nothing matches this filter.' : 'No activity yet. Sync a folder to get started.'}</p>
        </div>
      {/each}
    </div>
    {#if selected}
      {#key selected.job_id}
        <JobDrawer job={selected} />
      {/key}
    {/if}
  </div>
</section>

<style>
  .view {
    padding: 28px 32px 24px;
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .view-head {
    margin-bottom: 18px;
  }
  h1 {
    font-size: 24px;
  }
  .sub {
    margin: 4px 0 0;
    color: var(--muted);
  }
  .toolbar {
    display: flex;
    gap: 12px;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 14px;
    flex-wrap: wrap;
  }
  .segmented {
    display: inline-flex;
    padding: 3px;
    border-radius: 9px;
    background: var(--surface-2);
    border: 1px solid var(--border);
  }
  .segmented button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 12px;
    border: none;
    border-radius: 7px;
    background: transparent;
    color: var(--text-2);
    font-weight: 550;
    cursor: pointer;
  }
  .segmented button.active {
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow);
  }
  .count {
    min-width: 18px;
    height: 18px;
    padding: 0 5px;
    border-radius: 999px;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 11px;
    font-weight: 700;
    display: grid;
    place-items: center;
  }
  .count.danger {
    background: var(--danger-soft);
    color: var(--danger);
  }
  .search {
    display: flex;
    align-items: center;
    gap: 7px;
    height: 34px;
    padding: 0 10px;
    width: 280px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    background: var(--surface);
    color: var(--muted);
  }
  .search input {
    border: none;
    outline: none;
    background: transparent;
    flex: 1;
    min-width: 0;
  }
  .split {
    flex: 1;
    min-height: 0;
    display: flex;
    gap: 16px;
  }
  .list {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 60px 20px;
    color: var(--muted);
  }
</style>
