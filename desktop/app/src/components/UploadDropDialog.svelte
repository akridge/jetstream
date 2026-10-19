<script>
  import X from '@lucide/svelte/icons/x';
  import Folder from '@lucide/svelte/icons/folder';
  import File from '@lucide/svelte/icons/file';
  import Info from '@lucide/svelte/icons/info';
  import { app } from '../lib/state.svelte.js';
  import { api } from '../lib/api.js';
  import { cloudOps } from '../lib/cloudops.svelte.js';
  import { basename, count } from '../lib/format.js';
  import { classifyPaths, newOpId, uploadLocalFiles } from '../lib/platform.js';

  /** Paths dropped onto the open cloud folder gs://bucket/prefix. */
  let { bucket, prefix, paths, onclose } = $props();

  let folders = $state([]);
  let files = $state([]);
  let preview = $state(false);
  let busy = $state(false);
  let error = $state('');

  let tool = $derived(
    [app.prefs.default_upload_tool, 'jetstream', 'gcloud'].find((t) => t && app.toolAvailable(t)) || 'gcloud',
  );
  let destination = $derived(`gs://${bucket}/${prefix}`);

  $effect(() => {
    classifyPaths(paths).then((list) => {
      folders = list.filter(([, isDir]) => isDir).map(([p]) => p);
      files = list.filter(([, isDir]) => !isDir).map(([p]) => p);
    });
  });

  async function upload() {
    busy = true;
    error = '';
    try {
      // Folders: regular upload jobs (Activity, progress, resume), never overwriting.
      for (const folder of folders) {
        await api.createUpload({
          source_path: folder,
          gcs_destination: `${bucket}/${prefix}${basename(folder)}`,
          dry_run: preview,
          recursive: true,
          split_by_folder: false,
          upload_tool: tool,
          no_clobber: true,
          threads: 4,
        });
      }
      if (folders.length) {
        app.toast(`${preview ? 'Previewing' : 'Uploading'} ${count(folders.length)} ${folders.length === 1 ? 'folder' : 'folders'} — see Activity`, 'info', {
          action: { label: 'Activity', run: () => (app.view = 'activity') },
        });
      }
      // Loose files: uploaded straight away by the app.
      if (files.length && !preview) {
        const id = newOpId();
        cloudOps.start(id, `Upload ${count(files.length)} ${files.length === 1 ? 'file' : 'files'}`, files.length, 'Uploaded', (onProgress) =>
          uploadLocalFiles(id, bucket, prefix, files, onProgress),
        );
      }
      onclose();
    } catch (err) {
      error = String(err?.message || err);
    } finally {
      busy = false;
    }
  }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && !busy && onclose()} />

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && !busy && onclose()}>
  <div class="dialog card" role="dialog" aria-modal="true" aria-labelledby="ud-title">
    <header>
      <h2 id="ud-title">Upload to the cloud</h2>
      <button class="btn ghost icon" aria-label="Close" disabled={busy} onclick={onclose}><X size={18} /></button>
    </header>
    <div class="body">
      <p class="lead">Into <span class="mono">{destination}</span></p>
      <ul class="items">
        {#each folders as f (f)}
          <li><Folder size={15} color="var(--accent)" /><span class="mono truncate start" title={f}><bdi>{f}</bdi></span><span class="muted">→ {basename(f)}/</span></li>
        {/each}
        {#each files as f (f)}
          <li><File size={15} color="var(--muted)" /><span class="mono truncate start" title={f}><bdi>{f}</bdi></span></li>
        {/each}
      </ul>
      <div class="note">
        <Info size={15} />
        <span>
          Nothing in the cloud is overwritten: files that already exist there are left as they are.
          {#if folders.length}Folders upload as jobs you can follow in <strong>Activity</strong> (using {tool === 'jetstream' ? 'the JetStream engine' : 'gcloud'}).{/if}
        </span>
      </div>
      {#if folders.length}
        <label class="toggle">
          <input type="checkbox" bind:checked={preview} />
          <span><strong>Preview folders first</strong><small>See what would upload without uploading.{files.length ? ' Loose files wait until you upload for real.' : ''}</small></span>
        </label>
      {/if}
      {#if error}<div class="error">{error}</div>{/if}
    </div>
    <footer>
      <button class="btn ghost" disabled={busy} onclick={onclose}>Cancel</button>
      <button class="btn primary" disabled={busy || (!folders.length && !files.length)} onclick={upload}>
        {busy ? 'Starting…' : preview ? 'Preview' : `Upload ${count(folders.length + files.length)} ${folders.length + files.length === 1 ? 'item' : 'items'}`}
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
    width: min(560px, 100%);
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
    gap: 14px;
  }
  .lead {
    margin: 0;
    word-break: break-all;
  }
  .items {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-height: 220px;
    overflow-y: auto;
  }
  .items li {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    min-width: 0;
  }
  .items .mono {
    flex: 1;
    min-width: 0;
  }
  .muted {
    color: var(--muted);
    white-space: nowrap;
  }
  .note {
    display: flex;
    gap: 10px;
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    background: var(--surface-2);
    font-size: 13px;
    color: var(--text-2);
  }
  .note :global(svg) {
    flex-shrink: 0;
    margin-top: 2px;
  }
  .toggle {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    cursor: pointer;
  }
  .toggle span {
    display: flex;
    flex-direction: column;
  }
  .toggle small {
    color: var(--muted);
  }
  .toggle input {
    margin-top: 3px;
    accent-color: var(--accent);
  }
  .error {
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    background: var(--danger-soft);
    color: var(--danger);
  }
</style>
