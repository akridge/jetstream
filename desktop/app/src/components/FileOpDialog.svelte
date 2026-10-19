<script>
  import { untrack } from 'svelte';
  import X from '@lucide/svelte/icons/x';
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
  import Info from '@lucide/svelte/icons/info';
  import { cloudOps } from '../lib/cloudops.svelte.js';
  import { newOpId, planCloudOp, runCloudOp } from '../lib/platform.js';
  import { bytes, count } from '../lib/format.js';

  /**
   * kind: 'transfer' (move or copy to a destination the user picks), 'delete', or 'rename'.
   * paths: selected object names / folder prefixes (ending in "/") in `bucket`.
   * here: the folder being browsed (default destination for transfers).
   * planned: { opId, summary } when the caller already planned it (rename).
   */
  let { kind, bucket, paths, here = '', newName = '', planned = null, onclose } = $props();

  let destination = $state(untrack(() => `gs://${bucket}/${here}`));
  let move = $state(true);
  let opId = $state(untrack(() => planned?.opId || null));
  let summary = $state(untrack(() => planned?.summary || null));
  let planning = $state(false);
  let error = $state('');
  let understood = $state(false);

  const CLASS_NAMES = { NEARLINE: 'Nearline', COLDLINE: 'Coldline', ARCHIVE: 'Archive' };
  let what = $derived(
    paths.length === 1
      ? `“${paths[0].replace(/\/$/, '').split('/').pop()}${paths[0].endsWith('/') ? '/' : ''}”`
      : `${count(paths.length)} items`,
  );
  let title = $derived(kind === 'delete' ? `Delete ${what}` : kind === 'rename' ? `Rename ${what}` : `Move or copy ${what}`);
  let removes = $derived(kind === 'delete' || kind === 'rename' || (kind === 'transfer' && move));
  let permanent = $derived(kind === 'delete' && summary && !summary.soft_delete_seconds);
  let keepDays = $derived(summary ? Math.round(summary.soft_delete_seconds / 86400) : 0);
  let canRun = $derived(!!summary && !planning && (summary.objects || summary.folder_renames) && (!permanent || understood));

  function parseDestination(value) {
    const clean = value.trim().replace(/^gs:\/\//, '').replace(/^\/+/, '');
    const [b, ...rest] = clean.split('/');
    const p = rest.join('/');
    return { bucket: b, prefix: p && !p.endsWith('/') ? `${p}/` : p };
  }

  async function plan() {
    planning = true;
    error = '';
    summary = null;
    opId = newOpId();
    try {
      let request;
      if (kind === 'delete') request = { kind: 'delete', bucket, paths };
      else {
        const dst = parseDestination(destination);
        if (!dst.bucket) throw new Error('Enter a destination like gs://bucket/folder/');
        request = { kind: move ? 'move' : 'copy', bucket, paths, dst_bucket: dst.bucket, dst_prefix: dst.prefix };
      }
      summary = await planCloudOp(opId, request);
    } catch (err) {
      error = String(err?.message || err);
    } finally {
      planning = false;
    }
  }

  // Deletes are planned straight away; transfers once a destination is chosen.
  $effect(() => {
    if (kind === 'delete' && !untrack(() => summary)) untrack(plan);
  });

  function run() {
    const total = (summary.objects || 0) + (summary.folder_renames || 0);
    const verb = kind === 'delete' ? 'Deleted' : kind === 'rename' ? 'Renamed' : move ? 'Moved' : 'Copied';
    const label = kind === 'rename' ? `Rename to ${newName}` : kind === 'delete' ? `Delete ${what}` : `${move ? 'Move' : 'Copy'} ${what}`;
    const id = opId;
    cloudOps.start(id, label, total, verb, (onProgress) => runCloudOp(id, onProgress));
    onclose();
  }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && onclose()} />

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="dialog card" role="dialog" aria-modal="true" aria-labelledby="fo-title">
    <header>
      <h2 id="fo-title" class="truncate">{title}</h2>
      <button class="btn ghost icon" aria-label="Close" onclick={onclose}><X size={18} /></button>
    </header>
    <div class="body">
      {#if kind === 'transfer'}
        <div class="field">
          <label for="fo-dest">Destination folder</label>
          <form class="dest" onsubmit={(e) => { e.preventDefault(); plan(); }}>
            <!-- svelte-ignore a11y_autofocus -->
            <input id="fo-dest" class="input mono" bind:value={destination} oninput={() => (summary = null)} autofocus />
            <button class="btn" type="submit" disabled={planning}>{planning ? 'Checking…' : 'Check'}</button>
          </form>
        </div>
        <div class="segmented" role="radiogroup" aria-label="Move or copy">
          <button role="radio" aria-checked={move} class:selected={move} onclick={() => { move = true; summary = null; }}>Move</button>
          <button role="radio" aria-checked={!move} class:selected={!move} onclick={() => { move = false; summary = null; }}>Copy</button>
        </div>
      {:else if kind === 'rename'}
        <p class="lead">New name: <strong class="mono">{newName}</strong></p>
      {/if}

      {#if planning && kind !== 'transfer'}
        <p class="muted">Checking what this affects…</p>
      {/if}
      {#if error}<div class="callout danger">{error}</div>{/if}

      {#if summary}
        <div class="facts">
          {#if summary.folder_renames}
            <p><strong>Renamed in one step by Google</strong> (this bucket has real folders), however many files it holds.</p>
          {:else}
            <p>
              <strong>{count(summary.objects)} {summary.objects === 1 ? 'file' : 'files'}</strong> · {bytes(summary.bytes)}
              {#if summary.example && kind !== 'delete'}<br /><span class="muted">e.g. <span class="mono">{summary.example}</span></span>{/if}
            </p>
            {#if kind !== 'delete'}
              <p class="muted">Copied inside Google Cloud (nothing downloads), each copy checked{removes ? ', then the original removed' : ''}.</p>
            {/if}
          {/if}
        </div>

        {#if summary.conflicts}
          <div class="callout warn">
            <TriangleAlert size={16} />
            <span>
              {count(summary.conflicts)} already {summary.conflicts === 1 ? 'exists' : 'exist'} at the destination with different content and will be left as they are (nothing is overwritten).
              {#each summary.conflict_examples as ex (ex)}<br /><span class="mono small">{ex}</span>{/each}
            </span>
          </div>
        {/if}
        {#if summary.already_copied}
          <div class="callout info">
            <Info size={16} />
            <span>{count(summary.already_copied)} were already copied by an earlier move; only their originals will be removed.</span>
          </div>
        {/if}
        {#if summary.held}
          <div class="callout warn">
            <TriangleAlert size={16} />
            <span>{count(summary.held)} {summary.held === 1 ? 'object is' : 'objects are'} under a hold and can't be removed; {summary.held === 1 ? 'it' : 'they'} will be listed as failed.</span>
          </div>
        {/if}
        {#each summary.early_deletion as fee (fee.storage_class)}
          <div class="callout warn">
            <TriangleAlert size={16} />
            <span>
              {count(fee.objects)} {CLASS_NAMES[fee.storage_class] || fee.storage_class} {fee.objects === 1 ? 'file' : 'files'} ({bytes(fee.bytes)})
              {fee.objects === 1 ? 'is' : 'are'} younger than {fee.min_days} days. Google charges an <strong>early deletion fee</strong> for the remaining days{kind === 'delete'
                ? '.'
                : ', plus retrieval fees to read them. The copies start a new minimum storage period.'}
            </span>
          </div>
        {/each}
        {#if removes && summary.soft_delete_seconds}
          <div class="callout info">
            <Info size={16} />
            <span>
              {kind === 'delete' ? 'Deleted files' : 'The originals'} can be restored for {keepDays} {keepDays === 1 ? 'day' : 'days'} from <strong>Recently deleted</strong> (this bucket keeps deleted files, and bills them during that time).
            </span>
          </div>
        {/if}
        {#if summary.objects > 1000 && kind !== 'delete'}
          <p class="muted small">Undo isn't offered for operations over 1,000 files.</p>
        {/if}
        {#if permanent}
          <div class="callout danger">
            <TriangleAlert size={16} />
            <span>This bucket doesn't keep deleted files. <strong>Deleting can't be undone.</strong></span>
          </div>
          <label class="toggle">
            <input type="checkbox" bind:checked={understood} />
            <span>I understand {summary.objects === 1 ? 'this file' : `these ${count(summary.objects)} files`} will be permanently deleted.</span>
          </label>
        {/if}
      {/if}
    </div>
    <footer>
      <button class="btn ghost" onclick={onclose}>Cancel</button>
      {#if kind === 'transfer' && !summary}
        <button class="btn primary" disabled={planning || !destination.trim()} onclick={plan}>{planning ? 'Checking…' : 'Check'}</button>
      {:else}
        <button class="btn primary" class:danger-btn={kind === 'delete'} disabled={!canRun} onclick={run}>
          {kind === 'delete' ? (permanent ? 'Delete permanently' : 'Delete') : kind === 'rename' ? 'Rename' : move ? 'Move' : 'Copy'}
        </button>
      {/if}
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
  .dest {
    display: flex;
    gap: 8px;
  }
  .dest input {
    flex: 1;
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
    padding: 6px 14px;
    border-radius: 6px;
    cursor: pointer;
    color: var(--text-2);
    font: inherit;
  }
  .segmented button.selected {
    background: var(--surface);
    color: var(--text);
    font-weight: 600;
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.12);
  }
  .lead,
  .facts p {
    margin: 0;
    word-break: break-word;
  }
  .facts {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .muted {
    color: var(--muted);
  }
  .small {
    font-size: 12px;
  }
  .callout {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    font-size: 13px;
    word-break: break-word;
  }
  .callout :global(svg) {
    flex-shrink: 0;
    margin-top: 1px;
  }
  .callout.warn {
    background: var(--warning-soft, var(--surface-2));
    color: var(--warning);
  }
  .callout.info {
    background: var(--accent-soft);
    color: var(--text);
  }
  .callout.danger {
    background: var(--danger-soft);
    color: var(--danger);
  }
  .toggle {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    cursor: pointer;
    font-weight: 550;
  }
  .toggle input {
    margin-top: 3px;
    accent-color: var(--danger);
  }
  .danger-btn {
    background: var(--danger);
    border-color: var(--danger);
    color: #fff;
  }
</style>
