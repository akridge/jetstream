<script>
  import FolderPlus from '@lucide/svelte/icons/folder-plus';
  import FolderSync from '@lucide/svelte/icons/folder-sync';
  import FolderInput from '@lucide/svelte/icons/folder-input';
  import CloudUpload from '@lucide/svelte/icons/cloud-upload';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import CircleCheck from '@lucide/svelte/icons/circle-check';
  import ListChecks from '@lucide/svelte/icons/list-checks';
  import FolderCard from '../components/FolderCard.svelte';
  import { app, ACTIVE } from '../lib/state.svelte.js';
  import { api } from '../lib/api.js';
  import { folderStatus } from '../lib/status.svelte.js';
  import { bytes, count, relativeTime } from '../lib/format.js';
  import { isDesktop, pickFolder } from '../lib/platform.js';

  let running = $derived(app.jobList.filter((j) => ACTIVE.has(j.status)).length);

  async function add() {
    const path = await pickFolder();
    if (path) app.addFolder(path);
  }

  // Folders with new files waiting (from the background checks).
  let pending = $derived(folderStatus.pending);
  let pendingFiles = $derived(pending.reduce((n, p) => n + p.status.changes, 0));
  let pendingBytes = $derived(pending.reduce((n, p) => n + p.status.bytes, 0));
  let anyChecking = $derived(Object.keys(folderStatus.checking).length > 0);
  let checkedStatuses = $derived(app.presets.map((p) => folderStatus.of(p)).filter((s) => s && !s.error));
  let allUpToDate = $derived(app.presets.length > 0 && checkedStatuses.length === app.presets.length && pending.length === 0);
  let oldestCheck = $derived(checkedStatuses.length ? Math.min(...checkedStatuses.map((s) => s.checked_at)) : null);

  async function uploadAll(list = pending.map((p) => p.preset)) {
    app.toast(`Uploading ${count(list.length)} ${list.length === 1 ? 'folder' : 'folders'}, one after another`, 'info', { timeout: 4000 });
    for (const preset of list) await app.runPreset(preset, false, { quiet: true });
  }

  // Selecting several folders for one action.
  let selecting = $state(false);
  let selected = $state({});
  let chosen = $derived(app.presets.filter((p) => selected[p.preset_id]));
  function toggle(preset) {
    selected = { ...selected, [preset.preset_id]: !selected[preset.preset_id] };
  }
  function selectAll(on) {
    selected = on ? Object.fromEntries(app.presets.map((p) => [p.preset_id, true])) : {};
  }
  function stopSelecting() {
    selecting = false;
    selected = {};
  }
  async function previewChosen() {
    for (const preset of chosen) await app.runPreset(preset, true, { quiet: true });
    app.toast(`Previewing ${count(chosen.length)} folders`, 'info', { timeout: 3000 });
  }
  async function setAuto(on) {
    for (const preset of chosen) {
      await api.savePreset({ name: preset.name, source_path: preset.source_path, destination: preset.destination, options: { ...preset.options, auto_upload: on } });
    }
    await app.refreshPresets();
    app.toast(`Auto-upload ${on ? 'on' : 'off'} for ${count(chosen.length)} folders`, 'success', { timeout: 3000 });
    if (on) for (const preset of chosen) folderStatus.check(preset);
  }
  async function removeChosen() {
    const n = chosen.length;
    if (!window.confirm(`Stop syncing ${n} ${n === 1 ? 'folder' : 'folders'}? Nothing is deleted from this computer or the cloud.`)) return;
    for (const preset of chosen) await app.removePreset(preset);
    stopSelecting();
  }
</script>

<section class="view">
  <header class="view-head">
    <div>
      <h1>Synced folders</h1>
      <p class="sub">
        {#if app.presets.length}
          {app.presets.length} folder{app.presets.length === 1 ? '' : 's'}
          {#if running}· {running} job{running === 1 ? '' : 's'} running{/if}
        {:else}
          Keep local folders backed up to Google Cloud Storage.
        {/if}
      </p>
    </div>
    <div class="head-actions">
      {#if app.presets.length > 1}
        <button class="btn" class:selected-mode={selecting} onclick={() => (selecting ? stopSelecting() : (selecting = true))}><ListChecks size={16} /> {selecting ? 'Done' : 'Select'}</button>
      {/if}
      <button class="btn" title="Add the folders inside a folder or network share, or several folders at once" onclick={() => app.addFolders()}><FolderInput size={16} /> Add several…</button>
      <button class="btn primary" onclick={add}><FolderPlus size={17} /> Add folder</button>
    </div>
  </header>

  {#if selecting}
    <div class="bar card">
      <strong>{count(chosen.length)} selected</strong>
      <button class="link" onclick={() => selectAll(chosen.length < app.presets.length)}>{chosen.length < app.presets.length ? 'Select all' : 'Select none'}</button>
      <div class="push actions">
        <button class="btn primary sm" disabled={!chosen.length} onclick={() => uploadAll(chosen)}><CloudUpload size={15} /> Upload</button>
        <button class="btn sm" disabled={!chosen.length} onclick={previewChosen}>Preview</button>
        <button class="btn sm" disabled={!chosen.length} onclick={() => chosen.forEach((p) => folderStatus.check(p))}>Check for changes</button>
        <button class="btn sm" disabled={!chosen.length} onclick={() => setAuto(true)}>Auto-upload on</button>
        <button class="btn sm" disabled={!chosen.length} onclick={() => setAuto(false)}>Off</button>
        <button class="btn ghost sm danger" disabled={!chosen.length} onclick={removeChosen}>Remove…</button>
      </div>
    </div>
  {:else if pending.length}
    <div class="bar card accent">
      <CloudUpload size={18} />
      <span>
        <strong>{count(pending.length)} {pending.length === 1 ? 'folder has' : 'folders have'} new files</strong>
        <span class="muted">· {count(pendingFiles)} {pendingFiles === 1 ? 'file' : 'files'} · {bytes(pendingBytes)}</span>
      </span>
      <div class="push actions">
        <button class="btn ghost sm" disabled={anyChecking} onclick={() => folderStatus.checkAll()}><RefreshCw size={14} class={anyChecking ? 'spin' : ''} /> Check again</button>
        <button class="btn primary sm" onclick={() => uploadAll()}><CloudUpload size={15} /> Upload all</button>
      </div>
    </div>
  {:else if allUpToDate && app.presets.length > 1}
    <div class="bar quiet">
      <CircleCheck size={16} />
      <span>All {count(app.presets.length)} folders are up to date{oldestCheck ? ` · checked ${relativeTime(new Date(oldestCheck).toISOString())}` : ''}</span>
      <button class="link" disabled={anyChecking} onclick={() => folderStatus.checkAll()}>{anyChecking ? 'Checking…' : 'Check now'}</button>
    </div>
  {/if}

  {#if !app.loaded}
    <div class="grid">
      {#each [1, 2, 3] as i (i)}
        <div class="card skeleton"></div>
      {/each}
    </div>
  {:else if app.presets.length === 0}
    <button class="empty" onclick={add}>
      <div class="empty-icon"><FolderSync size={34} /></div>
      <h2>Add your first folder</h2>
      <p>
        {isDesktop ? 'Drag a folder from File Explorer onto this window, or click to choose one.' : 'Click to enter a folder path.'}
        <br />JetStream remembers where it goes, previews changes, and uploads only what's new.
      </p>
    </button>
    <p class="several">Several folders, or a network share full of them? <button class="link" onclick={() => app.addFolders()}>Add several at once</button></p>
  {:else}
    <div class="grid">
      {#each app.presets as preset (preset.preset_id)}
        <FolderCard {preset} {selecting} selected={!!selected[preset.preset_id]} onselect={() => toggle(preset)} />
      {/each}
      <button class="add-tile" onclick={add}>
        <FolderPlus size={22} />
        <span>Add folder</span>
        {#if isDesktop}<small>or drop one here</small>{/if}
      </button>
    </div>
  {/if}
</section>

<style>
  .view {
    padding: 28px 32px 40px;
    max-width: 1280px;
  }
  .view-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-end;
    gap: 16px;
    margin-bottom: 22px;
  }
  h1 {
    font-size: 24px;
  }
  .sub {
    margin: 4px 0 0;
    color: var(--muted);
  }
  .head-actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    justify-content: flex-end;
  }
  .selected-mode {
    border-color: var(--accent);
    color: var(--accent);
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
    margin-bottom: 16px;
    flex-wrap: wrap;
  }
  .bar.accent {
    border-color: color-mix(in srgb, var(--accent) 35%, var(--border));
    background: var(--accent-soft);
    color: var(--accent);
  }
  .bar.accent strong {
    color: var(--text);
  }
  .bar.quiet {
    color: var(--success);
    font-size: 13px;
    padding: 0 2px;
  }
  .bar.quiet span {
    color: var(--muted);
  }
  .bar .muted {
    color: var(--muted);
  }
  .push {
    margin-left: auto;
  }
  .actions {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .link {
    border: none;
    background: none;
    padding: 0;
    color: var(--accent);
    font-weight: 600;
    cursor: pointer;
  }
  .danger {
    color: var(--danger);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(340px, 1fr));
    gap: 16px;
  }
  .skeleton {
    height: 210px;
    background: linear-gradient(90deg, var(--surface) 0%, var(--surface-2) 50%, var(--surface) 100%);
    background-size: 200% 100%;
    animation: shimmer 1.2s infinite;
  }
  @keyframes shimmer {
    to {
      background-position: -200% 0;
    }
  }
  .empty {
    width: 100%;
    padding: 64px 24px;
    border: 2px dashed var(--border-strong);
    border-radius: 14px;
    background: var(--surface);
    text-align: center;
    cursor: pointer;
    color: var(--text);
  }
  .empty:hover {
    border-color: var(--accent);
    background: var(--surface-hover);
  }
  .empty-icon {
    width: 68px;
    height: 68px;
    margin: 0 auto 14px;
    border-radius: 18px;
    display: grid;
    place-items: center;
    background: var(--accent-soft);
    color: var(--accent);
  }
  .empty h2 {
    font-size: 19px;
    margin-bottom: 6px;
  }
  .empty p {
    color: var(--muted);
    margin: 0;
    line-height: 1.6;
  }
  .several {
    margin-top: 14px;
    text-align: center;
    color: var(--muted);
  }
  .add-tile {
    min-height: 210px;
    border: 2px dashed var(--border-strong);
    border-radius: var(--radius);
    background: transparent;
    color: var(--muted);
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    cursor: pointer;
    font-weight: 600;
  }
  .add-tile:hover {
    border-color: var(--accent);
    color: var(--accent);
  }
  .add-tile small {
    font-weight: 400;
  }
</style>
