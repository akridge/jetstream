<script>
  import { untrack } from 'svelte';
  import X from '@lucide/svelte/icons/x';
  import ShieldAlert from '@lucide/svelte/icons/shield-alert';
  import { app } from '../lib/state.svelte.js';
  import { api } from '../lib/api.js';
  import { bytes, count } from '../lib/format.js';

  /** report: Rust AuditReport; defaults: {quarantine_bucket, protected_prefixes}; runId: imported run (or null) */
  let { report, defaults, runId = $bindable(null), onclose } = $props();

  let bucket = $state(untrack(() => defaults?.quarantine_bucket || ''));
  let prefix = $state('quarantine');
  let preview = $state(true);
  let understood = $state(false);
  let busy = $state(false);
  let error = $state('');
  let result = $state(null);

  let validBucket = $derived(/^[a-z0-9][a-z0-9._-]{1,220}$/.test(bucket.trim()));
  let sameBucket = $derived(bucket.trim() === report.bucket);
  let canRun = $derived(validBucket && !busy && (preview || understood));

  async function ensureRun() {
    if (runId) return runId;
    const run = await api.importAudit({
      bucket_name: report.bucket,
      prefix: report.prefix,
      patterns: report.patterns,
      scanned_objects: report.scanned_objects,
      scanned_bytes: report.scanned_bytes,
      junk_objects: report.junk_objects,
      junk_bytes: report.junk_bytes,
      reached_scan_limit: report.reached_scan_limit,
      findings: report.findings.map((f) => ({
        object_name: f.object_name,
        size: f.size,
        updated: f.updated,
        matched_pattern: f.matched_pattern,
      })),
    });
    runId = run.run_id;
    return runId;
  }

  async function start() {
    busy = true;
    error = '';
    result = null;
    try {
      const id = await ensureRun();
      const body = {
        confirm_text: 'MOVE_TO_QUARANTINE',
        quarantine_bucket: bucket.trim(),
        quarantine_prefix: prefix.trim(),
        dry_run: preview,
        limit: 5000,
      };
      // The server handles up to 5000 per call; keep going until nothing is left.
      const total = { processed: 0, quarantined: 0, skipped: 0, errors: 0 };
      for (let round = 0; round < 50; round++) {
        const r = await api.quarantine(id, body);
        for (const key of Object.keys(total)) total[key] += r[key] || 0;
        if (preview || (r.processed || 0) < body.limit || !(r.quarantined + r.skipped + r.errors)) break;
      }
      result = { ...total, dry_run: preview };
      if (!preview) app.toast(`Moved ${count(total.quarantined)} objects to quarantine`, total.errors ? 'error' : 'success');
    } catch (err) {
      error = err.message;
    } finally {
      busy = false;
    }
  }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && !busy && onclose()} />

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && !busy && onclose()}>
  <div class="dialog card" role="dialog" aria-modal="true" aria-labelledby="q-title">
    <header>
      <h2 id="q-title">Move junk to quarantine</h2>
      <button class="btn ghost icon" aria-label="Close" disabled={busy} onclick={onclose}><X size={18} /></button>
    </header>
    <div class="body">
      <p class="lead">
        {count(report.findings.length)} objects ({bytes(report.findings.reduce((s, f) => s + f.size, 0))}) from
        <span class="mono">gs://{report.bucket}/{report.prefix}</span>
        {#if report.findings_truncated}<br /><small>Only the first {count(report.findings.length)} of {count(report.junk_objects)} findings are included; scan again afterwards for the rest.</small>{/if}
      </p>
      <div class="field">
        <label for="q-bucket">Quarantine bucket</label>
        <input id="q-bucket" class="input mono" bind:value={bucket} placeholder="my-quarantine-bucket" />
        {#if sameBucket}<span class="hint">Same bucket: objects move under the prefix below.</span>{/if}
      </div>
      <div class="field">
        <label for="q-prefix">Folder in the quarantine bucket</label>
        <input id="q-prefix" class="input mono" bind:value={prefix} />
        {#if bucket && prefix}
          <span class="hint">Example: <span class="mono">gs://{bucket.trim()}/{prefix.trim().replace(/\/+$/, '')}/{report.bucket}/{report.findings[0]?.object_name}</span></span>
        {/if}
      </div>
      <div class="note">
        <ShieldAlert size={16} />
        <span>
          Each object is copied, checked (size and checksum), then deleted from the original location.
          {#if defaults?.protected_prefixes?.length}Objects under {defaults.protected_prefixes.join(', ')} are never moved.{/if}
        </span>
      </div>
      <label class="toggle">
        <input type="checkbox" bind:checked={preview} />
        <span><strong>Preview only</strong><small>Check destinations without moving anything.</small></span>
      </label>
      {#if !preview}
        <label class="toggle warn">
          <input type="checkbox" bind:checked={understood} />
          <span><strong>I understand these objects will be removed from their current location.</strong></span>
        </label>
      {/if}
      {#if result}
        <div class="result">
          {#if result.dry_run}
            Preview: {count(result.processed)} objects would be moved{result.skipped ? `, ${count(result.skipped)} skipped (protected)` : ''}.
          {:else}
            Moved {count(result.quarantined)} · skipped {count(result.skipped)} · errors {count(result.errors)}
          {/if}
        </div>
      {/if}
      {#if error}<div class="error">{error}</div>{/if}
    </div>
    <footer>
      <button class="btn ghost" disabled={busy} onclick={onclose}>{result && !result.dry_run ? 'Done' : 'Cancel'}</button>
      <button class="btn primary" class:danger-btn={!preview} disabled={!canRun} onclick={start}>
        {busy ? 'Working…' : preview ? 'Preview' : 'Move to quarantine'}
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
    width: min(600px, 100%);
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
  .lead {
    margin: 0;
    word-break: break-all;
  }
  .lead small,
  .toggle small {
    color: var(--muted);
  }
  .hint .mono {
    word-break: break-all;
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
  .toggle strong {
    font-weight: 550;
  }
  .toggle input {
    margin-top: 3px;
    accent-color: var(--accent);
  }
  .toggle.warn strong {
    color: var(--warning);
  }
  .result {
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    background: var(--accent-soft);
  }
  .error {
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    background: var(--danger-soft);
    color: var(--danger);
  }
  .danger-btn {
    background: var(--danger);
    border-color: var(--danger);
    color: #fff;
  }
</style>
