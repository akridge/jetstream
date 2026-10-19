<script>
  import ArrowLeft from '@lucide/svelte/icons/arrow-left';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import Play from '@lucide/svelte/icons/play';
  import Cloud from '@lucide/svelte/icons/cloud';
  import Folder from '@lucide/svelte/icons/folder';
  import CircleCheck from '@lucide/svelte/icons/circle-check';
  import Upload from '@lucide/svelte/icons/upload';
  import CloudOff from '@lucide/svelte/icons/cloud-off';
  import Laptop from '@lucide/svelte/icons/laptop';
  import CircleAlert from '@lucide/svelte/icons/circle-alert';
  import ShieldCheck from '@lucide/svelte/icons/shield-check';
  import { app, commaList, lines } from '../lib/state.svelte.js';
  import { bytes, count, dateTime, duration, normalizeDestination, relativeTime } from '../lib/format.js';
  import { compareFolder, isDesktop, revealPath } from '../lib/platform.js';

  let preset = $derived(app.compareTarget);
  let report = $state(null);
  let loading = $state(false);
  let error = $state('');
  let tab = $state('local_only');
  let folderFilter = $state(null);

  const GROUPS = [
    { key: 'same', label: 'In sync', hint: 'on both, same size', icon: CircleCheck, tone: 'good' },
    { key: 'local_only', label: 'Only on this computer', hint: 'uploads on the next sync', icon: Laptop, tone: 'accent' },
    { key: 'cloud_only', label: 'Only in the cloud', hint: 'not on this computer', icon: CloudOff, tone: 'neutral' },
    { key: 'different', label: 'Different', hint: 'changed on one side', icon: CircleAlert, tone: 'warn' },
  ];

  let destination = $derived(preset ? `gs://${normalizeDestination(preset.destination)}/` : '');
  let shown = $derived.by(() => {
    if (!report) return [];
    const list = report[tab].list;
    if (folderFilter === null) return list.slice(0, 500);
    return list.filter((d) => (d.path.includes('/') ? d.path.split('/')[0] : '') === folderFilter).slice(0, 500);
  });
  let inSync = $derived(report && !report.local_only.files && !report.different.files);

  async function run() {
    if (!preset) return;
    loading = true;
    error = '';
    try {
      const o = preset.options || {};
      report = await compareFolder(preset.source_path, destination, {
        excludePatterns: lines(o.exclude_patterns) || [],
        excludeFolders: commaList(o.exclude_folders) || [],
        recursive: o.recursive !== false,
      });
      // Open on the most useful list.
      tab = GROUPS.map((g) => g.key).find((k) => k !== 'same' && report[k].files) || 'same';
      folderFilter = null;
    } catch (err) {
      error = String(err?.message || err);
    } finally {
      loading = false;
    }
  }

  // Compare when opened (and when a sync of this folder finishes).
  let lastRun = '';
  $effect(() => {
    const key = preset?.preset_id;
    if (key && key !== lastRun) {
      lastRun = key;
      report = null;
      run();
    }
  });

  function openInCloud(path = '') {
    const [bucket, ...rest] = normalizeDestination(preset.destination).split('/');
    const folder = [...rest, ...path.split('/').slice(0, -1)].filter(Boolean).join('/');
    app.cloudLocation = { bucket, prefix: folder ? `${folder}/` : '' };
    app.view = 'cloud';
  }

  const sep = (p) => (p.includes('\\') && !p.includes('/') ? '\\' : '/');
  const localPath = (rel) => `${preset.source_path.replace(/[\\/]+$/, '')}${sep(preset.source_path)}${rel.split('/').join(sep(preset.source_path))}`;
  const folderName = (f) => (f === '' ? '(top level)' : `${f}/`);
</script>

<section class="view">
  <button class="back" onclick={() => (app.view = 'folders')}><ArrowLeft size={15} /> Synced folders</button>

  {#if !preset}
    <div class="card notice">Choose <strong>Compare with cloud</strong> on a synced folder.</div>
  {:else}
    <header class="view-head">
      <div class="min0">
        <h1 class="truncate">{preset.name}</h1>
        <div class="route">
          <span class="mono truncate start" title={preset.source_path}><Folder size={14} /> <bdi>{preset.source_path}</bdi></span>
          <span class="arrow">⇄</span>
          <span class="mono truncate start" title={destination}><Cloud size={14} /> <bdi>{destination}</bdi></span>
        </div>
      </div>
      <div class="head-actions">
        <button class="btn" disabled={loading} onclick={run}><RefreshCw size={15} class={loading ? 'spin' : ''} /> {loading ? 'Comparing…' : 'Compare again'}</button>
        <button class="btn primary" onclick={() => app.runPreset(preset, false)}><Play size={14} /> Sync now</button>
      </div>
    </header>

    {#if !isDesktop}
      <div class="card notice">Comparing runs in the JetStream desktop app.</div>
    {:else if error}
      <div class="card error">{error}</div>
    {:else if loading && !report}
      <div class="card notice">Comparing this folder with the cloud… (large folders can take a minute)</div>
    {/if}

    {#if report}
      <div class="verdict card" class:ok={inSync}>
        {#if inSync}
          <CircleCheck size={20} />
          <div>
            <strong>Everything on this computer is in the cloud.</strong>
            <span class="muted">
              {count(report.same.files)} files · {bytes(report.same.bytes)}{report.verified ? ` · ${count(report.verified)} confirmed by checksum` : ''}{report.cloud_only.files ? ` · the cloud also has ${count(report.cloud_only.files)} files not on this computer` : ''}
            </span>
          </div>
        {:else}
          <Upload size={20} />
          <div>
            <strong>
              {count(report.local_only.files + report.different.files)} files ({bytes(report.local_only.bytes + report.different.bytes)}) on this computer aren't in the cloud yet{report.different.files ? ' or have changed' : ''}.
            </strong>
            <span class="muted">Sync now uploads them. Compared {relativeTime(new Date(Date.now() - report.elapsed_seconds * 1000).toISOString())} in {duration(report.elapsed_seconds)}.</span>
          </div>
        {/if}
      </div>

      <div class="tiles" role="tablist" aria-label="Compare results">
        {#each GROUPS as g (g.key)}
          {@const Icon = g.icon}
          <button class="card tile {g.tone}" role="tab" aria-selected={tab === g.key} class:active={tab === g.key} onclick={() => (tab = g.key)}>
            <span class="label"><Icon size={15} /> {g.label}</span>
            <span class="value">{count(report[g.key].files)}</span>
            <span class="muted">{bytes(report[g.key].bytes)} · {g.hint}</span>
          </button>
        {/each}
      </div>

      <div class="grid">
        <div class="card panel">
          <h2>By folder</h2>
          <p class="caption">Click a folder to filter the list</p>
          <div class="ftable" role="table">
            <div class="frow head" role="row">
              <span role="columnheader">Folder</span>
              <span role="columnheader" class="num" title="In sync">✓</span>
              <span role="columnheader" class="num" title="Only on this computer">Local</span>
              <span role="columnheader" class="num" title="Only in the cloud">Cloud</span>
              <span role="columnheader" class="num" title="Different">Diff</span>
            </div>
            {#each report.folders as f (f.folder)}
              <button class="frow" class:active={folderFilter === f.folder} onclick={() => (folderFilter = folderFilter === f.folder ? null : f.folder)}>
                <span class="truncate mono" title={folderName(f.folder)}>{folderName(f.folder)}</span>
                <span class="num">{f.same ? count(f.same) : ''}</span>
                <span class="num" class:hot={f.local_only}>{f.local_only ? count(f.local_only) : ''}</span>
                <span class="num">{f.cloud_only ? count(f.cloud_only) : ''}</span>
                <span class="num" class:hot={f.different}>{f.different ? count(f.different) : ''}</span>
              </button>
            {/each}
          </div>
          {#if report.excluded}<p class="caption">{count(report.excluded)} local {report.excluded === 1 ? 'file' : 'files'} skipped by this folder's filters {report.excluded === 1 ? "isn't" : "aren't"} counted.</p>{/if}
          {#if report.verified}<p class="caption"><ShieldCheck size={13} /> {count(report.verified)} matching files confirmed by checksum (uploaded by the JetStream engine); others matched by size.</p>{/if}
        </div>

        <div class="card panel list">
          <div class="list-head">
            <h2>{GROUPS.find((g) => g.key === tab).label}{folderFilter !== null ? ` · ${folderName(folderFilter)}` : ''}</h2>
            {#if folderFilter !== null}<button class="btn ghost sm" onclick={() => (folderFilter = null)}>Show all folders</button>{/if}
          </div>
          {#each shown as d (d.path)}
            <div class="file">
              <span class="mono truncate start" title={d.path}><bdi>{d.path}</bdi></span>
              <span class="muted size">
                {#if tab === 'different'}
                  here {bytes(d.local_size)} · cloud {bytes(d.cloud_size)}
                {:else}
                  {bytes(d.local_size ?? d.cloud_size)}
                {/if}
              </span>
              <span class="muted when" title={d.cloud_updated ? `In the cloud: ${dateTime(d.cloud_updated)}` : ''}>
                {#if d.local_modified}modified {relativeTime(new Date(d.local_modified * 1000).toISOString())}{:else if d.cloud_updated}uploaded {relativeTime(d.cloud_updated)}{/if}
              </span>
              <span class="row-actions">
                {#if d.local_size !== null && d.local_size !== undefined}
                  <button class="btn ghost sm" title="Show on this computer" onclick={() => revealPath(localPath(d.path))}><Folder size={13} /></button>
                {/if}
                {#if d.cloud_size !== null && d.cloud_size !== undefined}
                  <button class="btn ghost sm" title="Show in Cloud storage" onclick={() => openInCloud(d.path)}><Cloud size={13} /></button>
                {/if}
              </span>
            </div>
          {:else}
            <p class="empty">Nothing here.</p>
          {/each}
          {#if report[tab].files > shown.length && folderFilter === null}
            <p class="caption">Showing the {count(shown.length)} largest of {count(report[tab].files)}.</p>
          {/if}
        </div>
      </div>
    {/if}
  {/if}
</section>

<style>
  .view {
    padding: 20px 32px 32px;
    max-width: 1280px;
  }
  .back {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: none;
    background: none;
    color: var(--muted);
    cursor: pointer;
    padding: 4px 0;
    margin-bottom: 8px;
    font: inherit;
  }
  .back:hover {
    color: var(--accent);
  }
  .view-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 16px;
    margin-bottom: 16px;
  }
  .min0 {
    min-width: 0;
  }
  h1 {
    font-size: 24px;
  }
  .route {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 6px;
    color: var(--text-2);
    min-width: 0;
  }
  .route .mono {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    max-width: 44%;
  }
  .arrow {
    color: var(--muted);
  }
  .head-actions {
    display: flex;
    gap: 8px;
    flex-shrink: 0;
  }
  .notice {
    padding: 28px;
    color: var(--muted);
    text-align: center;
  }
  .error {
    padding: 14px 16px;
    color: var(--danger);
    background: var(--danger-soft);
  }
  .verdict {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    padding: 14px 16px;
    margin-bottom: 12px;
    color: var(--accent);
  }
  .verdict.ok {
    color: var(--success);
  }
  .verdict div {
    display: flex;
    flex-direction: column;
    gap: 2px;
    color: var(--text);
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 12px;
    margin-bottom: 16px;
  }
  .tile {
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    text-align: left;
    cursor: pointer;
    font: inherit;
    color: var(--text);
    min-width: 0;
  }
  .tile:hover {
    border-color: var(--border-strong);
  }
  .tile.active {
    border-color: var(--accent);
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .tile .label {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-2);
  }
  .tile.good .label :global(svg) {
    color: var(--success);
  }
  .tile.accent .label :global(svg) {
    color: var(--accent);
  }
  .tile.warn .label :global(svg) {
    color: var(--warning);
  }
  .tile.neutral .label :global(svg) {
    color: var(--muted);
  }
  .value {
    font-size: 22px;
    font-weight: 700;
    letter-spacing: -0.01em;
  }
  .tile .muted {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .muted {
    color: var(--muted);
    font-size: 13px;
  }
  .grid {
    display: grid;
    grid-template-columns: minmax(360px, 1fr) 1.7fr;
    gap: 12px;
    align-items: start;
  }
  .panel {
    padding: 14px 16px;
    min-width: 0;
  }
  h2 {
    font-size: 15px;
  }
  .caption :global(svg) {
    flex-shrink: 0;
    margin-top: 2px;
  }
  .caption {
    color: var(--muted);
    font-size: 12.5px;
    margin: 4px 0 10px;
    display: flex;
    align-items: flex-start;
    gap: 5px;
  }
  .ftable {
    display: flex;
    flex-direction: column;
  }
  .frow {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 40px 44px 44px 36px;
    gap: 6px;
    align-items: center;
    padding: 6px 6px;
    border: none;
    border-bottom: 1px solid var(--border);
    background: none;
    text-align: left;
    font: inherit;
    font-size: 13px;
    color: var(--text);
    cursor: pointer;
  }
  .frow.head {
    color: var(--muted);
    font-size: 11.5px;
    font-weight: 600;
    text-transform: uppercase;
    cursor: default;
  }
  .frow:not(.head):hover {
    background: var(--surface-hover);
  }
  .frow.active {
    background: var(--accent-soft);
  }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .hot {
    font-weight: 700;
  }
  .list-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 8px;
  }
  .file {
    display: grid;
    grid-template-columns: 1fr auto 130px auto;
    gap: 12px;
    align-items: center;
    padding: 5px 0;
    border-bottom: 1px solid var(--border);
    font-size: 13px;
  }
  .size {
    text-align: right;
    white-space: nowrap;
  }
  .when {
    white-space: nowrap;
  }
  .row-actions {
    display: flex;
    gap: 2px;
  }
  .empty {
    color: var(--muted);
    padding: 20px 0;
    text-align: center;
  }
</style>
