<script>
  // A synced folder's verification receipt: did every file reach the cloud intact, what isn't
  // verified and how to fix it, earlier receipts, and the printable / CSV versions.
  import { untrack } from 'svelte';
  import ShieldCheck from '@lucide/svelte/icons/shield-check';
  import ShieldAlert from '@lucide/svelte/icons/shield-alert';
  import Printer from '@lucide/svelte/icons/printer';
  import Copy from '@lucide/svelte/icons/copy';
  import FileSpreadsheet from '@lucide/svelte/icons/file-spreadsheet';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import Search from '@lucide/svelte/icons/search';
  import Sheet from './Sheet.svelte';
  import ProgressBar from './ProgressBar.svelte';
  import { app, notVerified } from '../lib/state.svelte.js';
  import { bytes, count, dateTime, duration, normalizeDestination, relativeTime } from '../lib/format.js';
  import { listReceipts, openReceiptPage, readReceipt, revealPath } from '../lib/platform.js';

  let { sheet } = $props();
  const preset = untrack(() => sheet.preset);
  let current = $state(untrack(() => sheet.json));
  let followLatest = $state(true);
  let receipt = $state(null);
  let error = $state('');
  let history = $state([]);
  let query = $state('');

  const KINDS = {
    missing_in_cloud: {
      title: 'Not in the cloud',
      text: "On this computer but not uploaded yet, usually files added since the last sync.",
      fix: 'Upload them',
    },
    mismatch: {
      title: 'Different from the cloud',
      text: 'The file here and its cloud copy have different content: one of them changed after it was uploaded.',
      fix: 'Compare',
    },
    unreadable: {
      title: "Couldn't be read",
      text: 'Locked by another program or not permitted. Close programs using them, then verify again.',
    },
    online_only: {
      title: 'Online-only (OneDrive)',
      text: "Not on this computer, so they can't be checked. Right-click the folder → Always keep on this device, then verify again.",
    },
  };

  let verifying = $derived(app.verifying[preset.preset_id]);
  let latest = $derived(app.receipts[preset.preset_id]?.json || null);
  let t = $derived(receipt?.totals || {});
  let problems = $derived(notVerified(t));
  let ok = $derived(receipt?.result === 'complete');
  let shown = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return (receipt?.problems || []).map((group) => ({ ...group, files: q ? group.files.filter((f) => f.path.toLowerCase().includes(q)) : group.files }));
  });

  // A new receipt (Verify again) replaces the one shown, unless an older one was picked.
  $effect(() => {
    if (latest && followLatest && latest !== untrack(() => current)) current = latest;
  });
  $effect(() => {
    const path = current;
    receipt = null;
    error = '';
    if (!path) return;
    readReceipt(path)
      .then((r) => path === current && (receipt = r))
      .catch((err) => (error = String(err?.message || err)));
  });
  $effect(() => {
    void app.receiptsVersion;
    listReceipts(preset.name).then((list) => (history = (list || []).slice(0, 20))).catch(() => {});
  });

  /** "Today 1:12 AM", "Yesterday 4:05 PM", "Mar 3, 2026" — receipts made minutes apart stay distinct. */
  function when(iso) {
    const d = new Date(iso);
    const time = d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
    const days = Math.round((new Date().setHours(0, 0, 0, 0) - new Date(iso).setHours(0, 0, 0, 0)) / 86400000);
    if (days === 0) return `Today ${time}`;
    if (days === 1) return `Yesterday ${time}`;
    return d.toLocaleDateString([], { year: 'numeric', month: 'short', day: 'numeric' }) + (days < 7 ? ` ${time}` : '');
  }

  function pick(json) {
    followLatest = json === history[0]?.json;
    current = json;
  }

  function close() {
    app.receiptSheet = null;
  }

  function fix(kind) {
    close();
    if (kind === 'missing_in_cloud') app.runPreset(preset, false);
    else if (kind === 'mismatch') app.compareFolder(preset);
  }

  function summaryText() {
    const lines = [
      `JetStream receipt: ${ok ? `all ${count(t.verified)} files verified` : `${count(problems)} of ${count(t.local_files)} files not verified`}`,
      `Folder: ${receipt.local_root}`,
      `Cloud: ${receipt.destination}`,
      `Checked by ${receipt.account} on ${dateTime(receipt.finished_at)}`,
      `Verified: ${count(t.verified)} files (${bytes(t.verified_bytes)})`,
    ];
    for (const group of receipt.problems || []) lines.push(`${KINDS[group.kind]?.title || group.kind}: ${count(group.count)}`);
    return lines.join('\n');
  }

  async function copySummary() {
    await navigator.clipboard?.writeText(summaryText());
    app.toast('Receipt summary copied', 'info', { timeout: 2000 });
  }

  async function printable() {
    try {
      await openReceiptPage(current);
    } catch (err) {
      app.toast(String(err?.message || err), 'error');
    }
  }
</script>

<Sheet title="Verification receipt" subtitle={preset.name} width={history.length > 1 ? 920 : 720} onclose={close}>
  {#if verifying}
    <div class="verifying" aria-live="polite">
      <RefreshCw size={14} class="spin" />
      <span>Verifying… {verifying.total ? `${count(verifying.done)} of ${count(verifying.total)} files` : 'looking through the folder'}</span>
      <div class="bar"><ProgressBar value={verifying.total ? (100 * verifying.done) / verifying.total : 0} indeterminate={!verifying.total} /></div>
    </div>
  {/if}

  <div class="layout" class:with-history={history.length > 1}>
    <div class="main">
      {#if error}
        <div class="error">{error}</div>
      {:else if !current}
        <div class="empty">
          <ShieldCheck size={36} />
          <p class="lead">No receipt yet</p>
          <p class="muted">Verifying checks every file in the folder against its cloud copy by checksum and saves a receipt.</p>
        </div>
      {:else if !receipt}
        <div class="empty muted">Loading…</div>
      {:else}
        <div class="seal" class:bad={!ok}>
          <span class="mark">{#if ok}<ShieldCheck size={30} />{:else}<ShieldAlert size={30} />{/if}</span>
          <div>
            <p class="headline">{ok ? `All ${count(t.verified)} files verified` : `${count(problems)} of ${count(t.local_files)} files not verified`}</p>
            <p class="route" title={receipt.local_root}><span class="dir">From</span><span class="mono truncate start"><bdi>{receipt.local_root}</bdi></span></p>
            <p class="route" title={receipt.destination}><span class="dir">To</span><span class="mono truncate start"><bdi>{receipt.destination}</bdi></span></p>
          </div>
        </div>

        <dl class="facts">
          <div><dt>Verified</dt><dd>{count(t.verified)} files · {bytes(t.verified_bytes)}</dd></div>
          <div><dt>Checked</dt><dd title={dateTime(receipt.finished_at)}>{relativeTime(receipt.finished_at)} · took {duration(Math.max(1, receipt.elapsed_seconds || 0))}</dd></div>
          <div><dt>By</dt><dd class="truncate">{receipt.account}</dd></div>
          <div>
            <dt>How</dt>
            <dd>
              Checksums compared for every file{t.files_read ? ` · ${count(t.files_read)} read now` : ''}{t.local_files - (t.files_read || 0) - (t.missing_in_cloud || 0) > 0 ? ` · ${count(t.local_files - (t.files_read || 0) - (t.missing_in_cloud || 0))} from earlier uploads` : ''}
            </dd>
          </div>
          {#if t.cloud_only}<div><dt>Cloud only</dt><dd>{count(t.cloud_only)} files in the cloud aren't in this folder (not counted)</dd></div>{/if}
        </dl>

        {#if receipt.problems?.length}
          {#if problems > 10}
            <label class="search"><Search size={14} /><input placeholder="Find a file" bind:value={query} aria-label="Find a file" /></label>
          {/if}
          {#each shown as group (group.kind)}
            {@const k = KINDS[group.kind] || { title: group.kind, text: '' }}
            <section class="problem">
              <header>
                <div>
                  <h3>{k.title} <span class="count">{count(group.count)}</span></h3>
                  <p class="muted">{k.text}</p>
                </div>
                {#if k.fix}<button class="btn sm" onclick={() => fix(group.kind)}>{k.fix}</button>{/if}
              </header>
              <div class="rows">
                {#each group.files as f (f.path)}
                  <div class="row" title={f.note || ''}>
                    <span class="mono truncate start"><bdi>{f.path}</bdi></span>
                    <span class="muted num">{bytes(f.size || 0)}</span>
                  </div>
                {:else}
                  <div class="row muted">No matches.</div>
                {/each}
                {#if group.count > group.files.length && !query}<div class="row muted">and {count(group.count - group.files.length)} more (all in the CSV)</div>{/if}
              </div>
            </section>
          {/each}
        {/if}
      {/if}
    </div>

    {#if history.length > 1}
      <aside class="history" aria-label="Earlier receipts">
        <h3>Receipts</h3>
        {#each history as h (h.json)}
          <button class="h" class:on={h.json === current} onclick={() => pick(h.json)}>
            <span class="dot" class:bad={h.result !== 'complete'}></span>
            <span class="when" title={dateTime(h.finished_at)}>{when(h.finished_at)}</span>
            <span class="muted small">{h.result === 'complete' ? `${count(h.totals?.verified)} verified` : `${count(notVerified(h.totals))} not verified`}</span>
          </button>
        {/each}
      </aside>
    {/if}
  </div>

  {#snippet footer()}
    {#if receipt}
      <button class="btn ghost" onclick={copySummary}><Copy size={15} /> Copy summary</button>
      <button class="btn ghost" onclick={() => revealPath(receipt.csv)}><FileSpreadsheet size={15} /> CSV</button>
      <button class="btn ghost" onclick={printable}><Printer size={15} /> Printable receipt</button>
    {/if}
    <span class="grow"></span>
    {#if verifying}
      <button class="btn" onclick={() => app.stopVerifying(preset)}>Stop</button>
    {:else}
      <button class="btn" onclick={() => { followLatest = true; app.createReceipt(preset); }}><RefreshCw size={15} /> {current ? 'Verify again' : 'Verify now'}</button>
    {/if}
    <button class="btn primary" onclick={close}>Done</button>
  {/snippet}
</Sheet>

<style>
  .verifying {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--text-2);
  }
  .verifying .bar {
    flex: 1;
  }
  .layout {
    display: grid;
    gap: 18px;
  }
  .layout.with-history {
    grid-template-columns: minmax(0, 1fr) 200px;
  }
  .main {
    display: flex;
    flex-direction: column;
    gap: 14px;
    min-width: 0;
  }
  .seal {
    display: flex;
    gap: 14px;
    align-items: center;
    padding: 16px;
    border-radius: var(--radius);
    background: var(--success-soft, var(--surface-2));
  }
  .seal.bad {
    background: var(--warning-soft);
  }
  .mark {
    width: 52px;
    height: 52px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    flex: none;
    color: #fff;
    background: var(--success);
  }
  .seal.bad .mark {
    background: var(--warning);
  }
  .seal > div {
    min-width: 0;
  }
  .headline {
    margin: 0;
    font-size: 18px;
    font-weight: 650;
  }
  .route {
    display: flex;
    gap: 6px;
    margin: 2px 0 0;
    font-size: 12px;
    color: var(--text-2);
    min-width: 0;
  }
  .route .dir {
    flex: none;
    width: 34px;
    color: var(--muted);
  }
  .route .mono {
    min-width: 0;
  }
  .facts {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 10px 20px;
    margin: 0;
  }
  .facts dt {
    font-size: 12px;
    color: var(--muted);
  }
  .facts dd {
    margin: 0;
    font-size: 13.5px;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 10px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    color: var(--muted);
  }
  .search input {
    flex: 1;
    border: none;
    outline: none;
    background: none;
    min-height: 32px;
    font: inherit;
    color: var(--text);
  }
  .problem {
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }
  .problem header {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    justify-content: space-between;
    padding: 12px 14px;
    background: var(--surface-2);
  }
  .problem h3 {
    margin: 0;
    font-size: 14px;
  }
  .problem header p {
    margin: 2px 0 0;
    font-size: 12.5px;
  }
  .count {
    display: inline-block;
    min-width: 22px;
    padding: 0 7px;
    margin-left: 4px;
    border-radius: 999px;
    background: var(--warning-soft);
    color: var(--warning);
    font-size: 12px;
    text-align: center;
  }
  .rows {
    max-height: 220px;
    overflow-y: auto;
  }
  .row {
    display: flex;
    gap: 12px;
    justify-content: space-between;
    padding: 5px 14px;
    border-top: 1px solid var(--border);
    font-size: 12.5px;
  }
  .row .mono {
    font-family: var(--mono);
    font-size: 12px;
    min-width: 0;
  }
  .num {
    flex: none;
    font-variant-numeric: tabular-nums;
  }
  .history {
    border-left: 1px solid var(--border);
    padding-left: 14px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-height: 520px;
    overflow-y: auto;
  }
  .history h3 {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted);
    margin: 0 0 6px;
  }
  .h {
    display: grid;
    grid-template-columns: 10px 1fr;
    column-gap: 8px;
    align-items: center;
    text-align: left;
    padding: 6px 8px;
    border: none;
    border-radius: 8px;
    background: none;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
  .h:hover {
    background: var(--surface-2);
  }
  .h.on {
    background: var(--accent-soft);
  }
  .h .small {
    grid-column: 2;
    font-size: 12px;
  }
  .when {
    font-size: 13px;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--success);
  }
  .dot.bad {
    background: var(--warning);
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 6px;
    padding: 28px 12px;
    color: var(--muted);
  }
  .lead {
    margin: 0;
    font-weight: 600;
    color: var(--text);
  }
  .grow {
    flex: 1;
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
