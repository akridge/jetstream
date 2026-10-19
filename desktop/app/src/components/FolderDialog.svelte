<script>
  import { untrack } from 'svelte';
  import X from '@lucide/svelte/icons/x';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import Zap from '@lucide/svelte/icons/zap';
  import Cloud from '@lucide/svelte/icons/cloud';
  import { app } from '../lib/state.svelte.js';
  import { api } from '../lib/api.js';
  import { bytes, count, normalizeDestination, basename } from '../lib/format.js';
  import { pickFolder } from '../lib/platform.js';

  let { dialog } = $props();

  // Edit a local copy; nothing is saved until the user confirms.
  let form = $state(untrack(() => structuredClone($state.snapshot(dialog.preset))));
  form.options ??= {};
  let saving = $state(false);
  let error = $state('');
  let showFilters = $state(!!(form.options.exclude_patterns || form.options.exclude_folders));
  let stats = $state(null);
  let counting = $state(false);
  let nameTouched = $state(untrack(() => !!dialog.previousName));
  form.options.schedule ??= { mode: 'off' };
  const sched = form.options.schedule;
  sched.time ??= '18:00';
  sched.every_hours ??= 6;
  sched.days ??= [0, 1, 2, 3, 4, 5, 6];
  const DAY_NAMES = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];

  function toggleDay(day) {
    const days = new Set(sched.days);
    if (days.has(day)) {
      if (days.size > 1) days.delete(day);
    } else days.add(day);
    sched.days = [...days].sort();
  }

  const QUICK_BUCKETS = ['nmfs_odp_pifsc', 'nmfs_odp_afsc', 'nmfs_odp_swfsc', 'nmfs_odp_sefsc', 'nmfs_odp_nefsc', 'nmfs_odp_nwfsc'];
  const FILTER_PRESETS = {
    'Temp files': { patterns: ['*.tmp', '*.bak', '*.temp', '~*', '*.swp'] },
    'System files': { patterns: ['Thumbs.db', '.DS_Store', 'desktop.ini'], folders: ['__pycache__', '.git'] },
    'RAW photos': { patterns: ['*.CR2', '*.CR3', '*.NEF', '*.ARW', '*.DNG'] },
  };

  let destination = $derived(normalizeDestination(form.destination));
  let validDestination = $derived(/^[a-z0-9][a-z0-9._-]{1,220}(\/.*)?$/.test(destination));
  let canSave = $derived(form.name.trim() && form.source_path.trim() && validDestination && !saving);
  let engineAvailable = $derived(app.toolAvailable('jetstream'));

  let countTimer;
  $effect(() => {
    const path = form.source_path.trim();
    clearTimeout(countTimer);
    stats = null;
    if (!path) return;
    countTimer = setTimeout(async () => {
      counting = true;
      try {
        const result = await api.analyzeFolder(path);
        if (form.source_path.trim() === path) stats = result;
      } catch (err) {
        if (form.source_path.trim() === path) stats = { error: err.message };
      } finally {
        counting = false;
      }
    }, 500);
  });

  async function browse() {
    const path = await pickFolder(form.source_path);
    if (path) setFolder(path);
  }

  export function setFolder(path) {
    form.source_path = path;
    if (!nameTouched) form.name = basename(path);
  }

  function addFilter(name) {
    const preset = FILTER_PRESETS[name];
    const patterns = new Set((form.options.exclude_patterns || '').split('\n').map((s) => s.trim()).filter(Boolean));
    (preset.patterns || []).forEach((p) => patterns.add(p));
    form.options.exclude_patterns = [...patterns].join('\n');
    const folders = new Set((form.options.exclude_folders || '').split(',').map((s) => s.trim()).filter(Boolean));
    (preset.folders || []).forEach((f) => folders.add(f));
    form.options.exclude_folders = [...folders].join(', ');
  }

  async function save(previewAfter) {
    error = '';
    saving = true;
    try {
      const preset = {
        name: form.name.trim(),
        source_path: form.source_path.trim(),
        destination,
        options: { ...form.options, recursive: form.options.recursive !== false },
      };
      const saved = await app.savePreset(preset, dialog.previousName);
      app.folderDialog = null;
      app.toast(`Saved ${saved.name}`, 'success', { timeout: 3000 });
      if (previewAfter) await app.runPreset(saved, true);
    } catch (err) {
      error = err.message;
    } finally {
      saving = false;
    }
  }

  function close() {
    app.folderDialog = null;
  }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && close()} />

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && close()}>
  <div class="dialog card" role="dialog" aria-modal="true" aria-labelledby="folder-dialog-title">
    <header>
      <h2 id="folder-dialog-title">{dialog.previousName ? 'Edit folder' : 'Add a folder to sync'}</h2>
      <button class="btn ghost icon" aria-label="Close" onclick={close}><X size={18} /></button>
    </header>

    <div class="body">
      <div class="field">
        <label for="fd-source">Folder on this computer</label>
        <div class="with-button">
          <input id="fd-source" class="input mono" bind:value={form.source_path} placeholder="D:\Survey\2026" />
          <button class="btn" onclick={browse}><FolderOpen size={16} /> Browse…</button>
        </div>
        <span class="hint">
          {#if counting}
            Counting files…
          {:else if stats?.error}
            <span class="bad">{stats.error}</span>
          {:else if stats}
            {stats.scan_mode === 'folder_only' ? 'About' : ''} {count(stats.total_files)} files · {bytes(stats.total_size_bytes)}
            {#if stats.subfolder_count}· {count(stats.subfolder_count)} subfolders{/if}
          {:else}
            Tip: you can also drop a folder onto the window.
          {/if}
        </span>
      </div>

      <div class="field">
        <label for="fd-dest">Destination in Google Cloud Storage</label>
        <div class="dest-input">
          <span class="prefix">gs://</span>
          <input id="fd-dest" class="input mono" bind:value={form.destination} placeholder="bucket/path/to/folder" />
        </div>
        <div class="chips">
          {#each QUICK_BUCKETS as bucket (bucket)}
            <button class="chip" onclick={() => (form.destination = `${bucket}/`)}>{bucket.replace('nmfs_odp_', '').toUpperCase()}</button>
          {/each}
        </div>
        {#if form.destination && !validDestination}
          <span class="hint bad">Use the form bucket-name/optional/path.</span>
        {:else if destination}
          <span class="hint">Files go to <span class="mono">gs://{destination}/</span>, keeping their subfolder layout.</span>
        {/if}
      </div>

      <div class="field">
        <label for="fd-name">Name</label>
        <input id="fd-name" class="input" bind:value={form.name} oninput={() => (nameTouched = true)} placeholder="Survey photos 2026" />
      </div>

      <div class="field">
        <span class="label">Upload with</span>
        <div class="tools">
          <button class="tool" class:selected={(form.options.upload_tool || 'gcloud') === 'gcloud'} onclick={() => (form.options.upload_tool = 'gcloud')}>
            <Cloud size={18} />
            <span><strong>gcloud storage</strong><small>Google's standard tool. Recommended.</small></span>
          </button>
          <button
            class="tool"
            class:selected={form.options.upload_tool === 'jetstream'}
            disabled={!engineAvailable}
            onclick={() => (form.options.upload_tool = 'jetstream')}
          >
            <Zap size={18} />
            <span>
              <strong>JetStream engine <em>beta</em></strong>
              <small>{engineAvailable ? 'Instant re-syncs, resumes large files.' : 'Not installed on this computer.'}</small>
            </span>
          </button>
        </div>
      </div>

      <div class="toggles">
        <label class="toggle">
          <input type="checkbox" checked={form.options.recursive !== false} onchange={(e) => (form.options.recursive = e.currentTarget.checked)} />
          <span><strong>Include subfolders</strong></span>
        </label>
        <label class="toggle">
          <input type="checkbox" bind:checked={form.options.no_clobber} />
          <span><strong>Never overwrite files already in the cloud</strong><small>Changed local files are skipped if a copy already exists.</small></span>
        </label>
      </div>

      <div class="field">
        <span class="label">Sync automatically</span>
        <div class="segmented" role="radiogroup" aria-label="Sync automatically">
          {#each [['off', 'Off'], ['daily', 'At a set time'], ['hourly', 'Every few hours']] as [mode, label] (mode)}
            <button role="radio" aria-checked={sched.mode === mode} class:selected={sched.mode === mode} onclick={() => (sched.mode = mode)}>{label}</button>
          {/each}
        </div>
        {#if sched.mode === 'daily'}
          <div class="schedule-row">
            <input type="time" class="input time" aria-label="Time of day" bind:value={sched.time} required />
            <div class="chips days">
              {#each DAY_NAMES as name, day (day)}
                <button class="chip" class:on={sched.days.includes(day)} aria-pressed={sched.days.includes(day)} onclick={() => toggleDay(day)}>{name}</button>
              {/each}
            </div>
          </div>
        {:else if sched.mode === 'hourly'}
          <div class="schedule-row">
            <span>Every</span>
            <select class="input hours" aria-label="Hours between syncs" bind:value={sched.every_hours}>
              {#each [1, 2, 3, 4, 6, 8, 12] as n (n)}<option value={n}>{n}</option>{/each}
            </select>
            <span>{sched.every_hours === 1 ? 'hour' : 'hours'}</span>
          </div>
        {/if}
        {#if sched.mode !== 'off'}
          <label class="toggle">
            <input type="checkbox" bind:checked={sched.preview} />
            <span><strong>Preview only</strong><small>Scheduled runs check what would upload without uploading.</small></span>
          </label>
          <span class="hint">Runs while JetStream is open (it keeps running in the tray when the window is closed).</span>
        {/if}
      </div>

      <button class="link" onclick={() => (showFilters = !showFilters)}>{showFilters ? 'Hide' : 'Skip some files…'}</button>
      {#if showFilters}
        <div class="filters">
          <div class="chips">
            {#each Object.keys(FILTER_PRESETS) as name (name)}
              <button class="chip" onclick={() => addFilter(name)}>+ {name}</button>
            {/each}
          </div>
          <div class="field">
            <label for="fd-patterns">Skip files matching (one per line)</label>
            <textarea id="fd-patterns" class="input" rows="3" bind:value={form.options.exclude_patterns} placeholder={'*.tmp\nThumbs.db'}></textarea>
          </div>
          <div class="field">
            <label for="fd-folders">Skip folders named (comma-separated)</label>
            <input id="fd-folders" class="input" bind:value={form.options.exclude_folders} placeholder="_archive, MISC" />
          </div>
        </div>
      {/if}

      {#if error}<div class="error">{error}</div>{/if}
    </div>

    <footer>
      <button class="btn ghost" onclick={close}>Cancel</button>
      <button class="btn" disabled={!canSave} onclick={() => save(false)}>Save</button>
      <button class="btn primary" disabled={!canSave} onclick={() => save(true)}>Save &amp; preview</button>
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
    animation: fade 0.12s ease-out;
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  .dialog {
    width: min(620px, 100%);
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
    gap: 18px;
  }
  .with-button {
    display: flex;
    gap: 8px;
  }
  .dest-input {
    display: flex;
    align-items: stretch;
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
    font-weight: 600;
    color: var(--text-2);
    cursor: pointer;
  }
  .chip:hover {
    border-color: var(--accent);
    color: var(--accent);
  }
  .tools {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  .tool {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 12px;
    border-radius: var(--radius-sm);
    border: 1.5px solid var(--border);
    background: var(--surface);
    text-align: left;
    cursor: pointer;
    color: var(--text-2);
  }
  .tool span {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .tool strong {
    color: var(--text);
  }
  .tool em {
    font-style: normal;
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--accent);
    margin-left: 4px;
  }
  .tool small,
  .toggle small {
    color: var(--muted);
    font-size: 12.5px;
  }
  .tool.selected {
    border-color: var(--accent);
    background: var(--accent-soft);
    color: var(--accent);
  }
  .tool:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }
  .toggles {
    display: flex;
    flex-direction: column;
    gap: 10px;
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
  .link {
    align-self: flex-start;
    border: none;
    background: none;
    padding: 0;
    color: var(--accent);
    font-weight: 600;
    cursor: pointer;
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
    padding: 6px 12px;
    border-radius: 6px;
    cursor: pointer;
    color: var(--text-2);
    font: inherit;
  }
  .segmented button.selected {
    background: var(--surface);
    color: var(--text);
    font-weight: 600;
    box-shadow: var(--shadow-sm, 0 1px 2px rgba(0, 0, 0, 0.12));
  }
  .schedule-row {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    margin-top: 4px;
  }
  .input.time {
    width: 120px;
  }
  .input.hours {
    width: 76px;
  }
  .days .chip.on {
    background: var(--accent-soft);
    border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
    color: var(--accent);
    font-weight: 600;
  }
  .hint {
    color: var(--muted);
    font-size: 12.5px;
  }
  .filters {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 14px;
    border-radius: var(--radius-sm);
    background: var(--surface-2);
  }
  .bad,
  .error {
    color: var(--danger);
  }
  .error {
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    background: var(--danger-soft);
  }
</style>
