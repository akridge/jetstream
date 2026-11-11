<script>
  // Add several folders at once: the subfolders of a folder or network share (tick the ones
  // you want), folders dropped on the window, or picked one by one. Each becomes its own synced
  // folder, uploaded to <destination>/<folder name>/.
  import { untrack } from 'svelte';
  import X from '@lucide/svelte/icons/x';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import FolderPlus from '@lucide/svelte/icons/folder-plus';
  import Search from '@lucide/svelte/icons/search';
  import { app } from '../lib/state.svelte.js';
  import { api } from '../lib/api.js';
  import { DEFAULT_CHECK_MINUTES, folderStatus } from '../lib/status.svelte.js';
  import { basename, count, normalizeDestination } from '../lib/format.js';
  import { listSubfolders, pickFolder, pickFolders } from '../lib/platform.js';

  let { dialog } = $props();

  const QUICK_BUCKETS = ['nmfs_odp_pifsc', 'nmfs_odp_afsc', 'nmfs_odp_swfsc', 'nmfs_odp_sefsc', 'nmfs_odp_nefsc', 'nmfs_odp_nwfsc'];
  const synced = (path) => app.presets.find((p) => samePath(p.source_path, path));
  const samePath = (a, b) => a.replace(/[\\/]+$/, '').toLowerCase() === b.replace(/[\\/]+$/, '').toLowerCase();
  const isNetwork = (path) => /^(\\\\|\/\/)/.test(path);
  const row = (path, name = basename(path)) => ({ path, name, checked: !synced(path), existing: synced(path)?.name || null });

  let parent = $state(untrack(() => dialog.parent || ''));
  let rows = $state(untrack(() => (dialog.paths || []).map((p) => row(p))));
  let loading = $state(false);
  let error = $state('');
  let filter = $state('');
  let destination = $state(untrack(() => app.prefs.last_parent_destination || ''));
  let tool = $state(app.toolAvailable('jetstream') ? 'jetstream' : 'gcloud');
  let autoUpload = $state(false);
  let receipts = $state(true);
  let saving = $state(false);

  async function loadParent(path) {
    parent = path;
    if (!path.trim()) return;
    loading = true;
    error = '';
    try {
      const subs = await listSubfolders(path.trim());
      const extra = rows.filter((r) => !subs.some((s) => samePath(s.path, r.path)) && !r.fromParent);
      rows = [...subs.map((s) => ({ ...row(s.path, s.name), fromParent: true })), ...extra];
      if (!subs.length) error = 'No folders inside this one. To sync it as a whole, use Add folder instead.';
    } catch (err) {
      error = String(err?.message || err);
    } finally {
      loading = false;
    }
  }
  $effect(() => {
    if (untrack(() => dialog.parent)) untrack(() => loadParent(dialog.parent));
  });

  async function browseParent() {
    const path = await pickFolder(parent, 'Choose the folder (or network share) that holds them');
    if (path) loadParent(path);
  }

  async function addOthers() {
    const paths = await pickFolders(parent);
    const fresh = paths.filter((p) => !rows.some((r) => samePath(r.path, p)));
    rows = [...rows, ...fresh.map((p) => row(p))];
  }

  let shown = $derived(filter.trim() ? rows.filter((r) => r.name.toLowerCase().includes(filter.trim().toLowerCase())) : rows);
  let chosen = $derived(rows.filter((r) => r.checked && !r.existing));
  let parentDest = $derived(normalizeDestination(destination));
  let validDestination = $derived(/^[a-z0-9][a-z0-9._-]{1,220}(\/.*)?$/.test(parentDest));
  // Two chosen folders with the same name would land in the same place.
  let clash = $derived.by(() => {
    const seen = new Set();
    for (const r of chosen) {
      const key = r.name.toLowerCase();
      if (seen.has(key)) return r.name;
      seen.add(key);
    }
    return null;
  });
  let canAdd = $derived(chosen.length > 0 && validDestination && !clash && !saving);
  let network = $derived(isNetwork(parent) || chosen.some((r) => isNetwork(r.path)));

  function setAll(on) {
    for (const r of shown) if (!r.existing) r.checked = on;
  }

  function uniqueName(name, taken) {
    let candidate = name;
    for (let n = 2; taken.has(candidate.toLowerCase()); n++) candidate = `${name} (${n})`;
    taken.add(candidate.toLowerCase());
    return candidate;
  }

  async function add(upload) {
    saving = true;
    error = '';
    try {
      const taken = new Set(app.presets.map((p) => p.name.toLowerCase()));
      const saved = [];
      for (const r of chosen) {
        saved.push(
          await api.savePreset({
            name: uniqueName(r.name, taken),
            source_path: r.path,
            destination: `${parentDest}/${r.name}`,
            options: {
              recursive: true,
              no_clobber: false,
              upload_tool: tool,
              exclude_patterns: '',
              exclude_folders: '',
              include_folder_name: true,
              parent_destination: parentDest,
              auto_upload: autoUpload,
              // Network shares are slow to walk: check them less often.
              check_minutes: isNetwork(r.path) ? 60 : DEFAULT_CHECK_MINUTES,
              receipts,
              schedule: { mode: 'off' },
            },
          }),
        );
      }
      await app.refreshPresets();
      app.savePrefs({ last_parent_destination: parentDest });
      app.addFoldersDialog = null;
      app.toast(`Added ${count(saved.length)} ${saved.length === 1 ? 'folder' : 'folders'}${upload ? '; uploading them one after another' : ''}`, 'success', { timeout: 5000 });
      for (const preset of saved) {
        if (upload) await app.runPreset(preset, false, { quiet: true });
        else folderStatus.check(preset);
      }
    } catch (err) {
      error = String(err?.message || err);
    } finally {
      saving = false;
    }
  }

  function close() {
    app.addFoldersDialog = null;
  }

  /** Folders dropped while the dialog is open are added to the list. */
  export function addPaths(paths) {
    const fresh = paths.filter((p) => !rows.some((r) => samePath(r.path, p)));
    rows = [...rows, ...fresh.map((p) => row(p))];
  }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && close()} />

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && close()}>
  <div class="dialog card" role="dialog" aria-modal="true" aria-labelledby="add-folders-title">
    <header>
      <h2 id="add-folders-title">Add several folders</h2>
      <button class="btn ghost icon" aria-label="Close" onclick={close}><X size={18} /></button>
    </header>

    <div class="body">
      <div class="field">
        <label for="af-parent">The folder (or network share) that holds them</label>
        <div class="with-button">
          <input
            id="af-parent"
            class="input mono"
            value={parent}
            placeholder={'\\\\server\\share\\projects  or  D:\\Surveys'}
            onchange={(e) => loadParent(e.currentTarget.value)}
            onkeydown={(e) => e.key === 'Enter' && loadParent(e.currentTarget.value)}
          />
          <button class="btn" onclick={browseParent}><FolderOpen size={16} /> Browse…</button>
        </div>
        <span class="hint">Tick the folders inside it to sync. Each one is synced on its own, so you can see, upload and check each separately.</span>
      </div>

      {#if rows.length || loading}
        <div class="picker">
          <div class="picker-head">
            <div class="search">
              <Search size={14} />
              <input class="input" placeholder="Filter" bind:value={filter} aria-label="Filter folders" />
            </div>
            <button class="link" onclick={() => setAll(true)}>Select all</button>
            <button class="link" onclick={() => setAll(false)}>None</button>
            <span class="hint push">{count(chosen.length)} of {count(rows.filter((r) => !r.existing).length)} selected</span>
          </div>
          <div class="list" role="group" aria-label="Folders to add">
            {#if loading}
              <div class="hint pad">Reading the folder…</div>
            {/if}
            {#each shown as r (r.path)}
              <label class="item" class:off={r.existing}>
                <input type="checkbox" bind:checked={r.checked} disabled={!!r.existing} />
                <span class="name mono truncate" title={r.path}>{r.name}</span>
                {#if r.existing}<span class="tag">already synced as “{r.existing}”</span>{:else if !r.fromParent}<span class="tag subtle truncate start" title={r.path}><bdi>{r.path}</bdi></span>{/if}
              </label>
            {/each}
          </div>
        </div>
      {/if}
      <button class="link" onclick={addOthers}><FolderPlus size={14} /> Add folders from somewhere else…</button>

      <div class="field">
        <label for="af-dest">Upload them to</label>
        <div class="dest-input">
          <span class="prefix">gs://</span>
          <input id="af-dest" class="input mono" bind:value={destination} placeholder="bucket/path" />
        </div>
        <div class="chips">
          {#each QUICK_BUCKETS as bucket (bucket)}
            <button class="chip" onclick={() => (destination = `${bucket}/`)}>{bucket.replace('nmfs_odp_', '').toUpperCase()}</button>
          {/each}
        </div>
        {#if destination && !validDestination}
          <span class="hint bad">Enter a bucket name, optionally followed by a path.</span>
        {:else if validDestination && chosen.length}
          <span class="hint">Each folder gets its own folder there, e.g. <span class="mono">gs://{parentDest}/{chosen[0].name}/</span></span>
        {/if}
        {#if clash}<span class="hint bad">Two selected folders are named “{clash}”; they would upload to the same place. Untick one.</span>{/if}
      </div>

      <div class="field">
        <span class="label">Options for these folders</span>
        <div class="segmented" role="radiogroup" aria-label="Upload with">
          <button role="radio" aria-checked={tool === 'jetstream'} class:selected={tool === 'jetstream'} disabled={!app.toolAvailable('jetstream')} onclick={() => (tool = 'jetstream')}>JetStream engine</button>
          <button role="radio" aria-checked={tool === 'gcloud'} class:selected={tool === 'gcloud'} onclick={() => (tool = 'gcloud')}>gcloud</button>
        </div>
        <label class="toggle">
          <input type="checkbox" bind:checked={autoUpload} />
          <span><strong>Upload new files automatically</strong><small>When new files appear, they're uploaded once nothing has changed for 2 minutes.</small></span>
        </label>
        <label class="toggle">
          <input type="checkbox" bind:checked={receipts} />
          <span><strong>Make a verification receipt after each sync</strong></span>
        </label>
        {#if network}
          <span class="hint">Network folders are checked for new files every hour (walking a share takes a while); change it per folder with Edit.</span>
        {/if}
      </div>

      {#if error}<div class="error">{error}</div>{/if}
    </div>

    <footer>
      <button class="btn ghost" onclick={close}>Cancel</button>
      <button class="btn" disabled={!canAdd} onclick={() => add(false)}>Add {chosen.length ? count(chosen.length) : ''} {chosen.length === 1 ? 'folder' : 'folders'}</button>
      <button class="btn primary" disabled={!canAdd} onclick={() => add(true)}>{saving ? 'Adding…' : `Add & upload`}</button>
    </footer>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 50;
    background: rgba(8, 15, 25, 0.45);
    backdrop-filter: blur(2px);
    display: grid;
    place-items: center;
    padding: 24px;
  }
  .dialog {
    width: min(640px, 100%);
    max-height: calc(100vh - 48px);
    display: flex;
    flex-direction: column;
    box-shadow: var(--shadow-lg);
  }
  header,
  footer {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 16px 20px;
  }
  header {
    justify-content: space-between;
    border-bottom: 1px solid var(--border);
  }
  header h2 {
    font-size: 17px;
  }
  footer {
    justify-content: flex-end;
    border-top: 1px solid var(--border);
  }
  .body {
    padding: 18px 20px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .field > label,
  .label {
    font-weight: 600;
    font-size: 13px;
  }
  .with-button {
    display: flex;
    gap: 8px;
  }
  .picker {
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }
  .picker-head {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 10px;
    background: var(--surface-2);
    border-bottom: 1px solid var(--border);
  }
  .search {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--muted);
  }
  .search .input {
    height: 28px;
    width: 160px;
  }
  .push {
    margin-left: auto;
  }
  .list {
    max-height: 260px;
    overflow-y: auto;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 12px;
    border-bottom: 1px solid var(--border);
    cursor: pointer;
    min-width: 0;
  }
  .item:last-child {
    border-bottom: none;
  }
  .item:hover {
    background: var(--surface-hover);
  }
  .item.off {
    cursor: default;
    color: var(--muted);
  }
  .item input {
    width: 16px;
    height: 16px;
    accent-color: var(--accent);
    flex: none;
  }
  .name {
    font-size: 13px;
    min-width: 0;
  }
  .tag {
    margin-left: auto;
    font-size: 12px;
    color: var(--muted);
    flex: none;
  }
  .tag.subtle {
    flex: 0 1 auto;
    max-width: 50%;
    min-width: 0;
  }
  .pad {
    padding: 10px 12px;
  }
  .dest-input {
    display: flex;
  }
  .prefix {
    display: grid;
    place-items: center;
    padding: 0 10px;
    border: 1px solid var(--border-strong);
    border-right: none;
    border-radius: var(--radius-sm) 0 0 var(--radius-sm);
    background: var(--surface-2);
    color: var(--muted);
    font-family: var(--mono);
    font-size: 12.5px;
  }
  .dest-input .input {
    border-radius: 0 var(--radius-sm) var(--radius-sm) 0;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    height: 26px;
    padding: 0 10px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--surface-2);
    font-size: 12px;
    cursor: pointer;
    color: var(--text-2);
  }
  .chip:hover {
    border-color: var(--accent);
    color: var(--accent);
  }
  .segmented {
    display: inline-flex;
    align-self: flex-start;
    padding: 3px;
    gap: 2px;
    border-radius: var(--radius-sm);
    background: var(--surface-2);
    border: 1px solid var(--border);
  }
  .segmented button {
    border: none;
    background: none;
    padding: 5px 12px;
    border-radius: 6px;
    font-size: 13px;
    cursor: pointer;
    color: var(--text-2);
  }
  .segmented button.selected {
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-sm, 0 1px 2px rgba(0, 0, 0, 0.1));
    font-weight: 600;
  }
  .segmented button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .toggle {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    cursor: pointer;
  }
  .toggle input {
    margin-top: 3px;
    width: 16px;
    height: 16px;
    accent-color: var(--accent);
  }
  .toggle span {
    display: flex;
    flex-direction: column;
  }
  .toggle strong {
    font-weight: 550;
  }
  .toggle small {
    color: var(--muted);
    font-size: 12.5px;
  }
  .link {
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    border: none;
    background: none;
    padding: 0;
    color: var(--accent);
    font-weight: 600;
    cursor: pointer;
  }
  .hint {
    color: var(--muted);
    font-size: 12.5px;
  }
  .bad {
    color: var(--danger);
  }
  .error {
    color: var(--danger);
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    background: var(--danger-soft);
  }
</style>
