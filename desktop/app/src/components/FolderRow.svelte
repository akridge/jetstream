<script>
  // A synced folder as one row of the compact list. Click to expand it into its full card.
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import Play from '@lucide/svelte/icons/play';
  import Eye from '@lucide/svelte/icons/eye';
  import EyeOff from '@lucide/svelte/icons/eye-off';
  import FolderCard from './FolderCard.svelte';
  import ProgressBar from './ProgressBar.svelte';
  import { app } from '../lib/state.svelte.js';
  import { folderState, isHidden, tagsOf } from '../lib/folders.svelte.js';
  import { bytes, normalizeDestination, relativeTime } from '../lib/format.js';

  let { preset, expanded = false, focused = false, selecting = false, selected = false, onselect = () => {}, ontoggle = () => {}, onfocus = () => {} } = $props();
  let st = $derived(folderState(preset));
  let tags = $derived(tagsOf(preset));
</script>

<div class="wrap" class:expanded class:focused data-folder={preset.preset_id}>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="row" role="row" tabindex="-1" onclick={() => { onfocus(); ontoggle(); }}>
    {#if selecting}
      <label class="pick"><input type="checkbox" checked={selected} onclick={(e) => e.stopPropagation()} onchange={onselect} aria-label="Select {preset.name}" /></label>
    {/if}
    <span class="chev" class:open={expanded}><ChevronRight size={15} /></span>
    <span class="dot {st.kind}" title={st.label}></span>
    <span class="name">
      <strong class="truncate">{preset.name}</strong>
      <span class="mono muted truncate start" title={preset.source_path}><bdi>{preset.source_path}</bdi></span>
    </span>
    <span class="dest mono muted truncate start" title="gs://{normalizeDestination(preset.destination)}/"><bdi>gs://{normalizeDestination(preset.destination)}/</bdi></span>
    <span class="state {st.kind}">
      {#if st.kind === 'syncing' && st.progress != null}
        <span>{st.label} {Math.round(st.progress)}%</span>
        <span class="bar"><ProgressBar value={st.progress} /></span>
      {:else}
        <span>{st.label}{st.kind === 'new' && st.bytes ? ` · ${bytes(st.bytes)}` : ''}</span>
      {/if}
    </span>
    <span class="tags">
      {#if isHidden(preset)}<span class="pill"><EyeOff size={11} /> Hidden</span>{/if}
      {#each tags.slice(0, 2) as t (t)}<span class="pill tag">{t}</span>{/each}
      {#if tags.length > 2}<span class="pill tag">+{tags.length - 2}</span>{/if}
    </span>
    <span class="when muted">{st.lastSynced ? relativeTime(new Date(st.lastSynced).toISOString()) : '—'}</span>
    <span class="quick">
      {#if st.kind !== 'syncing'}
        <button class="btn ghost sm icon" title="Preview (P)" aria-label="Preview {preset.name}" onclick={(e) => { e.stopPropagation(); app.runPreset(preset, true); }}><Eye size={15} /></button>
        <button class="btn ghost sm icon" title="Sync now (S)" aria-label="Sync {preset.name}" onclick={(e) => { e.stopPropagation(); app.runPreset(preset, false); }}><Play size={15} /></button>
      {/if}
    </span>
  </div>
  {#if expanded}
    <div class="card-wrap"><FolderCard {preset} /></div>
  {/if}
</div>

<style>
  .wrap {
    border-top: 1px solid var(--border);
  }
  .wrap:first-child {
    border-top: none;
  }
  .row {
    display: grid;
    grid-template-columns: auto auto 10px minmax(160px, 1.4fr) minmax(120px, 1fr) minmax(150px, 1fr) minmax(0, auto) 92px 70px;
    align-items: center;
    gap: 12px;
    padding: 9px 14px;
    cursor: pointer;
    outline: none;
  }
  .row:not(:has(.pick)) {
    grid-template-columns: auto 10px minmax(160px, 1.4fr) minmax(120px, 1fr) minmax(150px, 1fr) minmax(0, auto) 92px 70px;
  }
  .row:hover {
    background: var(--surface-2);
  }
  .focused > .row {
    box-shadow: inset 3px 0 0 var(--accent);
    background: var(--surface-2);
  }
  .chev {
    color: var(--muted);
    display: grid;
    transition: transform 0.15s;
  }
  .chev.open {
    transform: rotate(90deg);
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--border-strong);
  }
  .dot.ok {
    background: var(--success);
  }
  .dot.new {
    background: var(--accent);
  }
  .dot.syncing {
    background: var(--accent);
    animation: pulse 1.2s ease-in-out infinite;
  }
  .dot.attention {
    background: var(--warning);
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .dot.syncing {
      animation: none;
    }
  }
  .name {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .name strong {
    font-size: 13.5px;
  }
  .name .mono,
  .dest {
    font-size: 11.5px;
    min-width: 0;
  }
  .state {
    font-size: 12.5px;
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .state.attention {
    color: var(--warning);
    font-weight: 600;
  }
  .state.new {
    color: var(--accent);
    font-weight: 600;
  }
  .state.ok {
    color: var(--success);
  }
  .state .bar {
    flex: 1;
    min-width: 40px;
  }
  .tags {
    display: flex;
    gap: 4px;
    flex-wrap: nowrap;
    overflow: hidden;
  }
  .pill.tag {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .when {
    font-size: 12px;
    text-align: right;
  }
  .quick {
    display: flex;
    justify-content: flex-end;
    gap: 2px;
  }
  .card-wrap {
    padding: 4px 14px 14px 40px;
  }
  .muted {
    color: var(--muted);
  }
</style>
