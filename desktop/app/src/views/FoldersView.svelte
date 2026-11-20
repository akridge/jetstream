<script>
  import { onMount, tick } from 'svelte';
  import FolderPlus from '@lucide/svelte/icons/folder-plus';
  import FolderSync from '@lucide/svelte/icons/folder-sync';
  import FolderInput from '@lucide/svelte/icons/folder-input';
  import CloudUpload from '@lucide/svelte/icons/cloud-upload';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import ListChecks from '@lucide/svelte/icons/list-checks';
  import Search from '@lucide/svelte/icons/search';
  import X from '@lucide/svelte/icons/x';
  import LayoutGrid from '@lucide/svelte/icons/layout-grid';
  import Rows3 from '@lucide/svelte/icons/rows-3';
  import Tag from '@lucide/svelte/icons/tag';
  import ShieldCheck from '@lucide/svelte/icons/shield-check';
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
  import HardDrive from '@lucide/svelte/icons/hard-drive-upload';
  import Keyboard from '@lucide/svelte/icons/keyboard';
  import FolderCard from '../components/FolderCard.svelte';
  import FolderRow from '../components/FolderRow.svelte';
  import WelcomeCard from '../components/WelcomeCard.svelte';
  import { app, ACTIVE, TERMINAL } from '../lib/state.svelte.js';
  import { api } from '../lib/api.js';
  import { folderStatus } from '../lib/status.svelte.js';
  import { allTags, folderState, isHidden, tagsOf, updateOptions } from '../lib/folders.svelte.js';
  import { bytes, count, normalizeDestination, relativeTime } from '../lib/format.js';
  import { isDesktop, pickFolder } from '../lib/platform.js';

  let running = $derived(app.jobList.filter((j) => ACTIVE.has(j.status)).length);

  // Getting started: for people who haven't uploaded anything yet (not shown to people who
  // already have, unless they saw it before finishing).
  let firstUploadDone = $derived(app.presets.length > 0 && app.jobList.some((j) => TERMINAL.has(j.status) && j.status === 'completed' && !j.dry_run && !j.filters?.job_type));
  let showWelcome = $derived(app.loaded && !app.prefs.welcome_done && (!firstUploadDone || !!app.prefs.welcome_shown));
  $effect(() => {
    if (showWelcome && !firstUploadDone && !app.prefs.welcome_shown) app.savePrefs({ welcome_shown: true });
  });

  async function add() {
    const path = await pickFolder();
    if (path) app.addFolder(path);
  }

  // ----- what's shown -----
  const FILTERS = [
    ['all', 'All'],
    ['attention', 'Needs attention'],
    ['new', 'New files'],
    ['syncing', 'Syncing'],
    ['ok', 'Up to date'],
  ];
  const SORTS = [
    ['status', 'What needs me first'],
    ['name', 'Name'],
    ['synced', 'Last synced'],
    ['new', 'Most new files'],
  ];
  let q = $derived(app.folderQuery);
  let view = $derived(app.prefs.folders_view === 'list' ? 'list' : 'cards');
  let sort = $derived(SORTS.some(([id]) => id === app.prefs.folders_sort) ? app.prefs.folders_sort : 'status');
  let states = $derived(new Map(app.presets.map((p) => [p.preset_id, folderState(p)])));
  let visible = $derived(app.presets.filter((p) => !isHidden(p)));
  let hiddenCount = $derived(app.presets.length - visible.length);
  let tags = $derived(allTags(app.presets));
  let counts = $derived.by(() => {
    const c = { all: visible.length, attention: 0, new: 0, syncing: 0, ok: 0, hidden: hiddenCount };
    for (const p of visible) c[states.get(p.preset_id).kind] = (c[states.get(p.preset_id).kind] || 0) + 1;
    return c;
  });
  let shown = $derived.by(() => {
    const words = q.text.trim().toLowerCase().split(/\s+/).filter(Boolean);
    let list = q.filter === 'hidden' ? app.presets.filter(isHidden) : visible;
    if (q.filter !== 'all' && q.filter !== 'hidden') list = list.filter((p) => states.get(p.preset_id).kind === q.filter);
    if (q.tag) list = list.filter((p) => tagsOf(p).includes(q.tag));
    if (words.length) {
      list = list.filter((p) => {
        const hay = `${p.name} ${p.source_path} gs://${normalizeDestination(p.destination)} ${tagsOf(p).join(' ')}`.toLowerCase();
        return words.every((w) => hay.includes(w));
      });
    }
    const by = {
      name: (a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: 'base', numeric: true }),
      synced: (a, b) => (states.get(b.preset_id).lastSynced || 0) - (states.get(a.preset_id).lastSynced || 0),
      new: (a, b) => states.get(b.preset_id).changes - states.get(a.preset_id).changes,
      status: (a, b) => states.get(a.preset_id).rank - states.get(b.preset_id).rank,
    }[sort];
    return [...list].sort((a, b) => by(a, b) || a.name.localeCompare(b.name, undefined, { sensitivity: 'base', numeric: true }));
  });
  let filtering = $derived(!!(q.text.trim() || q.filter !== 'all' || q.tag));
  const setQuery = (changes) => (app.folderQuery = { ...app.folderQuery, ...changes });
  function clearFilters() {
    app.folderQuery = { text: '', filter: 'all', tag: null };
  }

  // ----- summary -----
  let backedUp = $derived(visible.reduce((n, p) => n + (states.get(p.preset_id).status?.same_bytes || 0), 0));
  let verifiedFolders = $derived(visible.filter((p) => app.receipts[p.preset_id]?.result === 'complete').length);
  let lastVerified = $derived(
    visible.map((p) => app.receipts[p.preset_id]?.finished_at).filter(Boolean).sort().pop() || null,
  );

  // Folders with new files waiting (from the background checks), hidden ones left out.
  let pending = $derived(folderStatus.pending.filter((p) => !isHidden(p.preset)));
  let pendingFiles = $derived(pending.reduce((n, p) => n + p.status.changes, 0));
  let pendingBytes = $derived(pending.reduce((n, p) => n + p.status.bytes, 0));
  let anyChecking = $derived(Object.keys(folderStatus.checking).length > 0);

  async function uploadAll(list = pending.map((p) => p.preset)) {
    app.toast(`Uploading ${count(list.length)} ${list.length === 1 ? 'folder' : 'folders'}, one after another`, 'info', { timeout: 4000 });
    for (const preset of list) await app.runPreset(preset, false, { quiet: true });
  }

  // ----- selecting several -----
  let selecting = $state(false);
  let selected = $state({});
  let chosen = $derived(app.presets.filter((p) => selected[p.preset_id]));
  function toggle(preset) {
    selected = { ...selected, [preset.preset_id]: !selected[preset.preset_id] };
  }
  function selectAll(on) {
    selected = on ? Object.fromEntries(shown.map((p) => [p.preset_id, true])) : {};
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
    for (const preset of chosen) await updateOptions(preset, { auto_upload: on });
    await app.refreshPresets();
    app.toast(`Auto-upload ${on ? 'on' : 'off'} for ${count(chosen.length)} folders`, 'success', { timeout: 3000 });
    if (on) for (const preset of chosen) folderStatus.check(preset);
  }
  async function hideChosen(on) {
    const list = chosen;
    for (const preset of list) await updateOptions(preset, { hidden: on });
    await app.refreshPresets();
    stopSelecting();
    app.toast(`${count(list.length)} ${list.length === 1 ? 'folder' : 'folders'} ${on ? 'hidden' : 'shown again'}`, 'info', {
      timeout: 6000,
      action: {
        label: 'Undo',
        run: async () => {
          for (const preset of list) await updateOptions({ ...preset, options: { ...preset.options, hidden: on } }, { hidden: !on });
          await app.refreshPresets();
        },
      },
    });
  }
  async function removeChosen() {
    const n = chosen.length;
    if (!window.confirm(`Stop syncing ${n} ${n === 1 ? 'folder' : 'folders'}? Nothing is deleted from this computer or the cloud.`)) return;
    for (const preset of chosen) await app.removePreset(preset);
    stopSelecting();
  }

  // ----- keyboard -----
  let searchEl = $state();
  let focusId = $state(null);
  let expanded = $state({});
  let showKeys = $state(false);
  let focusIndex = $derived(shown.findIndex((p) => p.preset_id === focusId));

  async function moveFocus(delta) {
    if (!shown.length) return;
    const columns = view === 'cards' ? Math.max(1, Math.round((document.querySelector('.grid')?.clientWidth || 340) / 356)) : 1;
    const step = Math.abs(delta) === 2 ? columns * Math.sign(delta) : Math.sign(delta);
    const next = focusIndex < 0 ? 0 : Math.min(shown.length - 1, Math.max(0, focusIndex + step));
    focusId = shown[next].preset_id;
    await tick();
    document.querySelector(`[data-folder="${focusId}"]`)?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }

  function keydown(e) {
    if (app.view !== 'folders' || e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey) return;
    const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(e.target?.tagName) || e.target?.isContentEditable;
    if (document.querySelector('[role=dialog]')) return;
    if (typing) {
      if (e.key === 'Escape' && e.target === searchEl) {
        if (q.text) setQuery({ text: '' });
        else searchEl.blur();
      } else if (e.key === 'ArrowDown' && e.target === searchEl) {
        e.preventDefault();
        searchEl.blur();
        moveFocus(1);
      }
      return;
    }
    const focused = shown.find((p) => p.preset_id === focusId);
    const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
    const act = {
      '/': () => searchEl?.focus(),
      ArrowDown: () => moveFocus(view === 'cards' ? 2 : 1),
      ArrowUp: () => moveFocus(view === 'cards' ? -2 : -1),
      ArrowRight: () => view === 'cards' && moveFocus(1),
      ArrowLeft: () => view === 'cards' && moveFocus(-1),
      j: () => moveFocus(1),
      k: () => moveFocus(-1),
      Enter: () => focused && (view === 'list' ? (expanded = { ...expanded, [focusId]: !expanded[focusId] }) : app.compareFolder(focused)),
      s: () => focused && app.runPreset(focused, false),
      p: () => focused && app.runPreset(focused, true),
      v: () => focused && app.createReceipt(focused),
      e: () => focused && app.editFolder(focused),
      t: () => focused && (app.tagsSheet = [focused]),
      Escape: () => (focusId ? (focusId = null) : filtering && clearFilters()),
      '?': () => (showKeys = !showKeys),
    }[key];
    if (act) {
      e.preventDefault();
      act();
    }
  }

  onMount(() => {
    if (!app.prefs.folders_sort) app.savePrefs({ folders_sort: 'status' });
  });
</script>

<svelte:window onkeydown={keydown} />

<section class="view">
  <header class="view-head">
    <div>
      <h1>Synced folders</h1>
      <p class="sub">
        {#if app.presets.length}
          {count(visible.length)} folder{visible.length === 1 ? '' : 's'}{hiddenCount ? ` · ${count(hiddenCount)} hidden` : ''}
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

  {#if showWelcome}
    <WelcomeCard />
  {/if}

  {#if app.loaded && visible.length > 1}
    <div class="summary">
      <div class="tile">
        <HardDrive size={18} />
        <div><strong>{bytes(backedUp)}</strong><span>in the cloud and up to date</span></div>
      </div>
      <div class="tile">
        <ShieldCheck size={18} />
        <div>
          <strong>{count(verifiedFolders)} of {count(visible.length)}</strong>
          <span>folders verified{lastVerified ? ` · last ${relativeTime(lastVerified)}` : ''}</span>
        </div>
      </div>
      {#if counts.attention}
        <button class="tile warn" onclick={() => setQuery({ filter: 'attention' })}>
          <TriangleAlert size={18} />
          <div><strong>{count(counts.attention)} {counts.attention === 1 ? 'folder needs' : 'folders need'} you</strong><span>Show them ›</span></div>
        </button>
      {:else}
        <div class="tile ok">
          <ShieldCheck size={18} />
          <div><strong>All good</strong><span>{counts.syncing ? `${count(counts.syncing)} syncing now` : counts.new ? `${count(counts.new)} with new files` : 'Nothing needs you'}</span></div>
        </div>
      {/if}
    </div>
  {/if}

  {#if selecting}
    <div class="bar card">
      <strong>{count(chosen.length)} selected</strong>
      <button class="link" onclick={() => selectAll(chosen.length < shown.length)}>{chosen.length < shown.length ? `Select all ${count(shown.length)}` : 'Select none'}</button>
      <div class="push actions">
        <button class="btn primary sm" disabled={!chosen.length} onclick={() => uploadAll(chosen)}><CloudUpload size={15} /> Upload</button>
        <button class="btn sm" disabled={!chosen.length} onclick={previewChosen}>Preview</button>
        <button class="btn sm" disabled={!chosen.length} onclick={() => chosen.forEach((p) => folderStatus.check(p))}>Check for changes</button>
        <button class="btn sm" disabled={!chosen.length} onclick={() => (app.tagsSheet = chosen)}><Tag size={14} /> Tags…</button>
        <button class="btn sm" disabled={!chosen.length} onclick={() => setAuto(true)}>Auto-upload on</button>
        <button class="btn sm" disabled={!chosen.length} onclick={() => setAuto(false)}>Off</button>
        {#if q.filter === 'hidden'}
          <button class="btn sm" disabled={!chosen.length} onclick={() => hideChosen(false)}>Show again</button>
        {:else}
          <button class="btn sm" disabled={!chosen.length} onclick={() => hideChosen(true)}>Hide</button>
        {/if}
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
  {/if}

  {#if app.loaded && (app.presets.length > 1 || hiddenCount)}
    <div class="toolbar">
      <label class="search">
        <Search size={15} />
        <input bind:this={searchEl} placeholder="Search folders" value={q.text} oninput={(e) => setQuery({ text: e.currentTarget.value })} aria-label="Search folders" />
        {#if q.text}<button class="clear" aria-label="Clear search" onclick={() => setQuery({ text: '' })}><X size={13} /></button>{:else}<kbd>/</kbd>{/if}
      </label>
      <div class="chips" role="radiogroup" aria-label="Show">
        {#each FILTERS as [id, label] (id)}
          {#if id === 'all' || counts[id]}
            <button class="chip" role="radio" aria-checked={q.filter === id} class:on={q.filter === id} onclick={() => setQuery({ filter: id })}>
              {label}<span class="n">{count(counts[id])}</span>
            </button>
          {/if}
        {/each}
        {#if hiddenCount}
          <button class="chip" role="radio" aria-checked={q.filter === 'hidden'} class:on={q.filter === 'hidden'} onclick={() => setQuery({ filter: 'hidden' })}>
            Hidden<span class="n">{count(hiddenCount)}</span>
          </button>
        {/if}
      </div>
      <div class="right">
        <label class="sort">
          <span class="muted">Sort</span>
          <select value={sort} onchange={(e) => app.savePrefs({ folders_sort: e.currentTarget.value })} aria-label="Sort folders">
            {#each SORTS as [id, label] (id)}<option value={id}>{label}</option>{/each}
          </select>
        </label>
        <div class="segmented" role="radiogroup" aria-label="Layout">
          <button role="radio" aria-checked={view === 'cards'} class:active={view === 'cards'} title="Cards" aria-label="Cards" onclick={() => app.savePrefs({ folders_view: 'cards' })}><LayoutGrid size={15} /></button>
          <button role="radio" aria-checked={view === 'list'} class:active={view === 'list'} title="Compact list" aria-label="Compact list" onclick={() => app.savePrefs({ folders_view: 'list' })}><Rows3 size={15} /></button>
        </div>
        <button class="btn ghost sm icon" class:on={showKeys} title="Keyboard shortcuts (?)" aria-label="Keyboard shortcuts" onclick={() => (showKeys = !showKeys)}><Keyboard size={16} /></button>
      </div>
    </div>
    {#if tags.length}
      <div class="tagbar">
        <Tag size={13} />
        {#each tags as [t, n] (t)}
          <button class="tagchip" class:on={q.tag === t} aria-pressed={q.tag === t} onclick={() => setQuery({ tag: q.tag === t ? null : t })}>{t} <span class="n">{n}</span></button>
        {/each}
      </div>
    {/if}
    {#if showKeys}
      <div class="keys" aria-label="Keyboard shortcuts">
        <span><kbd>/</kbd> search</span><span><kbd>↑</kbd><kbd>↓</kbd> move</span><span><kbd>Enter</kbd> {view === 'list' ? 'expand' : 'compare'}</span>
        <span><kbd>S</kbd> sync</span><span><kbd>P</kbd> preview</span><span><kbd>V</kbd> verify</span><span><kbd>E</kbd> edit</span><span><kbd>T</kbd> tags</span>
        <span><kbd>Ctrl</kbd><kbd>K</kbd> do anything</span><span><kbd>Esc</kbd> clear</span>
      </div>
    {/if}
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
  {:else if !shown.length}
    <div class="none card">
      <p><strong>No folders match</strong></p>
      <p class="muted">
        {q.filter === 'attention' ? 'Nothing needs you right now.' : q.text ? `Nothing matches “${q.text}”.` : 'Try another filter.'}
      </p>
      <button class="btn sm" onclick={clearFilters}>Show all folders</button>
    </div>
  {:else if view === 'list'}
    <div class="list card" role="table" aria-label="Synced folders">
      {#each shown as preset (preset.preset_id)}
        <FolderRow
          {preset}
          {selecting}
          selected={!!selected[preset.preset_id]}
          onselect={() => toggle(preset)}
          expanded={!!expanded[preset.preset_id]}
          focused={focusId === preset.preset_id}
          onfocus={() => (focusId = preset.preset_id)}
          ontoggle={() => (expanded = { ...expanded, [preset.preset_id]: !expanded[preset.preset_id] })}
        />
      {/each}
    </div>
    {#if filtering}<p class="foot muted">Showing {count(shown.length)} of {count(app.presets.length)} · <button class="link" onclick={clearFilters}>Show all</button></p>{/if}
  {:else}
    <div class="grid">
      {#each shown as preset (preset.preset_id)}
        <FolderCard {preset} {selecting} focused={focusId === preset.preset_id} selected={!!selected[preset.preset_id]} onselect={() => toggle(preset)} />
      {/each}
      {#if !filtering}
        <button class="add-tile" onclick={add}>
          <FolderPlus size={22} />
          <span>Add folder</span>
          {#if isDesktop}<small>or drop one here</small>{/if}
        </button>
      {/if}
    </div>
    {#if filtering}<p class="foot muted">Showing {count(shown.length)} of {count(app.presets.length)} · <button class="link" onclick={clearFilters}>Show all</button></p>{/if}
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
    margin-bottom: 18px;
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
  .summary {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 12px;
    margin-bottom: 14px;
  }
  .tile {
    display: flex;
    gap: 12px;
    align-items: center;
    padding: 12px 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--accent);
    text-align: left;
    font: inherit;
  }
  .tile div {
    display: flex;
    flex-direction: column;
    min-width: 0;
    color: var(--text);
  }
  .tile strong {
    font-size: 16px;
    font-variant-numeric: tabular-nums;
  }
  .tile span {
    font-size: 12.5px;
    color: var(--muted);
  }
  .tile.ok {
    color: var(--success);
  }
  .tile.warn {
    color: var(--warning);
    background: var(--warning-soft);
    border-color: color-mix(in srgb, var(--warning) 30%, var(--border));
    cursor: pointer;
  }
  .tile.warn span {
    color: var(--warning);
    font-weight: 600;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
    margin-bottom: 14px;
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
  .toolbar {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    margin-bottom: 10px;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 34px;
    padding: 0 8px 0 10px;
    width: 240px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    background: var(--surface);
    color: var(--muted);
  }
  .search:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .search input {
    flex: 1;
    min-width: 0;
    border: none;
    outline: none;
    background: none;
    font: inherit;
    color: var(--text);
  }
  .clear {
    border: none;
    background: none;
    color: var(--muted);
    cursor: pointer;
    display: grid;
    padding: 2px;
  }
  kbd {
    font-family: var(--mono);
    font-size: 11px;
    padding: 1px 5px;
    border: 1px solid var(--border-strong);
    border-bottom-width: 2px;
    border-radius: 4px;
    color: var(--text-2);
    background: var(--surface);
  }
  .chips {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 30px;
    padding: 0 12px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text-2);
    font: inherit;
    font-size: 13px;
    cursor: pointer;
  }
  .chip:hover {
    border-color: var(--border-strong);
  }
  .chip.on {
    background: var(--text);
    border-color: var(--text);
    color: var(--surface);
  }
  .chip .n {
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
    opacity: 0.7;
  }
  .right {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .sort {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
  }
  .sort select {
    height: 30px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--surface);
    color: var(--text);
    font: inherit;
    font-size: 13px;
    padding: 0 6px;
  }
  .segmented {
    display: inline-flex;
    padding: 2px;
    border-radius: 8px;
    background: var(--surface-2);
    border: 1px solid var(--border);
  }
  .segmented button {
    display: grid;
    place-items: center;
    border: none;
    background: none;
    color: var(--muted);
    padding: 4px 8px;
    border-radius: 6px;
    cursor: pointer;
  }
  .segmented button.active {
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow);
  }
  .btn.on {
    color: var(--accent);
  }
  .tagbar {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    margin: -2px 0 12px;
    color: var(--muted);
  }
  .tagchip {
    height: 24px;
    padding: 0 10px;
    border-radius: 999px;
    border: none;
    background: var(--accent-soft);
    color: var(--accent);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  .tagchip.on {
    background: var(--accent);
    color: #fff;
  }
  .tagchip .n {
    opacity: 0.7;
  }
  .keys {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 16px;
    margin: 0 0 12px;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .keys kbd {
    margin-right: 2px;
  }
  .link {
    border: none;
    background: none;
    padding: 0;
    color: var(--accent);
    font-weight: 600;
    cursor: pointer;
    font: inherit;
  }
  .danger {
    color: var(--danger);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(340px, 1fr));
    gap: 16px;
  }
  .list {
    overflow: hidden;
  }
  .none {
    padding: 28px;
    text-align: center;
  }
  .none p {
    margin: 0 0 6px;
  }
  .foot {
    margin: 12px 2px 0;
    font-size: 13px;
  }
  .muted {
    color: var(--muted);
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
  @media (max-width: 900px) {
    .summary {
      grid-template-columns: 1fr;
    }
    .right {
      margin-left: 0;
    }
  }
</style>
