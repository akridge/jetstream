<script>
  // Ctrl+K / ⌘K: jump anywhere or do anything by typing. Every view, every synced folder's
  // actions, recent buckets and the app-wide actions, filtered as you type.
  import { onMount, tick } from 'svelte';
  import Search from '@lucide/svelte/icons/search';
  import FolderSync from '@lucide/svelte/icons/folder-sync';
  import Activity from '@lucide/svelte/icons/activity';
  import Cloud from '@lucide/svelte/icons/cloud';
  import LibraryBig from '@lucide/svelte/icons/library-big';
  import Brush from '@lucide/svelte/icons/brush';
  import FolderSearch from '@lucide/svelte/icons/folder-search';
  import ChartColumn from '@lucide/svelte/icons/chart-column';
  import Settings from '@lucide/svelte/icons/settings';
  import Play from '@lucide/svelte/icons/play';
  import Eye from '@lucide/svelte/icons/eye';
  import ShieldCheck from '@lucide/svelte/icons/shield-check';
  import GitCompare from '@lucide/svelte/icons/git-compare-arrows';
  import Pencil from '@lucide/svelte/icons/pencil';
  import FileCheck from '@lucide/svelte/icons/file-check';
  import Tag from '@lucide/svelte/icons/tag';
  import FolderPlus from '@lucide/svelte/icons/folder-plus';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import CloudUpload from '@lucide/svelte/icons/cloud-upload';
  import Gauge from '@lucide/svelte/icons/gauge';
  import SunMoon from '@lucide/svelte/icons/sun-moon';
  import CornerDownLeft from '@lucide/svelte/icons/corner-down-left';
  import Sparkles from '@lucide/svelte/icons/sparkles';
  import { app } from '../lib/state.svelte.js';
  import { folderStatus } from '../lib/status.svelte.js';
  import { folderState } from '../lib/folders.svelte.js';
  import { isDesktop, pickFolder } from '../lib/platform.js';
  import { normalizeDestination } from '../lib/format.js';

  let query = $state('');
  let index = $state(0);
  let input = $state();
  let listEl = $state();

  const go = (view) => () => (app.view = view);
  // The theme showing now (the system's when set to follow it).
  let dark = $derived(app.prefs.theme === 'dark' || (app.prefs.theme !== 'light' && window.matchMedia?.('(prefers-color-scheme: dark)').matches));
  function openCloud(path) {
    const [bucket, ...rest] = normalizeDestination(path).split('/');
    app.openCloud(bucket, rest.filter(Boolean).length ? `${rest.filter(Boolean).join('/')}/` : '');
  }

  let commands = $derived.by(() => {
    const list = [
      { section: 'Go to', label: 'Synced folders', icon: FolderSync, run: go('folders') },
      { section: 'Go to', label: 'Activity', icon: Activity, run: go('activity') },
      { section: 'Go to', label: 'Cloud storage', icon: Cloud, run: go('cloud') },
      { section: 'Go to', label: 'Catalogs', icon: LibraryBig, run: go('catalogs') },
      { section: 'Go to', label: 'Cloud cleanup', icon: Brush, run: go('audit') },
      { section: 'Go to', label: 'Folder insights', icon: FolderSearch, run: go('insights') },
      { section: 'Go to', label: 'Analytics', icon: ChartColumn, run: go('analytics') },
      { section: 'Go to', label: 'Settings', icon: Settings, run: go('settings') },
      { section: 'Actions', label: 'Add a folder…', icon: FolderPlus, run: async () => { const p = await pickFolder(); if (p) app.addFolder(p); } },
      { section: 'Actions', label: 'Add several folders…', icon: FolderPlus, run: () => app.addFolders() },
      { section: 'Actions', label: 'Check all folders for changes', icon: RefreshCw, run: () => folderStatus.checkAll() },
      ...(folderStatus.pending.length
        ? [{ section: 'Actions', label: `Upload all new files (${folderStatus.pending.length} folders)`, icon: CloudUpload, run: async () => { for (const p of folderStatus.pending) await app.runPreset(p.preset, false, { quiet: true }); } }]
        : []),
      ...(isDesktop ? [{ section: 'Actions', label: 'Speed check…', icon: Gauge, run: () => (app.speedCheck = {}) }] : []),
      { section: 'Actions', label: 'Make a catalog…', icon: LibraryBig, run: go('catalogs') },
      ...(app.desktop ? [{ section: 'Actions', label: `What's new in ${app.desktop.version}`, icon: Sparkles, run: () => (app.whatsNew = app.desktop.version) }] : []),
      { section: 'Actions', label: `Switch to ${dark ? 'light' : 'dark'} theme`, icon: SunMoon, run: () => app.savePrefs({ theme: dark ? 'light' : 'dark' }) },
    ];
    for (const p of app.presets) {
      const st = folderState(p);
      const hint = st.label;
      const dest = `gs://${normalizeDestination(p.destination)}/`;
      list.push(
        { section: p.name, label: `Sync ${p.name}`, hint, icon: Play, run: () => app.runPreset(p, false), weight: st.kind === 'new' ? 2 : 0, folder: true },
        { section: p.name, label: `Preview ${p.name}`, icon: Eye, run: () => app.runPreset(p, true), folder: true },
        { section: p.name, label: `Verify ${p.name}`, icon: ShieldCheck, run: () => app.createReceipt(p), folder: true },
        { section: p.name, label: `Receipt for ${p.name}`, icon: FileCheck, run: () => app.openReceipt(p), weight: st.unverified ? 3 : 0, folder: true },
        { section: p.name, label: `Compare ${p.name} with the cloud`, icon: GitCompare, run: () => app.compareFolder(p), folder: true },
        { section: p.name, label: `Open ${p.name} in Cloud storage`, hint: dest, icon: Cloud, run: () => openCloud(dest), folder: true },
        { section: p.name, label: `Edit ${p.name}`, icon: Pencil, run: () => app.editFolder(p), folder: true },
        { section: p.name, label: `Tags for ${p.name}`, icon: Tag, run: () => (app.tagsSheet = [p]), folder: true },
      );
    }
    for (const s of app.prefs.starred || []) list.push({ section: 'Starred', label: `Open gs://${s.bucket}/${s.prefix}`, icon: Cloud, run: () => app.openCloud(s.bucket, s.prefix) });
    for (const b of app.prefs.recent_buckets || []) list.push({ section: 'Buckets', label: `Open gs://${b}`, icon: Cloud, run: () => openCloud(b) });
    return list;
  });

  /** Every word must appear; the label starting with the query, or words starting it, rank higher. */
  let results = $derived.by(() => {
    const text = query.trim().toLowerCase();
    if (/^gs:\/\/\S+/.test(text)) {
      return [{ section: 'Cloud storage', label: `Open ${query.trim()}`, icon: Cloud, run: () => openCloud(query.trim()) }];
    }
    if (!text) return commands.filter((c) => !c.folder || c.weight).slice(0, 24);
    const words = text.split(/\s+/);
    return commands
      .map((c) => {
        const label = c.label.toLowerCase();
        if (!words.every((w) => label.includes(w) || (c.hint || '').toLowerCase().includes(w))) return null;
        let score = c.weight || 0;
        if (label.startsWith(text)) score += 10;
        score += words.filter((w) => new RegExp(`(^|[\\s/_-])${w.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`).test(label)).length * 3;
        return { ...c, score };
      })
      .filter(Boolean)
      .sort((a, b) => b.score - a.score)
      .slice(0, 40);
  });

  $effect(() => {
    void query;
    index = 0;
  });

  onMount(() => input?.focus());

  function close() {
    app.paletteOpen = false;
  }

  function run(command) {
    close();
    command?.run();
  }

  async function keydown(e) {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      index = (index + (e.key === 'ArrowDown' ? 1 : -1) + results.length) % Math.max(1, results.length);
      await tick();
      listEl?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' });
    } else if (e.key === 'Enter') {
      e.preventDefault();
      run(results[index]);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      close();
    }
  }
</script>

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && close()}>
  <div class="palette card" role="dialog" aria-modal="true" aria-label="Command palette">
    <label class="search">
      <Search size={18} />
      <input
        bind:this={input}
        bind:value={query}
        onkeydown={keydown}
        placeholder="Type a folder, a view, or an action…"
        role="combobox"
        aria-expanded="true"
        aria-controls="palette-list"
        aria-activedescendant={results.length ? `palette-${index}` : undefined}
      />
      <kbd>Esc</kbd>
    </label>
    <div class="list" id="palette-list" role="listbox" bind:this={listEl}>
      {#each results as c, i (c.section + c.label)}
        {#if i === 0 || results[i - 1].section !== c.section}<div class="section">{c.section}</div>{/if}
        {@const Icon = c.icon}
        <button
          id="palette-{i}"
          class="item"
          role="option"
          aria-selected={i === index}
          onmousemove={() => (index = i)}
          onclick={() => run(c)}
        >
          <Icon size={16} />
          <span class="label truncate">{c.label}</span>
          {#if c.hint}<span class="hint truncate">{c.hint}</span>{/if}
          {#if i === index}<span class="enter"><CornerDownLeft size={13} /></span>{/if}
        </button>
      {:else}
        <p class="empty">Nothing matches “{query}”.</p>
      {/each}
    </div>
    <div class="foot"><kbd>↑</kbd><kbd>↓</kbd> choose <kbd>Enter</kbd> run <kbd>Esc</kbd> close</div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 60;
    background: rgba(8, 15, 25, 0.35);
    backdrop-filter: blur(2px);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding: 12vh 24px 24px;
    animation: fade 0.12s ease-out;
  }
  .palette {
    width: min(620px, 100%);
    max-height: 70vh;
    display: flex;
    flex-direction: column;
    box-shadow: var(--shadow-lg);
    overflow: hidden;
    animation: drop 0.16s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  @keyframes drop {
    from {
      opacity: 0;
      transform: translateY(-6px) scale(0.99);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .backdrop,
    .palette {
      animation: none;
    }
  }
  .search {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 16px;
    border-bottom: 1px solid var(--border);
    color: var(--muted);
  }
  .search input {
    flex: 1;
    border: none;
    outline: none;
    background: none;
    font: inherit;
    font-size: 16px;
    color: var(--text);
  }
  .list {
    overflow-y: auto;
    padding: 6px;
  }
  .section {
    padding: 8px 10px 4px;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted);
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 10px;
    border: none;
    border-radius: 8px;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 14px;
    text-align: left;
    cursor: pointer;
  }
  .item :global(svg) {
    flex: none;
    color: var(--muted);
  }
  .item[aria-selected='true'] {
    background: var(--accent-soft);
  }
  .item[aria-selected='true'] :global(svg) {
    color: var(--accent);
  }
  .label {
    flex: none;
    max-width: 65%;
  }
  .hint {
    flex: 1;
    min-width: 0;
    color: var(--muted);
    font-size: 12.5px;
  }
  .enter {
    margin-left: auto;
    color: var(--muted);
    display: grid;
  }
  .empty {
    padding: 18px;
    text-align: center;
    color: var(--muted);
    margin: 0;
  }
  .foot {
    display: flex;
    gap: 6px;
    align-items: center;
    padding: 8px 14px;
    border-top: 1px solid var(--border);
    font-size: 12px;
    color: var(--muted);
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
</style>
