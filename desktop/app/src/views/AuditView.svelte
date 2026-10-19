<script>
  import Brush from '@lucide/svelte/icons/brush';
  import Square from '@lucide/svelte/icons/square';
  import Plus from '@lucide/svelte/icons/plus';
  import X from '@lucide/svelte/icons/x';
  import ShieldAlert from '@lucide/svelte/icons/shield-alert';
  import Copy from '@lucide/svelte/icons/copy';
  import BarList from '../components/BarList.svelte';
  import QuarantineDialog from '../components/QuarantineDialog.svelte';
  import { app } from '../lib/state.svelte.js';
  import { api } from '../lib/api.js';
  import { bytes, count, duration, relativeTime } from '../lib/format.js';
  import { cancelAudit, defaultJunkPatterns, isDesktop, startAudit } from '../lib/platform.js';

  const LABELS = {
    '\\.gstmp$': 'Unfinished uploads (.gstmp)',
    '(^|/)@eaDir/': 'NAS thumbnail folders (@eaDir)',
    '(^|/)Thumbs\\.db$': 'Windows thumbnail caches (Thumbs.db)',
    '(^|/)\\.DS_Store$': 'macOS folder files (.DS_Store)',
    '(^|/)desktop\\.ini$': 'Windows folder settings (desktop.ini)',
    '(^|/)~\\$': 'Office lock files (~$…)',
    '\\.(tmp|temp|crdownload|part)$': 'Temporary and partial downloads',
  };
  const label = (pattern) => LABELS[pattern] || pattern;

  let location = $state('');
  let patterns = $state([]);
  let enabled = $state({});
  let newPattern = $state('');
  let patternError = $state('');
  let defaults = $state(null);

  let running = $state(false);
  let live = $state({ scanned: 0, found: 0 });
  let auditId = $state(null);
  let startedAt = 0;
  let elapsed = $state(0);
  let report = $state(null);
  let error = $state('');
  let runId = $state(null);
  let filter = $state('');
  let showQuarantine = $state(false);

  $effect(() => {
    if (!isDesktop) return;
    defaultJunkPatterns().then((list) => {
      patterns = list;
      enabled = Object.fromEntries(list.map((p) => [p, true]));
    });
    api.auditDefaults().then((d) => (defaults = d)).catch(() => {});
  });

  let active = $derived(patterns.filter((p) => enabled[p]));
  let shortcuts = $derived(app.prefs.recent_buckets?.length ? app.prefs.recent_buckets : []);
  let shown = $derived(
    report
      ? report.findings.filter((f) => !filter || f.matched_pattern === filter).slice(0, 300)
      : [],
  );

  function addPattern() {
    const value = newPattern.trim();
    if (!value) return;
    try {
      new RegExp(value);
    } catch (err) {
      patternError = `Not a valid pattern: ${err.message}`;
      return;
    }
    patternError = '';
    if (!patterns.includes(value)) patterns = [...patterns, value];
    enabled = { ...enabled, [value]: true };
    newPattern = '';
  }

  async function scan() {
    const clean = location.trim().replace(/^gs:\/\//, '').replace(/^\/+/, '');
    const [bucket, ...rest] = clean.split('/');
    if (!bucket || !active.length) return;
    error = '';
    report = null;
    runId = null;
    filter = '';
    running = true;
    live = { scanned: 0, found: 0 };
    startedAt = Date.now();
    const timer = setInterval(() => (elapsed = (Date.now() - startedAt) / 1000), 250);
    const job = startAudit(bucket, rest.join('/'), active, (tick) => (live = tick));
    auditId = job.auditId;
    try {
      report = await job.done;
      app.remember('recent_buckets', bucket);
    } catch (err) {
      error = String(err?.message || err);
    } finally {
      clearInterval(timer);
      running = false;
    }
  }

  function copyList() {
    navigator.clipboard?.writeText(report.findings.map((f) => `gs://${report.bucket}/${f.object_name}`).join('\n'));
    app.toast(`Copied ${count(report.findings.length)} paths`, 'info', { timeout: 2500 });
  }
</script>

<section class="view">
  <header class="view-head">
    <div>
      <h1>Cloud cleanup</h1>
      <p class="sub">Find temporary and system files left in a bucket, then move them to quarantine. Scanning changes nothing.</p>
    </div>
  </header>

  {#if !isDesktop}
    <div class="card notice">Bucket cleanup scans run in the JetStream desktop app.</div>
  {:else}
    <div class="card setup">
      <form class="locationbar" onsubmit={(e) => { e.preventDefault(); scan(); }}>
        <input class="input mono" bind:value={location} placeholder="gs://bucket/optional/folder/" aria-label="Bucket or folder to scan" />
        <button class="btn primary" type="submit" disabled={running || !location.trim() || !active.length}><Brush size={16} /> Scan for junk</button>
      </form>
      {#if shortcuts.length}
        <div class="chips">
          {#each shortcuts as b (b)}
            <button class="chip mono" onclick={() => (location = `gs://${b}/`)}>{b}</button>
          {/each}
        </div>
      {/if}
      <div class="patterns">
        <span class="label">Look for</span>
        <div class="pattern-list">
          {#each patterns as p (p)}
            <label class="pattern" class:off={!enabled[p]} title={p}>
              <input type="checkbox" bind:checked={enabled[p]} />
              {label(p)}
            </label>
          {/each}
        </div>
        <form class="add" onsubmit={(e) => { e.preventDefault(); addPattern(); }}>
          <input class="input mono" bind:value={newPattern} placeholder="Custom pattern, e.g. \.bak$" aria-label="Custom pattern" />
          <button class="btn sm" type="submit"><Plus size={14} /> Add</button>
        </form>
        {#if patternError}<span class="hint bad">{patternError}</span>{/if}
      </div>
    </div>

    {#if running}
      <div class="card running">
        <div class="spinner"></div>
        <div class="grow">
          <strong>Scanning {location.trim()}…</strong>
          <span class="muted">{count(live.scanned)} objects checked · {count(live.found)} junk found · {duration(elapsed)}</span>
        </div>
        <button class="btn sm" onclick={() => cancelAudit(auditId)}><Square size={13} /> Stop</button>
      </div>
    {:else if error}
      <div class="card error">{error}</div>
    {/if}

    {#if report && !running}
      {#if report.cancelled}<div class="callout">Stopped early — results cover only part of the bucket.</div>{/if}
      <div class="tiles">
        <div class="card tile">
          <span class="label">Checked</span>
          <span class="value">{count(report.scanned_objects)}</span>
          <span class="muted">objects · {bytes(report.scanned_bytes)}</span>
        </div>
        <div class="card tile hero">
          <span class="label">Junk found</span>
          <span class="value">{count(report.junk_objects)}</span>
          <span class="muted">objects · {bytes(report.junk_bytes)}</span>
        </div>
        <div class="card tile">
          <span class="label">Scan time</span>
          <span class="value">{report.elapsed_seconds < 1 ? '<1s' : duration(report.elapsed_seconds)}</span>
          <span class="muted">gs://{report.bucket}/{report.prefix}</span>
        </div>
      </div>

      {#if report.junk_objects}
        <div class="grid">
          <div class="card panel">
            <h3>By kind</h3>
            <p class="caption">Click one to filter the list</p>
            <BarList
              items={report.by_pattern}
              label={(p) => label(p.pattern)}
              value={(p) => p.bytes}
              format={bytes}
              detail={(p) => `${count(p.objects)} objects`}
              onselect={(p) => (filter = filter === p.pattern ? '' : p.pattern)}
            />
          </div>
          <div class="card panel act">
            <h3>Next step</h3>
            <p class="caption">Quarantine keeps a copy you can restore, instead of deleting.</p>
            <button class="btn primary" onclick={() => (showQuarantine = true)}><ShieldAlert size={16} /> Move to quarantine…</button>
            <button class="btn ghost" onclick={copyList}><Copy size={15} /> Copy list of paths</button>
            {#if runId}<span class="muted">Saved as audit run {runId.slice(0, 8)} (also visible on the classic Cloud audit page).</span>{/if}
          </div>
        </div>

        <div class="card panel">
          <div class="list-head">
            <h3>Findings {filter ? `· ${label(filter)}` : ''}</h3>
            {#if filter}<button class="btn ghost sm" onclick={() => (filter = '')}><X size={14} /> Clear filter</button>{/if}
          </div>
          <div class="findings">
            {#each shown as f (f.object_name)}
              <div class="finding">
                <span class="mono truncate start" title={f.object_name}><bdi>{f.object_name}</bdi></span>
                <span class="muted truncate">{label(f.matched_pattern)}</span>
                <span class="muted">{f.updated ? relativeTime(f.updated) : ''}</span>
                <span class="size">{bytes(f.size)}</span>
              </div>
            {/each}
          </div>
          {#if report.findings.length > shown.length || report.findings_truncated}
            <p class="caption">Showing {count(shown.length)} of {count(filter ? report.findings.filter((f) => f.matched_pattern === filter).length : report.junk_objects)}.</p>
          {/if}
        </div>
      {:else}
        <div class="card notice">No junk found. 🎉</div>
      {/if}
    {/if}
  {/if}
</section>

{#if showQuarantine && report}
  <QuarantineDialog {report} {defaults} bind:runId onclose={() => (showQuarantine = false)} />
{/if}

<style>
  .view {
    padding: 28px 32px 40px;
    max-width: 1200px;
  }
  .view-head {
    margin-bottom: 18px;
  }
  h1 {
    font-size: 24px;
  }
  h3 {
    font-size: 14.5px;
  }
  .sub {
    margin: 4px 0 0;
    color: var(--muted);
  }
  .notice {
    padding: 28px;
    text-align: center;
    color: var(--muted);
  }
  .setup {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    margin-bottom: 16px;
  }
  .locationbar,
  .add {
    display: flex;
    gap: 8px;
  }
  .chips,
  .pattern-list {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    height: 26px;
    padding: 0 10px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--surface-2);
    font-size: 12px;
    color: var(--text-2);
    cursor: pointer;
  }
  .patterns {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .label {
    font-weight: 600;
    font-size: 13px;
    color: var(--text-2);
  }
  .pattern {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 30px;
    padding: 0 10px;
    border: 1px solid var(--accent);
    border-radius: 999px;
    background: var(--accent-soft);
    font-size: 12.5px;
    cursor: pointer;
  }
  .pattern.off {
    border-color: var(--border);
    background: var(--surface);
    color: var(--muted);
  }
  .pattern input {
    accent-color: var(--accent);
  }
  .add {
    max-width: 480px;
  }
  .bad {
    color: var(--danger);
  }
  .running {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 16px 18px;
    margin-bottom: 16px;
  }
  .grow {
    flex: 1;
    display: flex;
    flex-direction: column;
  }
  .spinner {
    width: 24px;
    height: 24px;
    border: 3px solid var(--border-strong);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  .error {
    padding: 14px 16px;
    margin-bottom: 16px;
    background: var(--danger-soft);
    color: var(--danger);
  }
  .callout {
    margin-bottom: 14px;
    padding: 10px 12px;
    border-radius: var(--radius-sm);
    background: var(--warning-soft);
    color: var(--warning);
  }
  .tiles {
    display: grid;
    grid-template-columns: 1fr 1.3fr 1fr;
    gap: 12px;
    margin-bottom: 16px;
  }
  .tile {
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .value {
    font-size: 22px;
    font-weight: 650;
  }
  .hero .value {
    font-size: 30px;
  }
  .muted,
  .caption {
    color: var(--muted);
    font-size: 12.5px;
  }
  .tile .muted {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .caption {
    margin: 2px 0 10px;
  }
  .grid {
    display: grid;
    grid-template-columns: 1.4fr 1fr;
    gap: 16px;
    margin-bottom: 16px;
  }
  .panel {
    padding: 16px 18px;
  }
  .act {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
  }
  .list-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 8px;
  }
  .findings {
    max-height: 420px;
    overflow-y: auto;
  }
  .finding {
    display: grid;
    grid-template-columns: 1fr 220px 100px 80px;
    gap: 12px;
    align-items: center;
    padding: 6px 0;
    border-top: 1px solid var(--border);
  }
  .size {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
</style>
