<script>
  // A catalog group's files as thumbnails, with the preview pane for the one selected.
  import X from '@lucide/svelte/icons/x';
  import ThumbTile from './ThumbTile.svelte';
  import PreviewPane from './PreviewPane.svelte';
  import { app } from '../lib/state.svelte.js';
  import { cloudOps } from '../lib/cloudops.svelte.js';
  import { bytes, count } from '../lib/format.js';
  import { catalogGroupFiles, downloadCloud, newOpId, openUrl, pickFolder, revealPath } from '../lib/platform.js';

  let { opId, bucket, group, onclose } = $props();

  const PAGE = 120;
  let entries = $state([]);
  let total = $state(0);
  let loading = $state(false);
  let error = $state('');
  let selected = $state(null);
  let paneWidth = $derived(Number(app.prefs.cloud_preview_width) || 340);

  $effect(() => {
    const key = group;
    entries = [];
    selected = null;
    error = '';
    load(key, 0);
  });

  async function load(key, offset) {
    loading = true;
    try {
      const page = await catalogGroupFiles(opId, key, offset, PAGE);
      if (key !== group) return;
      entries = offset ? [...entries, ...page.entries] : page.entries;
      total = page.total;
    } catch (err) {
      error = String(err?.message || err);
    } finally {
      loading = false;
    }
  }

  const gsPath = (entry) => `gs://${bucket}/${entry.path}`;
  const encodePath = (path) => path.split('/').map(encodeURIComponent).join('/');

  async function download(entry) {
    const dest = await pickFolder(app.prefs.last_download_dir || '', 'Choose where to save');
    if (!dest) return;
    app.savePrefs({ last_download_dir: dest });
    const id = newOpId();
    cloudOps.start(id, `Download “${entry.name}”`, 0, 'Downloaded', (onProgress) => downloadCloud(id, bucket, [entry.path], dest, onProgress), {
      action: { label: 'Show folder', run: () => revealPath(dest) },
    });
  }
</script>

<div class="gallery card">
  <header>
    <div class="title">
      <strong class="mono">{group ?? 'All files'}</strong>
      <span class="muted">{count(total)} {total === 1 ? 'file' : 'files'}</span>
    </div>
    <button class="btn ghost sm icon" aria-label="Close group" onclick={onclose}><X size={15} /></button>
  </header>
  <div class="split">
    <div class="tiles-col">
      {#if error}
        <div class="error">{error}</div>
      {/if}
      <div class="tiles">
        {#each entries as entry (entry.path)}
          <button class="tile" class:selected={selected?.path === entry.path} title={entry.path} onclick={() => (selected = selected?.path === entry.path ? null : entry)}>
            <ThumbTile {bucket} {entry} />
            <span class="name truncate">{entry.name}</span>
            <span class="meta muted">{bytes(entry.size || 0)}</span>
          </button>
        {/each}
      </div>
      {#if entries.length < total}
        <div class="more"><button class="btn sm" disabled={loading} onclick={() => load(group, entries.length)}>Show more ({count(total - entries.length)} left)</button></div>
      {:else if loading && !entries.length}
        <p class="muted pad">Loading…</p>
      {/if}
    </div>
    {#if selected}
      <PreviewPane
        {bucket}
        entries={[selected]}
        width={paneWidth}
        onresize={(w) => app.savePrefs({ cloud_preview_width: Math.round(w) })}
        onclose={() => (selected = null)}
        oncopy={() => {
          navigator.clipboard?.writeText(gsPath(selected));
          app.toast('Path copied', 'info', { timeout: 2000 });
        }}
        ondownload={() => download(selected)}
        onconsole={() => openUrl(`https://console.cloud.google.com/storage/browser/_details/${bucket}/${encodePath(selected.path)}`)}
      />
    {/if}
  </div>
</div>

<style>
  .gallery {
    margin-top: 16px;
    overflow: hidden;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 14px;
    border-bottom: 1px solid var(--border);
  }
  .title {
    display: flex;
    gap: 10px;
    align-items: baseline;
    min-width: 0;
  }
  .split {
    display: flex;
    align-items: stretch;
  }
  .tiles-col {
    flex: 1;
    min-width: 0;
    max-height: 72vh;
    overflow-y: auto;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 12px;
    padding: 14px;
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 6px;
    border: 1px solid transparent;
    border-radius: 10px;
    background: none;
    text-align: left;
    cursor: pointer;
    font: inherit;
    color: inherit;
    min-width: 0;
  }
  .tile:hover {
    background: var(--surface-2);
  }
  .tile.selected {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .name {
    font-size: 12.5px;
  }
  .meta {
    font-size: 11.5px;
  }
  .more {
    padding: 0 14px 14px;
  }
  .pad {
    padding: 14px;
    margin: 0;
  }
  .muted {
    color: var(--muted);
  }
  .error {
    margin: 14px;
    color: var(--danger);
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    background: var(--danger-soft);
  }
</style>
