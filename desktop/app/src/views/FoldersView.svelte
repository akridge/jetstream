<script>
  import FolderPlus from '@lucide/svelte/icons/folder-plus';
  import FolderSync from '@lucide/svelte/icons/folder-sync';
  import FolderCard from '../components/FolderCard.svelte';
  import { app, ACTIVE } from '../lib/state.svelte.js';
  import { isDesktop, pickFolder } from '../lib/platform.js';

  let running = $derived(app.jobList.filter((j) => ACTIVE.has(j.status)).length);

  async function add() {
    const path = await pickFolder();
    if (path) app.addFolder(path);
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
    <button class="btn primary" onclick={add}><FolderPlus size={17} /> Add folder</button>
  </header>

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
  {:else}
    <div class="grid">
      {#each app.presets as preset (preset.preset_id)}
        <FolderCard {preset} />
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
