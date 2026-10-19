<script>
  import { untrack } from 'svelte';
  import X from '@lucide/svelte/icons/x';
  import { app } from '../lib/state.svelte.js';
  import { api } from '../lib/api.js';

  /** item: { bucket, path, isFolder, name }; mode: 'copy' | 'move' */
  let { item, mode, onclose } = $props();

  const source = untrack(() => `gs://${item.bucket}/${item.path}`);
  let destination = $state(untrack(() => `gs://${item.bucket}/`));
  let moveKind = $state('folder');
  let preview = $state(true);
  let noClobber = $state(false);
  let busy = $state(false);
  let error = $state('');

  let dest = $derived(destination.trim());
  let validDest = $derived(/^gs:\/\/[a-z0-9][a-z0-9._-]+(\/.*)?$/.test(dest));
  let title = $derived(mode === 'copy' ? 'Sync folder to another location' : item.isFolder ? 'Move folder' : 'Move file');

  /** Destination as a folder (trailing slash), for syncs and folder moves. */
  function normalizedDest() {
    return dest.endsWith('/') ? dest : `${dest}/`;
  }

  let resultPath = $derived.by(() => {
    if (!validDest) return '';
    const base = dest.endsWith('/') ? dest : `${dest}/`;
    if (mode === 'copy') return base;
    if (!item.isFolder) return dest.endsWith('/') ? `${base}${item.name}` : dest;
    return moveKind === 'folder' ? `${base}${item.name}` : base;
  });

  async function start() {
    busy = true;
    error = '';
    try {
      if (mode === 'copy') {
        await api.cloudTransfer({ source_path: source, dest_path: normalizedDest(), recursive: true, dry_run: preview, no_clobber: noClobber });
      } else {
        const destPath = item.isFolder ? normalizedDest() : resultPath;
        await api.cloudMove({
          source_path: source,
          dest_path: destPath,
          source_kind: item.isFolder ? moveKind : 'object',
          dry_run: preview,
        });
      }
      app.toast(`${preview ? 'Preview' : mode === 'copy' ? 'Sync' : 'Move'} started: ${item.name}`, 'info', {
        action: { label: 'Activity', run: () => (app.view = 'activity') },
      });
      onclose();
    } catch (err) {
      error = err.message;
    } finally {
      busy = false;
    }
  }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && onclose()} />

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="dialog card" role="dialog" aria-modal="true" aria-labelledby="cloud-op-title">
    <header>
      <h2 id="cloud-op-title">{title}</h2>
      <button class="btn ghost icon" aria-label="Close" onclick={onclose}><X size={18} /></button>
    </header>
    <div class="body">
      <div class="field">
        <span class="label">From</span>
        <div class="mono source">{source}</div>
      </div>
      <div class="field">
        <label for="cloud-dest">To</label>
        <input id="cloud-dest" class="input mono" bind:value={destination} placeholder="gs://bucket/folder/" />
        {#if dest && !validDest}
          <span class="hint bad">Start with gs:// followed by a bucket name.</span>
        {/if}
      </div>

      {#if mode === 'move' && item.isFolder}
        <div class="choices">
          <label class="choice" class:selected={moveKind === 'folder'}>
            <input type="radio" bind:group={moveKind} value="folder" />
            <span><strong>Move the folder</strong><small>Creates “{item.name}” inside the destination.</small></span>
          </label>
          <label class="choice" class:selected={moveKind === 'folder_contents'}>
            <input type="radio" bind:group={moveKind} value="folder_contents" />
            <span><strong>Move only what's inside</strong><small>The folder's files go directly into the destination.</small></span>
          </label>
        </div>
      {/if}

      {#if resultPath}
        <div class="result">
          {mode === 'copy' ? 'Files will be copied into' : 'Result'} <span class="mono">{resultPath}</span>
          {#if mode === 'move'}<br /><small>Each object is copied and verified before the original is deleted. Existing objects at the destination are never overwritten.</small>{/if}
          {#if mode === 'copy'}<br /><small>Uses gcloud storage rsync: only new or changed files are copied; nothing is deleted.</small>{/if}
        </div>
      {/if}

      <label class="toggle">
        <input type="checkbox" bind:checked={preview} />
        <span><strong>Preview first</strong><small>Check what would happen without changing anything.</small></span>
      </label>
      {#if mode === 'copy'}
        <label class="toggle">
          <input type="checkbox" bind:checked={noClobber} />
          <span><strong>Never overwrite files at the destination</strong></span>
        </label>
      {/if}

      {#if error}<div class="error">{error}</div>{/if}
    </div>
    <footer>
      <button class="btn ghost" onclick={onclose}>Cancel</button>
      <button class="btn primary" disabled={!validDest || busy} onclick={start}>
        {preview ? 'Preview' : mode === 'copy' ? 'Start sync' : 'Move'}
      </button>
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
    width: min(580px, 100%);
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
  h2 {
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
  .source {
    padding: 8px 11px;
    border-radius: var(--radius-sm);
    background: var(--surface-2);
    word-break: break-all;
  }
  .choices {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  .choice {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 11px 12px;
    border: 1.5px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .choice.selected {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .choice input,
  .toggle input {
    margin-top: 3px;
    accent-color: var(--accent);
  }
  .choice span,
  .toggle span {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .choice small,
  .toggle small,
  .result small {
    color: var(--muted);
    font-size: 12.5px;
  }
  .toggle {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    cursor: pointer;
  }
  .toggle strong {
    font-weight: 550;
  }
  .result {
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    background: var(--accent-soft);
    font-size: 13px;
    word-break: break-all;
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
