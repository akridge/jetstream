<script>
  // "Fix file details": give files already in the cloud their file type and, from the synced
  // folder on this computer, their original dates. One sheet from start to finish:
  // checking → review (what changes, with examples) → updating → done. Nothing is re-uploaded.
  import { onMount, untrack } from 'svelte';
  import FileCheck from '@lucide/svelte/icons/file-check';
  import CalendarClock from '@lucide/svelte/icons/calendar-clock';
  import FileType from '@lucide/svelte/icons/file-type';
  import CircleCheck from '@lucide/svelte/icons/circle-check';
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ArrowRight from '@lucide/svelte/icons/arrow-right';
  import Sheet from './Sheet.svelte';
  import ProgressBar from './ProgressBar.svelte';
  import { app, commaList, lines } from '../lib/state.svelte.js';
  import { cloudOps } from '../lib/cloudops.svelte.js';
  import { count, duration } from '../lib/format.js';
  import { cancelCloudOp, newOpId, planFixDetails, runFixDetails } from '../lib/platform.js';

  let { dialog } = $props();
  // Kept from the start: closing clears the dialog prop.
  const destination = untrack(() => dialog.destination);
  const preset = untrack(() => dialog.preset || null);

  let phase = $state('checking'); // checking | review | updating | done | error
  let opId = $state('');
  let tick = $state(null);
  let plan = $state(null);
  let result = $state(null);
  let error = $state('');
  let showLeft = $state(false);
  let filter = $state('all');
  let startedAt = 0;

  let changes = $derived(plan ? plan.changes ?? Math.max(plan.types, plan.dates) : 0);
  let topTypes = $derived(plan ? Object.entries(plan.by_type).sort((a, b) => b[1] - a[1]).slice(0, 5) : []);
  let examples = $derived(
    (plan?.examples || []).filter((e) => (filter === 'dates' ? e.modified || e.created : filter === 'types' ? e.content_type : true)),
  );
  let rate = $derived(phase === 'updating' && tick?.done && startedAt ? tick.done / Math.max(0.5, (Date.now() - startedAt) / 1000) : 0);

  onMount(check);

  async function check() {
    phase = 'checking';
    plan = null;
    result = null;
    error = '';
    tick = null;
    opId = newOpId();
    try {
      const o = preset?.options || {};
      plan = await planFixDetails(
        opId,
        destination,
        preset ? { localPath: preset.source_path, excludePatterns: lines(o.exclude_patterns) || [], excludeFolders: commaList(o.exclude_folders) || [], recursive: o.recursive !== false } : {},
        (t) => (tick = t),
      );
      phase = 'review';
    } catch (err) {
      const message = String(err?.message || err);
      if (/stopped/i.test(message)) close();
      else {
        error = message;
        phase = 'error';
      }
    }
  }

  async function update() {
    phase = 'updating';
    tick = { done: 0, total: changes, skipped: 0, failed: 0 };
    startedAt = Date.now();
    try {
      result = await runFixDetails(opId, (t) => (tick = t));
      phase = 'done';
    } catch (err) {
      error = String(err?.message || err);
      phase = 'error';
    } finally {
      cloudOps.version++;
    }
  }

  function stop() {
    cancelCloudOp(opId);
  }

  function close() {
    if (phase === 'checking' && opId) cancelCloudOp(opId);
    app.fixDetails = null;
  }

  function openInCloud() {
    const [bucket, ...rest] = destination.replace(/^gs:\/\//, '').replace(/\/+$/, '').split('/');
    app.cloudLocation = { bucket, prefix: rest.length ? `${rest.join('/')}/` : '' };
    app.view = 'cloud';
    close();
  }

  function copyFailures() {
    navigator.clipboard?.writeText(result.failures.map((f) => `${f.path}\t${f.error}`).join('\n'));
    app.toast('Copied the list', 'info', { timeout: 2000 });
  }

  const date = (secs) => (secs ? new Date(secs * 1000).toLocaleString([], { year: 'numeric', month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' }) : '');
  const fileName = (name) => name.slice((plan?.prefix || '').length);
  let stageText = $derived.by(() => {
    if (!tick?.stage) return 'Starting…';
    if (tick.stage === 1) return `Listing the cloud folder · ${count(tick.done)} files so far`;
    if (tick.stage === 2) return 'Looking through the folder on this computer…';
    return `Reading files to confirm they match · ${count(tick.done)} of ${count(tick.total)}`;
  });
</script>

<Sheet title="Fix file details" subtitle={destination} width={700} locked={phase === 'updating'} onclose={close}>
  {#snippet icon()}<span class="badge"><FileCheck size={18} /></span>{/snippet}

  {#if phase === 'checking'}
    <div class="center" aria-live="polite">
      <p class="lead">Checking what's missing…</p>
      <ProgressBar value={tick?.stage === 3 && tick.total ? (100 * tick.done) / tick.total : 0} indeterminate={tick?.stage !== 3} />
      <p class="muted small">{stageText}</p>
      {#if preset}<p class="muted small">Dates come from <span class="mono">{preset.source_path}</span>, only for files that match their cloud copy.</p>{/if}
    </div>
  {:else if phase === 'error'}
    <div class="error"><TriangleAlert size={16} /> {error}</div>
  {:else if phase === 'review' && !plan.types && !plan.dates}
    <div class="center">
      <span class="big-ok"><CircleCheck size={40} /></span>
      <p class="lead">Nothing to fix</p>
      <p class="muted">All {count(plan.objects)} files already have their file type{preset ? ' and original dates' : ''}.</p>
    </div>
  {:else if phase === 'review'}
    <div class="cards">
      {#if plan.dates}
        <div class="stat">
          <CalendarClock size={18} />
          <div>
            <strong>{count(plan.dates)} {plan.dates === 1 ? 'file gets its' : 'files get their'} original dates</strong>
            <span class="muted">Created and modified, from this computer</span>
          </div>
        </div>
      {/if}
      {#if plan.types}
        <div class="stat">
          <FileType size={18} />
          <div>
            <strong>{count(plan.types)} {plan.types === 1 ? 'file gets its' : 'files get their'} file type</strong>
            <span class="muted">{topTypes.map(([t, n]) => `${t.split('/').pop()} ${count(n)}`).join(' · ')}{Object.keys(plan.by_type).length > topTypes.length ? ' · …' : ''}</span>
          </div>
        </div>
      {/if}
    </div>

    <div class="examples">
      <div class="examples-head">
        <span class="label">What changes</span>
        {#if plan.dates && plan.types}
          <div class="segmented small">
            {#each [['all', 'All'], ['dates', 'Dates'], ['types', 'Types']] as [id, label] (id)}
              <button class:active={filter === id} onclick={() => (filter = id)}>{label}</button>
            {/each}
          </div>
        {/if}
      </div>
      <div class="table" role="table">
        <div class="tr th" role="row"><span role="columnheader">File</span><span role="columnheader">Now</span><span></span><span role="columnheader">After</span></div>
        {#each examples.slice(0, 200) as e (e.name)}
          <div class="tr" role="row">
            <span class="mono truncate" role="cell" title={fileName(e.name)}>{fileName(e.name)}</span>
            <span class="muted" role="cell">
              {#if e.content_type}<span class="mono">{e.current_type || 'no type'}</span>{/if}
              {#if e.modified || e.created}<span>{e.current_modified ? date(e.current_modified) : 'upload date only'}</span>{/if}
            </span>
            <span class="arrow" aria-hidden="true"><ArrowRight size={13} /></span>
            <span role="cell">
              {#if e.content_type}<span class="mono new">{e.content_type}</span>{/if}
              {#if e.modified}<span class="new">modified {date(e.modified)}</span>{/if}
              {#if e.created}<span class="new">created {date(e.created)}</span>{/if}
            </span>
          </div>
        {/each}
      </div>
      {#if changes > (plan.examples?.length || 0)}<p class="muted small">Showing {count(plan.examples.length)} of {count(changes)}.</p>{/if}
    </div>

    {#if preset && (plan.not_local || plan.different || plan.read_files)}
      <button class="disclosure" onclick={() => (showLeft = !showLeft)} aria-expanded={showLeft}>
        {#if showLeft}<ChevronDown size={14} />{:else}<ChevronRight size={14} />{/if} Files left as they are
      </button>
      {#if showLeft}
        <ul class="left">
          {#if plan.not_local}<li><strong>{count(plan.not_local)}</strong> in the cloud but not in this folder: there's no original file to take dates from.</li>{/if}
          {#if plan.different}<li><strong>{count(plan.different)}</strong> differ from the file on this computer, so they aren't the same file. Sync the folder first.</li>{/if}
          {#if plan.read_files}<li>{count(plan.read_files)} files were read to make sure they match their cloud copy.</li>{/if}
        </ul>
      {/if}
    {/if}
    <p class="note">
      Nothing is uploaded again: only each file's details change. Google's Cloud console will show “Last modified: today” for these files,
      because that's when their details changed. The original dates show in JetStream and under each file's custom metadata.
    </p>
  {:else if phase === 'updating'}
    <div class="center" aria-live="polite">
      <p class="lead">Updating file details…</p>
      <ProgressBar value={tick?.total ? (100 * ((tick.done || 0) + (tick.skipped || 0) + (tick.failed || 0))) / tick.total : 0} />
      <p class="muted small">
        {count(tick?.done || 0)} of {count(tick?.total || changes)} updated{rate ? ` · ${count(Math.round(rate))} a second` : ''}{tick?.failed ? ` · ${count(tick.failed)} failed` : ''}
      </p>
    </div>
  {:else if phase === 'done'}
    <div class="center">
      {#if result.failed}
        <span class="big-warn"><TriangleAlert size={40} /></span>
        <p class="lead">{count(result.done)} updated, {count(result.failed)} failed</p>
      {:else if result.cancelled}
        <span class="big-warn"><TriangleAlert size={40} /></span>
        <p class="lead">Stopped after {count(result.done)} files</p>
      {:else}
        <span class="big-ok"><CircleCheck size={40} /></span>
        <p class="lead">{count(result.done)} {result.done === 1 ? 'file' : 'files'} updated</p>
        <p class="muted">
          {[plan.dates ? `${count(plan.dates)} now show their original dates` : '', plan.types ? `${count(plan.types)} have their file type` : ''].filter(Boolean).join(' · ')}
        </p>
      {/if}
      <p class="muted small">
        Took {duration(Math.max(1, result.elapsed_seconds))}{result.skipped ? ` · ${count(result.skipped)} had changed since the check and were left alone` : ''}
      </p>
    </div>
    {#if result.failures?.length}
      <div class="failures">
        {#each result.failures as f (f.path)}
          <div class="failure"><span class="mono truncate" title={f.path}>{f.path.replace(/^gs:\/\/[^/]+\//, '')}</span><span class="muted">{f.error}</span></div>
        {/each}
      </div>
    {/if}
  {/if}

  {#snippet footer()}
    {#if phase === 'checking'}
      <span class="grow"></span>
      <button class="btn ghost" onclick={stop}>Stop</button>
    {:else if phase === 'review' && (plan.types || plan.dates)}
      <span class="grow"></span>
      <button class="btn ghost" onclick={close}>Cancel</button>
      <button class="btn primary" onclick={update}><FileCheck size={15} /> Update {count(changes)} {changes === 1 ? 'file' : 'files'}</button>
    {:else if phase === 'updating'}
      <span class="grow"></span>
      <button class="btn ghost" onclick={stop}>Stop</button>
    {:else if phase === 'done'}
      {#if result.failures?.length}<button class="btn ghost" onclick={copyFailures}>Copy list</button>{/if}
      <span class="grow"></span>
      {#if result.failed || result.cancelled}<button class="btn" onclick={check}>Check again</button>{/if}
      <button class="btn" onclick={openInCloud}>Open in Cloud storage</button>
      <button class="btn primary" onclick={close}>Done</button>
    {:else}
      <span class="grow"></span>
      {#if phase === 'error'}<button class="btn" onclick={check}>Try again</button>{/if}
      <button class="btn primary" onclick={close}>Close</button>
    {/if}
  {/snippet}
</Sheet>

<style>
  .badge {
    width: 34px;
    height: 34px;
    border-radius: 10px;
    display: grid;
    place-items: center;
    background: var(--accent-soft);
    color: var(--accent);
  }
  .center {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 10px;
    padding: 18px 8px;
  }
  .center :global(.track) {
    width: 100%;
  }
  .lead {
    font-size: 16px;
    font-weight: 600;
    margin: 0;
  }
  .big-ok {
    color: var(--success, #1a8754);
  }
  .big-warn {
    color: var(--warning, #b4590a);
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
    gap: 10px;
  }
  .stat {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 12px 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--surface-2);
    color: var(--accent);
  }
  .stat div {
    display: flex;
    flex-direction: column;
    gap: 2px;
    color: var(--text);
  }
  .stat .muted {
    font-size: 12.5px;
  }
  .examples-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 6px;
  }
  .segmented {
    display: inline-flex;
    padding: 2px;
    border-radius: 8px;
    background: var(--surface-2);
    border: 1px solid var(--border);
  }
  .segmented button {
    border: none;
    background: none;
    color: var(--text-2);
    font: inherit;
    font-size: 12px;
    padding: 3px 10px;
    border-radius: 6px;
    cursor: pointer;
  }
  .segmented button.active {
    background: var(--surface);
    color: var(--text);
    font-weight: 600;
    box-shadow: var(--shadow);
  }
  .table {
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    max-height: 260px;
    overflow-y: auto;
  }
  .tr {
    display: grid;
    grid-template-columns: minmax(0, 1.3fr) minmax(0, 1fr) 16px minmax(0, 1.2fr);
    gap: 8px;
    align-items: center;
    padding: 6px 10px;
    border-top: 1px solid var(--border);
    font-size: 12.5px;
  }
  .tr span[role='cell'] {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .tr.th {
    position: sticky;
    top: 0;
    border-top: none;
    background: var(--surface-2);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--muted);
  }
  .arrow {
    color: var(--muted);
  }
  .new {
    color: var(--text);
  }
  .mono {
    font-family: var(--mono);
    font-size: 11.5px;
  }
  .disclosure {
    align-self: flex-start;
    border: none;
    background: none;
    color: var(--text-2);
    font: inherit;
    font-size: 13px;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 0;
    cursor: pointer;
  }
  .left {
    margin: -4px 0 0;
    padding-left: 20px;
    font-size: 13px;
    color: var(--text-2);
    display: grid;
    gap: 4px;
  }
  .note {
    font-size: 12.5px;
    margin: 0;
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    background: var(--surface-2);
    color: var(--text-2);
  }
  .failures {
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    max-height: 200px;
    overflow-y: auto;
  }
  .failure {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    gap: 10px;
    padding: 6px 10px;
    border-top: 1px solid var(--border);
    font-size: 12.5px;
  }
  .failure:first-child {
    border-top: none;
  }
  .grow {
    flex: 1;
  }
  .small {
    font-size: 12.5px;
    margin: 0;
  }
  .muted {
    color: var(--muted);
  }
  .error {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    color: var(--danger);
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    background: var(--danger-soft);
  }
</style>
