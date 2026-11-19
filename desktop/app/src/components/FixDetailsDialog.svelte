<script>
  // "Fix file details": add a proper file type, and (from the synced folder on this computer)
  // the original files' dates, to files already in the cloud. Checks first, then updates each
  // file's details in place; nothing is re-uploaded.
  import { onMount } from 'svelte';
  import X from '@lucide/svelte/icons/x';
  import FileCheck from '@lucide/svelte/icons/file-check';
  import { app, commaList, lines } from '../lib/state.svelte.js';
  import { cloudOps } from '../lib/cloudops.svelte.js';
  import { count } from '../lib/format.js';
  import { newOpId, planFixDetails, runFixDetails } from '../lib/platform.js';

  let { dialog } = $props();
  const opId = newOpId();
  let plan = $state(null);
  let error = $state('');
  let preset = $derived(dialog.preset || null);
  let topTypes = $derived(plan ? Object.entries(plan.by_type).sort((a, b) => b[1] - a[1]).slice(0, 6) : []);

  onMount(async () => {
    try {
      const o = preset?.options || {};
      plan = await planFixDetails(opId, dialog.destination, preset
        ? { localPath: preset.source_path, excludePatterns: lines(o.exclude_patterns) || [], excludeFolders: commaList(o.exclude_folders) || [], recursive: o.recursive !== false }
        : {});
    } catch (err) {
      error = String(err?.message || err);
    }
  });

  function close() {
    app.fixDetails = null;
  }

  function fix() {
    // Read everything before closing: closing clears the dialog's props.
    const total = Math.max(plan.types, plan.dates);
    const label = `Fixing file details in ${dialog.destination}`;
    close();
    cloudOps.start(opId, label, total, 'Updated', (onProgress) => runFixDetails(opId, onProgress));
  }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && close()} />

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && close()}>
  <div class="dialog card" role="dialog" aria-modal="true" aria-labelledby="fix-title">
    <header>
      <h2 id="fix-title">Fix file details</h2>
      <button class="btn ghost icon" aria-label="Close" onclick={close}><X size={18} /></button>
    </header>
    <div class="body">
      <p class="mono where">{dialog.destination}{#if preset}<br /><span class="muted">dates from {preset.source_path}</span>{/if}</p>
      {#if error}
        <div class="error">{error}</div>
      {:else if !plan}
        <p class="muted">Checking the files{preset ? ' and comparing them with this computer' : ''}…</p>
      {:else if !plan.types && !plan.dates}
        <p><strong>Nothing to fix.</strong> All {count(plan.objects)} files already have their file type{preset ? ' and original dates' : ''}.</p>
      {:else}
        <ul class="summary">
          {#if plan.types}
            <li>
              <strong>{count(plan.types)} {plan.types === 1 ? 'file gets its' : 'files get their'} file type</strong>
              <span class="muted">({topTypes.map(([t, n]) => `${t} ${count(n)}`).join(', ')}{Object.keys(plan.by_type).length > topTypes.length ? ', …' : ''})</span>
            </li>
          {/if}
          {#if plan.dates}
            <li><strong>{count(plan.dates)} {plan.dates === 1 ? 'file gets its' : 'files get their'} original created and modified dates</strong> <span class="muted">from this computer</span></li>
          {/if}
        </ul>
        <p class="muted small">
          Of {count(plan.objects)} files in the cloud{#if preset}: {count(plan.not_local)} aren't on this computer and {count(plan.different)} differ from it (those don't get dates){#if plan.read_files}; {count(plan.read_files)} were read to check they match{/if}{/if}.
        </p>
        <p class="note">
          Nothing is re-uploaded: only each file's details change. Cloud Storage records that as an update, so the Cloud console's "Last modified"
          becomes today; the original dates appear in JetStream and under each file's custom metadata.
        </p>
      {/if}
      {#if !preset && plan && !error}
        <p class="muted small">To also record original dates, use <strong>Fix file details in the cloud…</strong> on the synced folder's card.</p>
      {/if}
    </div>
    <footer>
      <button class="btn ghost" onclick={close}>Cancel</button>
      <button class="btn primary" disabled={!plan || (!plan.types && !plan.dates)} onclick={fix}><FileCheck size={15} /> Fix {plan ? count(Math.max(plan.types, plan.dates)) : ''} files</button>
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
    gap: 12px;
  }
  .where {
    font-size: 12.5px;
    margin: 0;
    word-break: break-all;
  }
  .summary {
    margin: 0;
    padding-left: 18px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .small {
    font-size: 12.5px;
    margin: 0;
  }
  .note {
    font-size: 12.5px;
    margin: 0;
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    background: var(--surface-2);
    color: var(--text-2);
  }
  .muted {
    color: var(--muted);
  }
  .error {
    color: var(--danger);
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    background: var(--danger-soft);
  }
</style>
