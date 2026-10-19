<script>
  import FolderSearch from '@lucide/svelte/icons/folder-search';
  import FolderSync from '@lucide/svelte/icons/folder-sync';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import Square from '@lucide/svelte/icons/square';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import Archive from '@lucide/svelte/icons/archive';
  import BarList from '../components/BarList.svelte';
  import { app } from '../lib/state.svelte.js';
  import { basename, bytes, count, duration, relativeTime } from '../lib/format.js';
  import { cancelScan, isDesktop, pickFolder, revealPath, startScan } from '../lib/platform.js';

  let path = $state('');
  let report = $state(null);
  let scanning = $state(false);
  let live = $state({ files: 0, bytes: 0 });
  let scanId = $state(null);
  let error = $state('');
  let startedAt = $state(0);
  let elapsed = $state(0);

  const sep = (p) => (p.includes('\\') && !p.includes('/') ? '\\' : '/');
  const join = (root, rel) => `${root.replace(/[\\/]+$/, '')}${sep(root)}${rel.split('/').join(sep(root))}`;

  $effect(() => {
    if (app.scanRequest) {
      const requested = app.scanRequest;
      app.scanRequest = null;
      scan(requested);
    }
  });

  async function choose() {
    const picked = await pickFolder(path, 'Choose a folder to analyze');
    if (picked) scan(picked);
  }

  async function scan(target) {
    if (scanning) return;
    path = target;
    report = null;
    error = '';
    scanning = true;
    live = { files: 0, bytes: 0 };
    startedAt = Date.now();
    const timer = setInterval(() => (elapsed = (Date.now() - startedAt) / 1000), 250);
    const job = startScan(target, (tick) => (live = tick));
    scanId = job.scanId;
    try {
      report = await job.done;
      app.remember('recent_scans', target);
    } catch (err) {
      error = String(err?.message || err);
    } finally {
      clearInterval(timer);
      scanning = false;
      scanId = null;
    }
  }

  let shareOf = (b) => (report?.bytes ? `${Math.round((b / report.bytes) * 100)}%` : '');
</script>

<section class="view">
  <header class="view-head">
    <div>
      <h1>Folder insights</h1>
      <p class="sub">See what a folder holds before you sync it or clean it up. Nothing is changed.</p>
    </div>
    <button class="btn primary" disabled={!isDesktop || scanning} onclick={choose}><FolderSearch size={17} /> Analyze a folder…</button>
  </header>

  {#if !isDesktop}
    <div class="card notice">Folder analysis runs in the JetStream desktop app.</div>
  {:else if scanning}
    <div class="card scanning">
      <div class="spinner"></div>
      <div class="scan-text">
        <strong class="truncate">Analyzing {basename(path)}…</strong>
        <span>{count(live.files)} files · {bytes(live.bytes)} · {duration(elapsed)}</span>
      </div>
      <button class="btn sm" onclick={() => cancelScan(scanId)}><Square size={13} /> Stop</button>
    </div>
  {:else if error}
    <div class="card error">{error}</div>
  {/if}

  {#if report && !scanning}
    <div class="report-head">
      <div class="min0">
        <h2 class="truncate">{basename(report.path)}</h2>
        <div class="mono muted truncate start"><bdi>{report.path}</bdi></div>
      </div>
      <div class="actions">
        <button class="btn ghost sm" onclick={() => revealPath(report.path)}><FolderOpen size={14} /> Show</button>
        <button class="btn sm" onclick={() => scan(report.path)}><RefreshCw size={14} /> Scan again</button>
        <button class="btn primary sm" onclick={() => app.addFolder(report.path)}><FolderSync size={14} /> Sync this folder…</button>
      </div>
    </div>
    {#if report.cancelled}
      <div class="callout">Stopped early — these numbers cover only part of the folder.</div>
    {/if}

    <div class="tiles">
      <div class="card tile hero">
        <span class="label">Total size</span>
        <span class="value">{bytes(report.bytes)}</span>
      </div>
      <div class="card tile">
        <span class="label">Files</span>
        <span class="value">{count(report.files)}</span>
      </div>
      <div class="card tile">
        <span class="label">Folders</span>
        <span class="value">{count(report.folders)}</span>
      </div>
      <div class="card tile">
        <span class="label">Last changed</span>
        <span class="value small">{report.newest_modified ? relativeTime(new Date(report.newest_modified * 1000).toISOString()) : '—'}</span>
      </div>
      <div class="card tile">
        <span class="label">Scan time</span>
        <span class="value small">{duration(report.elapsed_seconds) === '0s' ? '<1s' : duration(report.elapsed_seconds)}</span>
      </div>
    </div>

    <div class="grid">
      <div class="card panel">
        <h3>What's in it</h3>
        <p class="caption">Share of total size by kind of file</p>
        <BarList items={report.categories} format={bytes} detail={(c) => `${count(c.files)} files · ${shareOf(c.bytes)}`} />
      </div>

      <div class="card panel">
        <h3>Biggest subfolders</h3>
        <p class="caption">Click one to analyze it on its own</p>
        <BarList
          items={report.top_folders.slice(0, 10)}
          format={bytes}
          detail={(f) => `${count(f.files)} files · ${shareOf(f.bytes)}`}
          onselect={(f) => f.name !== '(files in this folder)' && scan(join(report.path, f.name))}
        />
      </div>

      <div class="card panel">
        <h3>File types</h3>
        <p class="caption">Largest by total size</p>
        <BarList items={report.extensions.slice(0, 10)} format={bytes} detail={(e) => `${count(e.files)} files`} />
      </div>

      <div class="card panel">
        <h3>Cleanup ideas</h3>
        <p class="caption">Suggestions only — review before deleting anything</p>
        <div class="idea">
          <div class="idea-icon"><Trash2 size={17} /></div>
          <div class="min0">
            <strong>{count(report.junk.files)} temporary or system file{report.junk.files === 1 ? '' : 's'} · {bytes(report.junk.bytes)}</strong>
            <span class="muted">Thumbs.db, .DS_Store, *.tmp, *.bak and similar. Usually safe to skip when syncing.</span>
            {#each report.junk.examples.slice(0, 3) as f (f.path)}
              <span class="mono example truncate start"><bdi>{f.path}</bdi></span>
            {/each}
          </div>
        </div>
        <div class="idea">
          <div class="idea-icon"><Archive size={17} /></div>
          <div class="min0">
            <strong>{count(report.archive_candidates.files)} large file{report.archive_candidates.files === 1 ? '' : 's'} untouched for a year · {bytes(report.archive_candidates.bytes)}</strong>
            <span class="muted">Good candidates for archive storage in the cloud, then removal from this drive.</span>
            {#each report.archive_candidates.examples.slice(0, 3) as f (f.path)}
              <span class="mono example truncate start"><bdi>{f.path} · {bytes(f.bytes)}</bdi></span>
            {/each}
          </div>
        </div>
      </div>

      <div class="card panel wide">
        <h3>Largest files</h3>
        <div class="files">
          {#each report.largest_files.slice(0, 15) as f (f.path)}
            <div class="file">
              <span class="mono truncate start" title={f.path}><bdi>{f.path}</bdi></span>
              <span class="muted">{f.modified ? relativeTime(new Date(f.modified * 1000).toISOString()) : ''}</span>
              <span class="size">{bytes(f.bytes)}</span>
              <button class="btn ghost sm icon" aria-label="Show file" onclick={() => revealPath(join(report.path, f.path))}><FolderOpen size={14} /></button>
            </div>
          {/each}
        </div>
      </div>
    </div>
  {:else if !scanning && !error}
    <div class="card empty">
      <FolderSearch size={30} />
      <p>Choose a folder, or drop one onto the window.</p>
      {#if app.prefs.recent_scans?.length}
        <div class="recent">
          {#each app.prefs.recent_scans as recent (recent)}
            <button class="chip" onclick={() => scan(recent)} title={recent}>{basename(recent)}</button>
          {/each}
        </div>
      {/if}
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
    margin-bottom: 20px;
  }
  h1 {
    font-size: 24px;
  }
  h2 {
    font-size: 19px;
  }
  h3 {
    font-size: 14.5px;
  }
  .sub {
    margin: 4px 0 0;
    color: var(--muted);
  }
  .muted,
  .caption {
    color: var(--muted);
    font-size: 12.5px;
  }
  .caption {
    margin: 2px 0 10px;
  }
  .min0 {
    min-width: 0;
  }
  .notice,
  .empty {
    padding: 40px;
    color: var(--muted);
    text-align: center;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
  }
  .error {
    padding: 14px 16px;
    color: var(--danger);
    background: var(--danger-soft);
  }
  .scanning {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 18px 20px;
  }
  .scan-text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .scan-text span {
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }
  .spinner {
    width: 26px;
    height: 26px;
    border: 3px solid var(--border-strong);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  .report-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-end;
    gap: 16px;
    margin-bottom: 14px;
  }
  .actions {
    display: flex;
    gap: 8px;
    flex-shrink: 0;
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
    grid-template-columns: 1.4fr repeat(4, 1fr);
    gap: 12px;
    margin-bottom: 16px;
  }
  .tile {
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .label {
    color: var(--muted);
    font-size: 12.5px;
    font-weight: 600;
  }
  .value {
    font-size: 22px;
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .hero .value {
    font-size: 30px;
  }
  .value.small {
    font-size: 17px;
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 16px;
  }
  .panel {
    padding: 16px 18px;
    min-width: 0;
  }
  .wide {
    grid-column: 1 / -1;
  }
  .idea {
    display: flex;
    gap: 12px;
    padding: 10px 0;
    border-top: 1px solid var(--border);
  }
  .idea:first-of-type {
    border-top: none;
  }
  .idea > div:last-child {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .idea-icon {
    width: 32px;
    height: 32px;
    border-radius: 8px;
    background: var(--surface-2);
    color: var(--text-2);
    display: grid;
    place-items: center;
    flex-shrink: 0;
  }
  .example {
    color: var(--text-2);
    font-size: 12px;
  }
  .files {
    display: flex;
    flex-direction: column;
  }
  .file {
    display: grid;
    grid-template-columns: 1fr 110px 90px 32px;
    align-items: center;
    gap: 12px;
    padding: 5px 0;
    border-top: 1px solid var(--border);
  }
  .size {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .recent {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    justify-content: center;
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
</style>
