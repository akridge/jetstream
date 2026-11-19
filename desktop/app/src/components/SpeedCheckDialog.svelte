<script>
  // Speed check: what limits uploads from this computer to a bucket (the folder, the network,
  // or per-request time). Nothing is stored in the bucket; it takes about half a minute.
  import { onMount } from 'svelte';
  import X from '@lucide/svelte/icons/x';
  import Gauge from '@lucide/svelte/icons/gauge';
  import Copy from '@lucide/svelte/icons/copy';
  import Check from '@lucide/svelte/icons/check';
  import { app } from '../lib/state.svelte.js';
  import { bytes, count, normalizeDestination } from '../lib/format.js';
  import { cancelCloudOp, newOpId, speedCheck } from '../lib/platform.js';

  let { dialog } = $props();
  const STEPS = ['Listing the folder', 'Reading files', 'Timing requests', 'Uploading over one connection', 'Uploading over 8 connections'];
  let opId = $state('');
  let destination = $state(dialog.destination || (app.prefs.recent_buckets?.[0] ? `gs://${app.prefs.recent_buckets[0]}/` : ''));
  let localPath = $derived(dialog.localPath || null);
  let running = $state(false);
  let tick = $state(null);
  let report = $state(null);
  let error = $state('');
  let copied = $state(false);
  let step = $derived(tick ? Math.min(tick.done, STEPS.length - 1) : localPath ? 0 : 2);
  const LIMITS = {
    folder: 'The folder is the limit',
    network: 'The network is the limit',
    requests: 'Per-request time is the limit',
    none: 'Nothing stands out',
  };

  /** 4740.7 -> "4,741", 28 -> "28.0", null -> "–". */
  const rate = (v) => (v == null ? '–' : v >= 100 ? count(Math.round(v)) : v.toFixed(1));

  onMount(() => {
    if (dialog.destination) start();
  });

  async function start() {
    const target = normalizeDestination(destination);
    if (!target) {
      error = 'Enter a bucket, such as gs://my-bucket/';
      return;
    }
    error = '';
    report = null;
    tick = null;
    running = true;
    opId = newOpId();
    try {
      report = await speedCheck(opId, `gs://${target}`, localPath, (t) => (tick = t));
      if (report.cancelled) report = null;
    } catch (err) {
      error = String(err?.message || err);
    } finally {
      running = false;
    }
  }

  function close() {
    if (running) cancelCloudOp(opId);
    app.speedCheck = null;
  }

  function summaryText() {
    const r = report;
    const f = r.folder;
    const rows = [`JetStream speed check: ${r.destination}`, LIMITS[r.limit] || ''];
    if (f) {
      rows.push(`Folder: ${f.path}`, `  Listing: ${count(f.files)} files in ${f.list_seconds} s (${count(f.list_files_per_sec)} files/s)`);
      if (f.read_mb_per_sec != null) rows.push(`  Reading large files: ${f.read_mb_per_sec} MB/s`);
      if (f.small_files_per_sec != null) rows.push(`  Small files: ${count(f.small_files_per_sec)} files/s`);
    }
    rows.push(`One connection: ${r.single_mb_per_sec} MB/s`, `${r.streams} connections: ${r.parallel_mb_per_sec} MB/s`, `Request time: ${r.request_ms} ms`, '', ...r.findings.map((x) => `- ${x}`));
    return rows.join('\n');
  }

  async function copy() {
    try {
      await navigator.clipboard.writeText(summaryText());
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      app.toast("Couldn't copy to the clipboard", 'error');
    }
  }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && close()} />

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && close()}>
  <div class="dialog card" role="dialog" aria-modal="true" aria-labelledby="speed-title">
    <header>
      <h2 id="speed-title">Speed check</h2>
      <button class="btn ghost icon" aria-label="Close" onclick={close}><X size={18} /></button>
    </header>
    <div class="body">
      {#if !dialog.destination}
        <label class="field">
          <span>Bucket or folder to test against</span>
          <input class="input mono" placeholder="gs://my-bucket/" bind:value={destination} disabled={running} onkeydown={(e) => e.key === 'Enter' && !running && start()} />
        </label>
      {:else}
        <p class="mono where">{dialog.destination}{#if localPath}<br /><span class="muted">from {localPath}</span>{/if}</p>
      {/if}

      {#if running}
        <ol class="steps" aria-live="polite">
          {#each STEPS as label, i (label)}
            {#if localPath || i >= 2}
              <li class:done={i < step} class:now={i === step}>{label}{#if i === step && tick?.bytes} <span class="muted">· {bytes(tick.bytes)}</span>{/if}</li>
            {/if}
          {/each}
        </ol>
        <p class="muted small">Takes about half a minute. Test data goes to uploads that are never finished, so nothing is stored in the bucket.</p>
      {:else if error}
        <div class="error">{error}</div>
      {:else if report}
        <div class="verdict limit-{report.limit}"><Gauge size={18} /> <strong>{LIMITS[report.limit]}</strong></div>
        <div class="numbers">
          {#if report.folder}
            <div class="num"><span class="value">{count(report.folder.list_files_per_sec)}</span><span class="unit">files/s</span><span class="what">listing the folder ({count(report.folder.files)} files, {report.folder.list_seconds} s)</span></div>
            <div class="num"><span class="value">{rate(report.folder.read_mb_per_sec)}</span><span class="unit">MB/s</span><span class="what">reading large files{report.folder.read_mb_per_sec == null ? ' (none over 1 MB)' : report.folder.read_mb_per_sec > 1500 ? ' (probably from memory: read recently)' : ''}</span></div>
            {#if report.folder.small_files_per_sec != null}
              <div class="num"><span class="value">{count(report.folder.small_files_per_sec)}</span><span class="unit">files/s</span><span class="what">reading small files</span></div>
            {/if}
          {/if}
          <div class="num"><span class="value">{rate(report.single_mb_per_sec)}</span><span class="unit">MB/s</span><span class="what">uploading over one connection</span></div>
          <div class="num"><span class="value">{rate(report.parallel_mb_per_sec)}</span><span class="unit">MB/s</span><span class="what">uploading over {report.streams} connections</span></div>
          <div class="num"><span class="value">{report.request_ms >= 10 ? Math.round(report.request_ms) : report.request_ms}</span><span class="unit">ms</span><span class="what">per request to Cloud Storage</span></div>
        </div>
        <ul class="findings">
          {#each report.findings as finding (finding)}<li>{finding}</li>{/each}
        </ul>
        {#if app.transfer?.max_mbps}
          <p class="muted small">Your speed limit ({app.transfer.max_mbps} Mbit/s) wasn't applied during the check; uploads still use it.</p>
        {/if}
      {:else}
        <p class="muted small">
          Measures how fast uploads can go to this bucket over one connection and over several, and how long each request takes. To include
          reading a folder, use <strong>Speed check…</strong> on a synced folder's card. Nothing is stored in the bucket.
        </p>
      {/if}
    </div>
    <footer>
      {#if report}
        <button class="btn ghost" onclick={copy}>{#if copied}<Check size={15} /> Copied{:else}<Copy size={15} /> Copy results{/if}</button>
      {/if}
      <span class="grow"></span>
      <button class="btn ghost" onclick={close}>{running ? 'Stop' : 'Close'}</button>
      {#if !running}
        <button class="btn primary" onclick={start}><Gauge size={15} /> {report || error ? 'Run again' : 'Start'}</button>
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
  header h2 {
    font-size: 17px;
  }
  footer {
    border-top: 1px solid var(--border);
  }
  .grow {
    flex: 1;
  }
  .body {
    padding: 18px 20px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .field {
    display: grid;
    gap: 6px;
    font-size: 13px;
    color: var(--text-2);
  }
  .where {
    font-size: 12.5px;
    margin: 0;
    word-break: break-all;
  }
  .steps {
    margin: 0;
    padding-left: 22px;
    display: grid;
    gap: 6px;
    font-size: 13.5px;
    color: var(--muted);
  }
  .steps li.done {
    color: var(--text-2);
  }
  .steps li.done::marker {
    content: '✓  ';
    color: var(--success, var(--accent));
  }
  .steps li.now {
    color: var(--text);
    font-weight: 600;
  }
  .verdict {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    background: var(--surface-2);
    font-size: 14px;
  }
  .numbers {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(165px, 1fr));
    gap: 10px;
  }
  .num {
    display: grid;
    grid-template-columns: auto 1fr;
    align-items: baseline;
    column-gap: 4px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .value {
    font-size: 20px;
    font-weight: 650;
    font-variant-numeric: tabular-nums;
  }
  .unit {
    font-size: 12px;
    color: var(--muted);
  }
  .what {
    grid-column: 1 / -1;
    font-size: 12px;
    color: var(--text-2);
  }
  .findings {
    margin: 0;
    padding-left: 18px;
    display: grid;
    gap: 6px;
    font-size: 13.5px;
  }
  .small {
    font-size: 12.5px;
    margin: 0;
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
