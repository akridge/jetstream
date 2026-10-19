<script>
  import Folder from '@lucide/svelte/icons/folder';
  import File from '@lucide/svelte/icons/file';
  import FileImage from '@lucide/svelte/icons/file-image';
  import FileVideo from '@lucide/svelte/icons/file-video-camera';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import Copy from '@lucide/svelte/icons/copy';
  import ArrowRightLeft from '@lucide/svelte/icons/arrow-right-left';
  import FolderSync from '@lucide/svelte/icons/folder-sync';
  import Sigma from '@lucide/svelte/icons/sigma';
  import Cloud from '@lucide/svelte/icons/cloud';
  import CloudOpDialog from '../components/CloudOpDialog.svelte';
  import { app } from '../lib/state.svelte.js';
  import { bytes, count, relativeTime, dateTime } from '../lib/format.js';
  import { folderTotals, isDesktop, listFolder } from '../lib/platform.js';

  const QUICK_BUCKETS = ['nmfs_odp_pifsc', 'nmfs_odp_afsc', 'nmfs_odp_swfsc', 'nmfs_odp_sefsc', 'nmfs_odp_nefsc', 'nmfs_odp_nwfsc'];

  let location = $state('');
  let bucket = $state('');
  let prefix = $state('');
  let entries = $state([]);
  let nextToken = $state(null);
  let loading = $state(false);
  let error = $state('');
  let selected = $state(null);
  let totals = $state(null);
  let totalsFor = $state('');
  let counting = $state(false);
  let operation = $state(null);

  let crumbs = $derived(prefix.split('/').filter(Boolean));
  let target = $derived(
    selected || (prefix ? { bucket, path: prefix, isFolder: true, name: crumbs[crumbs.length - 1] } : null),
  );
  let shortcuts = $derived(app.prefs.recent_buckets?.length ? app.prefs.recent_buckets : QUICK_BUCKETS);

  function parse(value) {
    const clean = value.trim().replace(/^gs:\/\//, '').replace(/^\/+/, '');
    const [b, ...rest] = clean.split('/');
    const p = rest.join('/');
    return { bucket: b, prefix: p && !p.endsWith('/') ? `${p}/` : p };
  }

  async function open(b, p = '', { remember = true } = {}) {
    if (!b) return;
    bucket = b;
    prefix = p;
    location = `gs://${b}/${p}`;
    selected = null;
    totals = null;
    error = '';
    loading = true;
    try {
      const listing = await listFolder(b, p);
      entries = listing.entries;
      nextToken = listing.next_page_token;
      if (remember) app.remember('recent_buckets', b);
    } catch (err) {
      entries = [];
      nextToken = null;
      error = String(err?.message || err);
    } finally {
      loading = false;
    }
  }

  async function loadMore() {
    loading = true;
    try {
      const listing = await listFolder(bucket, prefix, nextToken);
      entries = [...entries, ...listing.entries];
      nextToken = listing.next_page_token;
    } catch (err) {
      error = String(err?.message || err);
    } finally {
      loading = false;
    }
  }

  function go() {
    const { bucket: b, prefix: p } = parse(location);
    open(b, p);
  }

  function crumbPath(index) {
    return crumbs.slice(0, index + 1).join('/') + '/';
  }

  async function computeTotals() {
    const item = target || { bucket, path: prefix };
    counting = true;
    totalsFor = `gs://${item.bucket}/${item.path}`;
    try {
      totals = await folderTotals(item.bucket, item.path);
    } catch (err) {
      app.toast(String(err?.message || err), 'error');
    } finally {
      counting = false;
    }
  }

  function copyPath() {
    const item = target || { bucket, path: prefix };
    navigator.clipboard?.writeText(`gs://${item.bucket}/${item.path}`);
    app.toast('Path copied', 'info', { timeout: 2000 });
  }

  function iconFor(entry) {
    if (entry.is_folder) return Folder;
    const type = entry.content_type || '';
    if (type.startsWith('image/') || /\.(jpe?g|png|tiff?|dng|cr2|nef)$/i.test(entry.name)) return FileImage;
    if (type.startsWith('video/') || /\.(mp4|mov|avi|mkv)$/i.test(entry.name)) return FileVideo;
    return File;
  }

  function select(entry) {
    const name = entry.name.replace(/\/$/, '');
    selected = selected?.path === entry.path ? null : { bucket, path: entry.path, isFolder: entry.is_folder, name };
  }
</script>

<section class="view">
  <header class="view-head">
    <div>
      <h1>Cloud storage</h1>
      <p class="sub">Browse buckets, check folder sizes, and copy or move data in the cloud.</p>
    </div>
  </header>

  <form class="locationbar" onsubmit={(e) => { e.preventDefault(); go(); }}>
    <Cloud size={17} />
    <input class="mono" bind:value={location} placeholder="gs://bucket/folder/" aria-label="Cloud location" />
    <button class="btn primary sm" type="submit" disabled={!location.trim() || !isDesktop}>Open</button>
  </form>
  <div class="shortcuts">
    {#each shortcuts as b (b)}
      <button class="chip" onclick={() => open(b)}>{b}</button>
    {/each}
  </div>

  {#if !isDesktop}
    <div class="card notice">Browsing buckets runs in the JetStream desktop app.</div>
  {:else if bucket}
    <div class="card browser">
      <div class="toolbar">
        <nav class="crumbs" aria-label="Folder path">
          <button onclick={() => open(bucket, '')}>{bucket}</button>
          {#each crumbs as crumb, i (i)}
            <ChevronRight size={14} />
            <button onclick={() => open(bucket, crumbPath(i), { remember: false })}>{crumb}</button>
          {/each}
        </nav>
        <div class="actions">
          <span class="target truncate">{target ? target.name : bucket}</span>
          <button class="btn ghost sm" onclick={copyPath} title="Copy gs:// path"><Copy size={14} /></button>
          <button class="btn ghost sm" disabled={counting} onclick={computeTotals}><Sigma size={14} /> Size</button>
          <button class="btn sm" disabled={!target?.isFolder} onclick={() => (operation = { item: target, mode: 'copy' })}><FolderSync size={14} /> Sync to…</button>
          <button class="btn sm" disabled={!target} onclick={() => (operation = { item: target, mode: 'move' })}><ArrowRightLeft size={14} /> Move…</button>
          <button class="btn ghost sm icon" aria-label="Refresh" onclick={() => open(bucket, prefix, { remember: false })}><RefreshCw size={14} class={loading ? 'spin' : ''} /></button>
        </div>
      </div>

      {#if counting || totals}
        <div class="totals">
          {#if counting}
            Counting everything in <span class="mono">{totalsFor}</span>…
          {:else}
            <span class="mono truncate">{totalsFor}</span>
            <strong>{bytes(totals.bytes)}</strong>
            <span>{count(totals.objects)}{totals.truncated ? '+' : ''} objects</span>
            {#each Object.entries(totals.by_storage_class) as [cls, b] (cls)}
              <span class="pill">{cls.toLowerCase()} · {bytes(b)}</span>
            {/each}
            {#if totals.newest}<span class="muted">updated {relativeTime(totals.newest)}</span>{/if}
          {/if}
        </div>
      {/if}

      {#if error}
        <div class="error">
          {error}
          {#if /sign|auth|credential|401|403/i.test(error)}
            <button class="btn sm" onclick={() => (app.view = 'settings')}>Check sign-in</button>
          {/if}
        </div>
      {/if}

      <div class="table" role="table">
        <div class="thead" role="row">
          <span role="columnheader">Name</span>
          <span role="columnheader" class="num">Size</span>
          <span role="columnheader">Updated</span>
          <span role="columnheader">Class</span>
        </div>
        {#each entries as entry (entry.path)}
          {@const Icon = iconFor(entry)}
          <div
            class="tr"
            class:selected={selected?.path === entry.path}
            role="row"
            tabindex="0"
            onclick={() => select(entry)}
            ondblclick={() => entry.is_folder && open(bucket, entry.path, { remember: false })}
            onkeydown={(e) => e.key === 'Enter' && (entry.is_folder ? open(bucket, entry.path, { remember: false }) : select(entry))}
          >
            <span class="name" role="cell">
              <Icon size={16} color={entry.is_folder ? 'var(--accent)' : 'var(--muted)'} />
              {#if entry.is_folder}
                <button class="link" onclick={(e) => { e.stopPropagation(); open(bucket, entry.path, { remember: false }); }}>{entry.name}</button>
              {:else}
                <span class="truncate">{entry.name}</span>
              {/if}
            </span>
            <span class="num" role="cell">{entry.size !== null && entry.size !== undefined ? bytes(entry.size) : ''}</span>
            <span class="muted" role="cell" title={entry.updated ? dateTime(entry.updated) : ''}>{entry.updated ? relativeTime(entry.updated) : ''}</span>
            <span class="muted" role="cell">{entry.storage_class ? entry.storage_class.toLowerCase() : ''}</span>
          </div>
        {:else}
          {#if !loading && !error}
            <div class="empty">This folder is empty.</div>
          {/if}
        {/each}
        {#if loading && !entries.length}
          <div class="empty">Loading…</div>
        {/if}
      </div>
      {#if nextToken}
        <div class="more"><button class="btn sm" disabled={loading} onclick={loadMore}>Load more</button></div>
      {/if}
    </div>
  {:else}
    <div class="card notice">Open a bucket above, or pick one of the shortcuts.</div>
  {/if}
</section>

{#if operation}
  <CloudOpDialog item={operation.item} mode={operation.mode} onclose={() => (operation = null)} />
{/if}

<style>
  .view {
    padding: 28px 32px 32px;
    max-width: 1280px;
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
  .locationbar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 6px 6px 12px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--muted);
  }
  .locationbar input {
    flex: 1;
    border: none;
    outline: none;
    background: transparent;
    min-width: 0;
    color: var(--text);
  }
  .shortcuts {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 10px 0 16px;
  }
  .chip {
    height: 26px;
    padding: 0 10px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--surface);
    font-size: 12px;
    color: var(--text-2);
    cursor: pointer;
    font-family: var(--mono);
  }
  .chip:hover {
    border-color: var(--accent);
    color: var(--accent);
  }
  .notice {
    padding: 28px;
    color: var(--muted);
    text-align: center;
  }
  .browser {
    overflow: hidden;
  }
  .toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
    flex-wrap: wrap;
  }
  .crumbs {
    display: flex;
    align-items: center;
    gap: 2px;
    color: var(--muted);
    min-width: 0;
    flex-wrap: wrap;
  }
  .crumbs button {
    border: none;
    background: none;
    padding: 4px 6px;
    border-radius: 6px;
    font-weight: 600;
    color: var(--text-2);
    cursor: pointer;
  }
  .crumbs button:hover {
    background: var(--surface-2);
    color: var(--text);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .target {
    max-width: 180px;
    color: var(--muted);
    font-size: 12.5px;
    margin-right: 4px;
  }
  .totals {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    padding: 10px 14px;
    background: var(--accent-soft);
    font-size: 13px;
  }
  .totals .mono {
    max-width: 380px;
  }
  .error {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 14px;
    background: var(--danger-soft);
    color: var(--danger);
  }
  .thead,
  .tr {
    display: grid;
    grid-template-columns: 1fr 110px 140px 110px;
    gap: 12px;
    align-items: center;
    padding: 0 14px;
  }
  .thead {
    height: 34px;
    color: var(--muted);
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    border-bottom: 1px solid var(--border);
  }
  .tr {
    height: 38px;
    border-bottom: 1px solid var(--border);
    cursor: default;
  }
  .tr:hover {
    background: var(--surface-hover);
  }
  .tr.selected {
    background: var(--accent-soft);
  }
  .name {
    display: flex;
    align-items: center;
    gap: 9px;
    min-width: 0;
  }
  .link {
    border: none;
    background: none;
    padding: 0;
    font-weight: 600;
    color: var(--text);
    cursor: pointer;
  }
  .link:hover {
    color: var(--accent);
    text-decoration: underline;
  }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .muted {
    color: var(--muted);
    font-size: 13px;
  }
  .empty {
    padding: 36px;
    text-align: center;
    color: var(--muted);
  }
  .more {
    padding: 12px;
    display: flex;
    justify-content: center;
  }
</style>
