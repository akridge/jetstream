<script>
  // Catalogs: list the files under a bucket folder (images by default, any kind as an
  // option), group them, and export the OASIS JSON (the same file the classic catalog tool
  // makes), a detailed CSV/JSON, or save the catalog. Built by the Rust core; regrouping
  // doesn't list the bucket again.
  import { onDestroy, onMount } from 'svelte';
  import { SvelteSet } from 'svelte/reactivity';
  import LibraryBig from '@lucide/svelte/icons/library-big';
  import Square from '@lucide/svelte/icons/square';
  import FileJson from '@lucide/svelte/icons/file-braces';
  import Sheet from '@lucide/svelte/icons/sheet';
  import Save from '@lucide/svelte/icons/save';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import Images from '@lucide/svelte/icons/images';
  import ThumbTile from '../components/ThumbTile.svelte';
  import CatalogGallery from '../components/CatalogGallery.svelte';
  import DatasetInfoCard from '../components/DatasetInfoCard.svelte';
  import { app } from '../lib/state.svelte.js';
  import { api } from '../lib/api.js';
  import { bytes, count, relativeTime } from '../lib/format.js';
  import { cancelCloudOp, catalogDiscard, catalogExport, catalogKinds, catalogRegroup, catalogSave, catalogScan, isDesktop, newOpId, revealPath } from '../lib/platform.js';

  // ----- what to list -----
  let prefix = $state(app.catalogPrefill || '');
  let kinds = $state(['images']);
  let kindList = $state([]);
  let customExt = $state('');
  let moreFilters = $state(false);
  let filenameFilter = $state('');
  let folderFilter = $state('');
  let ignoreFolders = $state('');
  let maxDepth = $state('');
  let maxResults = $state('');

  // ----- how to group -----
  let format = $state('both');
  let groupBy = $state('filename_token');
  let delimiter = $state('_');
  let tokenCount = $state(3);
  let folderIndex = $state(0);
  let regexPattern = $state('');
  let regexIndex = $state(1);
  let manualText = $state('');
  let siteMode = $state('none');
  let siteThreshold = $state(25);
  let sitePattern = $state('');
  let siteFolderIndex = $state(0);
  const excluded = new SvelteSet();
  const manualSites = new SvelteSet();

  // ----- result -----
  let opId = $state('');
  let scanning = $state(false);
  let tick = $state(null);
  let summary = $state(null);
  let error = $state('');
  let regrouping = $state(false);
  let gallery = $state(undefined); // group key, null for all files, undefined when closed
  let sortBy = $state('key');
  let rowsShown = $state(100);
  let dataset = $state(null);
  let sidecar = $state(true);
  let saveName = $state('');
  let saved = $state([]);
  let savedError = $state('');

  let bucket = $derived(summary ? summary.gcs_prefix.slice(5).split('/')[0] : '');
  let shortcuts = $derived((app.prefs.recent_buckets || []).slice(0, 8));
  let rows = $derived.by(() => {
    if (!summary) return [];
    const list = [...summary.groups];
    if (sortBy === 'count') list.sort((a, b) => b.count - a.count || a.key.localeCompare(b.key));
    return list;
  });
  let canSelect = $derived(summary && summary.all_groups <= summary.groups.length);
  /** Bytes in the catalog: the included groups' (all rows are here when some are left out). */
  let includedBytes = $derived(summary ? (summary.groups.some((g) => !g.included) ? summary.groups.reduce((n, g) => n + (g.included ? g.bytes : 0), 0) : summary.matched_bytes) : 0);

  onMount(async () => {
    app.catalogPrefill = null;
    kindList = await catalogKinds().catch(() => []);
    loadSaved();
  });
  onDestroy(() => catalogDiscard(opId));

  function manualOverrides() {
    // "Group: pattern, pattern" per line, in order.
    return manualText
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => {
        const at = line.indexOf(':');
        const group = (at > 0 ? line.slice(0, at) : line).trim();
        const patterns = (at > 0 ? line.slice(at + 1) : line).split(',').map((p) => p.trim()).filter(Boolean);
        return [group, patterns];
      })
      .filter(([g, p]) => g && p.length);
  }

  /** Grouping without the group selection (changing it starts the selection over). */
  function baseGrouping() {
    return {
      catalog_format: format,
      group_by_mode: groupBy,
      filename_delimiter: delimiter,
      filename_token_count: Math.max(1, Number(tokenCount) || 1),
      folder_segment_index: Math.max(0, Number(folderIndex) || 0),
      regex_pattern: regexPattern || null,
      regex_group_index: Math.max(0, Number(regexIndex) || 0),
      manual_overrides: manualOverrides(),
      site_instance_mode: siteMode,
      site_instance_threshold: Math.max(1, Number(siteThreshold) || 1),
      site_instance_pattern: sitePattern || null,
      site_instance_folder_segment_index: Math.max(0, Number(siteFolderIndex) || 0),
    };
  }

  function grouping() {
    const keys = summary ? summary.groups.map((g) => g.key) : [];
    return {
      ...baseGrouping(),
      selected_groups: excluded.size ? keys.filter((k) => !excluded.has(k)) : [],
      manual_site_groups: [...manualSites],
    };
  }

  function request() {
    const list = (text) => text.split(',').map((s) => s.trim()).filter(Boolean);
    return {
      gcs_prefix: prefix.trim().startsWith('gs://') ? prefix.trim() : `gs://${prefix.trim()}`,
      kinds: kinds.length ? kinds : [],
      extensions: list(customExt),
      filename_filter: filenameFilter.trim() || null,
      folder_filter: folderFilter.trim() || null,
      ignore_folder_names: list(ignoreFolders),
      max_path_depth: maxDepth === '' ? null : Math.max(0, Number(maxDepth) || 0),
      max_results: Math.max(0, Number(maxResults) || 0),
      ...grouping(),
    };
  }

  function toggleKind(id) {
    if (id === 'all') kinds = kinds.includes('all') ? ['images'] : ['all'];
    else {
      const without = kinds.filter((k) => k !== 'all' && k !== id);
      kinds = kinds.includes(id) ? without : [...without, id];
    }
  }

  async function scan() {
    if (!prefix.trim()) return;
    if (!kinds.length && !customExt.trim()) {
      error = 'Choose at least one kind of file, or enter extensions.';
      return;
    }
    catalogDiscard(opId);
    excluded.clear();
    manualSites.clear();
    error = '';
    summary = null;
    gallery = undefined;
    tick = null;
    scanning = true;
    opId = newOpId();
    // The new listing is built with this grouping; later changes regroup it.
    const req = request();
    lastBase = JSON.stringify(baseGrouping());
    lastSent = JSON.stringify(grouping());
    try {
      summary = await catalogScan(opId, req, (t) => (tick = t));
      rowsShown = 100;
      const b = summary.gcs_prefix.slice(5).split('/')[0];
      if (b) app.remember?.('recent_buckets', b);
    } catch (err) {
      error = /stopped/.test(String(err)) ? '' : String(err?.message || err);
    } finally {
      scanning = false;
    }
  }

  function stop() {
    cancelCloudOp(opId);
  }

  // Regroup when the grouping changes (debounced); no listing.
  let lastBase = '';
  let lastSent = '';
  let timer;
  $effect(() => {
    const base = JSON.stringify(baseGrouping());
    const selection = JSON.stringify([[...excluded], [...manualSites]]);
    if (!summary || scanning) return;
    clearTimeout(timer);
    timer = setTimeout(() => {
      if (base !== lastBase) {
        excluded.clear();
        manualSites.clear();
        lastBase = base;
      }
      const g = grouping();
      const sent = JSON.stringify(g);
      if (sent === lastSent) return;
      lastSent = sent;
      regrouping = true;
      catalogRegroup(opId, g)
        .then((s) => {
          summary = s;
          error = '';
        })
        .catch((err) => (error = String(err?.message || err)))
        .finally(() => (regrouping = false));
    }, 250);
    void selection;
    return () => clearTimeout(timer);
  });

  const stamp = () => new Date().toISOString().slice(0, 10).replace(/-/g, '');
  function defaultName(ext, kind = '') {
    const parts = summary.gcs_prefix.slice(5).split('/').filter(Boolean);
    const base = parts.slice(-2).join('_').replace(/[^\w.-]+/g, '_') || 'catalog';
    return `${base}${kind}_${stamp()}.${ext}`;
  }

  async function exportAs(kind) {
    try {
      const [ext, suffix] = kind === 'detailed-csv' ? ['csv', '_detailed'] : kind === 'detailed-json' ? ['json', '_detailed'] : ['json', ''];
      const written = await catalogExport(opId, kind, defaultName(ext, suffix), { dataset, datasetSidecar: kind === 'oasis' && sidecar && !!dataset });
      if (written) app.toast(written.length > 1 ? `Saved ${written[0]} and the dataset information next to it` : `Saved ${written[0]}`, 'success', { timeout: 5000, action: { label: 'Show', run: () => revealPath(written[0]) } });
    } catch (err) {
      app.toast(`Couldn't save the catalog: ${err?.message || err}`, 'error');
    }
  }

  async function saveCatalog() {
    try {
      const record = await catalogSave(opId, saveName.trim() || null, dataset);
      app.toast(`Saved “${record.name}” to your catalogs`, 'success', { timeout: 3500 });
      saveName = '';
      loadSaved();
    } catch (err) {
      app.toast(`Couldn't save: ${err?.message || err}`, 'error');
    }
  }

  async function loadSaved() {
    try {
      saved = await api.savedCatalogs();
      savedError = '';
    } catch (err) {
      savedError = String(err?.message || err);
    }
  }

  async function openSaved(item) {
    try {
      const record = await api.savedCatalog(item.id);
      const meta = record.catalog.metadata || {};
      const r = meta.jetstream_request;
      prefix = record.catalog.gcs_prefix;
      if (r) {
        kinds = r.kinds?.length ? r.kinds : [];
        customExt = (r.extensions || []).join(', ');
        filenameFilter = r.filename_filter || '';
        folderFilter = r.folder_filter || '';
        ignoreFolders = (r.ignore_folder_names || []).join(', ');
        maxDepth = r.max_path_depth ?? '';
        maxResults = r.max_results || '';
        format = r.catalog_format;
        groupBy = r.group_by_mode;
        delimiter = r.filename_delimiter;
        tokenCount = r.filename_token_count;
        folderIndex = r.folder_segment_index;
        regexPattern = r.regex_pattern || '';
        regexIndex = r.regex_group_index;
        manualText = (r.manual_overrides || []).map(([g, p]) => `${g}: ${p.join(', ')}`).join('\n');
        siteMode = r.site_instance_mode;
        siteThreshold = r.site_instance_threshold;
        sitePattern = r.site_instance_pattern || '';
        siteFolderIndex = r.site_instance_folder_segment_index;
      } else {
        // Made by the classic page: its extensions and modes (other options weren't recorded).
        kinds = [];
        customExt = (meta.extensions_used || []).join(', ');
        format = record.catalog.catalog_format || 'both';
        groupBy = meta.group_by_mode || 'filename_token';
        siteMode = meta.site_instance_mode || 'none';
      }
      dataset = meta.dataset || null;
      moreFilters = !!(filenameFilter || folderFilter || ignoreFolders || maxDepth !== '' || maxResults);
      await scan();
      if (r?.selected_groups?.length && summary) {
        for (const g of summary.groups) if (!r.selected_groups.includes(g.key)) excluded.add(g.key);
      }
      for (const g of r?.manual_site_groups || []) manualSites.add(g);
      window.scrollTo?.({ top: 0, behavior: 'smooth' });
    } catch (err) {
      app.toast(`Couldn't open it: ${err?.message || err}`, 'error');
    }
  }

  async function deleteSaved(item) {
    if (!confirm(`Delete the saved catalog “${item.name}”? The files in the bucket aren't touched.`)) return;
    try {
      await api.deleteSavedCatalog(item.id);
      loadSaved();
    } catch (err) {
      app.toast(`Couldn't delete it: ${err?.message || err}`, 'error');
    }
  }

  function toggleInclude(key) {
    excluded.has(key) ? excluded.delete(key) : excluded.add(key);
  }
  function toggleSite(key) {
    manualSites.has(key) ? manualSites.delete(key) : manualSites.add(key);
  }
</script>

<section class="view">
  <header class="view-head">
    <div>
      <h1>Catalogs</h1>
      <p class="sub">List the images (or any files) in a bucket folder, group them by site, and export a catalog: the OASIS JSON the classic catalog tool makes, or every file's details.</p>
    </div>
  </header>

  {#if !isDesktop}
    <div class="card notice">Catalogs are built in the JetStream desktop app. The classic catalog tool is under More tools.</div>
  {:else}
    <div class="card setup">
      <form class="locationbar" onsubmit={(e) => { e.preventDefault(); scanning ? stop() : scan(); }}>
        <input class="input mono" bind:value={prefix} placeholder="gs://bucket/folder/" aria-label="Bucket or folder to catalog" disabled={scanning} />
        {#if scanning}
          <button class="btn" type="submit"><Square size={14} /> Stop</button>
        {:else}
          <button class="btn primary" type="submit" disabled={!prefix.trim()}><LibraryBig size={16} /> Build catalog</button>
        {/if}
      </form>
      {#if shortcuts.length}
        <div class="chips">
          {#each shortcuts as b (b)}
            <button class="chip mono" onclick={() => (prefix = `gs://${b}/`)}>{b}</button>
          {/each}
        </div>
      {/if}

      <div class="group">
        <span class="label">Files</span>
        <div class="kinds" role="group" aria-label="Kinds of files">
          {#each kindList as kind (kind.id)}
            <button
              class="kind"
              class:on={kinds.includes(kind.id)}
              aria-pressed={kinds.includes(kind.id)}
              title={kind.extensions.length ? kind.extensions.join(' ') : 'Every file, whatever its type'}
              onclick={() => toggleKind(kind.id)}>{kind.label}</button
            >
          {/each}
        </div>
        <div class="inline">
          <label for="cat-ext">Also these extensions</label>
          <input id="cat-ext" class="input mono" placeholder=".nc, .csv" bind:value={customExt} />
        </div>
      </div>

      <button class="disclosure" onclick={() => (moreFilters = !moreFilters)} aria-expanded={moreFilters}>
        {#if moreFilters}<ChevronDown size={15} />{:else}<ChevronRight size={15} />{/if} More filters
      </button>
      {#if moreFilters}
        <div class="grid">
          <label class="field"><span class="label">File name</span><input class="input mono" placeholder="site_A or *_00?_*.jpg" bind:value={filenameFilter} /></label>
          <label class="field"><span class="label">Path contains</span><input class="input mono" placeholder="2024" bind:value={folderFilter} /></label>
          <label class="field"><span class="label">Skip folders named</span><input class="input mono" placeholder="thumbs, rejects" bind:value={ignoreFolders} /></label>
          <label class="field"><span class="label">Folder depth</span><input class="input" type="number" min="0" placeholder="any" bind:value={maxDepth} /></label>
          <label class="field"><span class="label">At most</span><input class="input" type="number" min="0" placeholder="1,000,000 files" bind:value={maxResults} /></label>
        </div>
      {/if}

      <div class="group">
        <span class="label">Groups</span>
        <div class="grid">
          <label class="field">
            <span class="label">Group by</span>
            <select class="input" bind:value={groupBy}>
              <option value="filename_token">Start of the file name</option>
              <option value="folder_segment">Folder</option>
              <option value="regex">Pattern (regular expression)</option>
              <option value="manual">My own groups</option>
              <option value="none">No groups</option>
            </select>
          </label>
          {#if groupBy === 'filename_token'}
            <label class="field"><span class="label">Split at</span><input class="input mono" bind:value={delimiter} /></label>
            <label class="field"><span class="label">Parts to keep</span><input class="input" type="number" min="1" max="10" bind:value={tokenCount} /></label>
          {:else if groupBy === 'folder_segment'}
            <label class="field"><span class="label">Folder level</span><input class="input" type="number" min="0" bind:value={folderIndex} /><small class="hint">0 is the first folder in the bucket</small></label>
          {:else if groupBy === 'regex'}
            <label class="field wide"><span class="label">Pattern</span><input class="input mono" placeholder={'site_([A-Z]+)_\\d+'} bind:value={regexPattern} /></label>
            <label class="field"><span class="label">Capture group</span><input class="input" type="number" min="0" bind:value={regexIndex} /></label>
          {:else if groupBy === 'manual'}
            <label class="field wide">
              <span class="label">One group per line</span>
              <textarea class="input" rows="3" placeholder={'Kona: kona, *_KON_*\nHilo: hilo'} bind:value={manualText}></textarea>
              <small class="hint">Group name, a colon, then name parts or patterns (* and ?). The first matching line wins.</small>
            </label>
          {/if}
          <label class="field">
            <span class="label">Site instances</span>
            <select class="input" bind:value={siteMode}>
              <option value="none">None</option>
              <option value="threshold">Groups with at least…</option>
              <option value="folder_boundary">Groups that are a folder</option>
              <option value="filename_pattern">Groups with a file matching…</option>
              <option value="manual">Groups I tick</option>
            </select>
          </label>
          {#if siteMode === 'threshold'}
            <label class="field"><span class="label">Files per group</span><input class="input" type="number" min="1" bind:value={siteThreshold} /></label>
          {:else if siteMode === 'folder_boundary'}
            <label class="field"><span class="label">Folder level</span><input class="input" type="number" min="0" bind:value={siteFolderIndex} /></label>
          {:else if siteMode === 'filename_pattern'}
            <label class="field"><span class="label">File name pattern</span><input class="input mono" bind:value={sitePattern} /></label>
          {/if}
          <label class="field">
            <span class="label">OASIS JSON holds</span>
            <select class="input" bind:value={format}>
              <option value="both">The list and the groups</option>
              <option value="flat">The list only</option>
              <option value="grouped">The groups only</option>
            </select>
          </label>
        </div>
      </div>
    </div>

    {#if scanning}
      <div class="card status" aria-live="polite">
        Listing <span class="mono">{prefix}</span>… {count(tick?.done || 0)} files looked at, {count(Math.max(0, (tick?.done || 0) - (tick?.skipped || 0)))} in the catalog ({bytes(tick?.bytes || 0)})
      </div>
    {/if}
    {#if error}<div class="error">{error}</div>{/if}

    {#if summary}
      <div class="card result">
        <div class="totals">
          <div><span class="big">{count(summary.total_items)}</span> files</div>
          <div><span class="big">{count(summary.total_groups)}</span> {summary.total_groups === 1 ? 'group' : 'groups'}</div>
          {#if summary.site_groups}<div><span class="big">{count(summary.site_groups)}</span> site instances</div>{/if}
          <div><span class="big">{bytes(includedBytes)}</span></div>
          <div class="muted small">
            of {count(summary.listed)} looked at · {summary.extensions_used.length ? summary.extensions_used.join(' ') : 'all files'}
            {#if regrouping} · regrouping…{/if}
          </div>
        </div>
        {#if summary.truncated}<p class="warn">Stopped at {count(summary.matched)} files (the limit). Narrow the folder or raise “At most” under More filters.</p>{/if}
        {#each summary.warnings as warning (warning)}<p class="warn">{warning}</p>{/each}

        <div class="table-head">
          <span class="label">Groups</span>
          <div class="sort">
            Sort by
            <button class:on={sortBy === 'key'} onclick={() => (sortBy = 'key')}>name</button>
            <button class:on={sortBy === 'count'} onclick={() => (sortBy = 'count')}>files</button>
          </div>
          <button class="btn ghost sm" onclick={() => (gallery = null)}><Images size={14} /> Browse all files</button>
        </div>
        <div class="groups" role="table">
          <div class="tr th" role="row">
            {#if canSelect}<span role="columnheader" title="Untick to leave a group out of the catalog">In</span>{/if}
            <span role="columnheader">Group</span>
            <span role="columnheader" class="num">Files</span>
            <span role="columnheader" class="num">Size</span>
            <span role="columnheader">Site</span>
            <span role="columnheader">Samples</span>
          </div>
          {#each rows.slice(0, rowsShown) as row (row.key)}
            <div class="tr" class:out={!row.included} role="row">
              {#if canSelect}<span role="cell"><input type="checkbox" aria-label="Include {row.key}" checked={!excluded.has(row.key)} onchange={() => toggleInclude(row.key)} /></span>{/if}
              <span role="cell" class="key"><button class="link mono" onclick={() => (gallery = row.key)}>{row.key}</button></span>
              <span role="cell" class="num">{count(row.count)}</span>
              <span role="cell" class="num muted">{bytes(row.bytes)}</span>
              <span role="cell">
                {#if siteMode === 'manual'}
                  <input type="checkbox" aria-label="{row.key} is a site instance" checked={manualSites.has(row.key)} onchange={() => toggleSite(row.key)} />
                {:else if row.site}<span class="pill accent">site</span>{/if}
              </span>
              <span role="cell" class="samples">
                {#each row.samples as entry (entry.path)}
                  <button class="sample" title={entry.name} onclick={() => (gallery = row.key)}><ThumbTile {bucket} {entry} /></button>
                {/each}
              </span>
            </div>
          {/each}
        </div>
        {#if rows.length > rowsShown}
          <button class="btn ghost sm" onclick={() => (rowsShown += 200)}>Show more groups ({count(rows.length - rowsShown)} more)</button>
        {/if}
        {#if summary.all_groups > summary.groups.length}
          <p class="muted small">Showing the first {count(summary.groups.length)} of {count(summary.all_groups)} groups; the catalog includes them all.</p>
        {/if}
      </div>

      {#if gallery !== undefined}
        <CatalogGallery {opId} {bucket} group={gallery} onclose={() => (gallery = undefined)} />
      {/if}

      <DatasetInfoCard bind:dataset />

      <div class="card panel export">
        <h2>Export</h2>
        <div class="actions">
          <button class="btn primary" disabled={regrouping || !summary.total_items} onclick={() => exportAs('oasis')}><FileJson size={15} /> Save OASIS JSON</button>
          <button class="btn" disabled={regrouping || !summary.total_items} onclick={() => exportAs('detailed-csv')}><Sheet size={15} /> Detailed CSV</button>
          <button class="btn" disabled={regrouping || !summary.total_items} onclick={() => exportAs('detailed-json')}><FileJson size={15} /> Detailed JSON</button>
        </div>
        <p class="muted small">
          <strong>OASIS JSON</strong>: the catalog's <span class="mono">gs://</span> paths, exactly as the classic catalog tool writes them.
          <strong>Detailed</strong>: one row per file with its group, size, type, original dates, checksums and storage class{dataset ? ', plus the dataset information' : ''}.
        </p>
        {#if dataset}
          <label class="check"><input type="checkbox" bind:checked={sidecar} /> Also save the dataset information next to the OASIS JSON (<span class="mono">….dataset.json</span>)</label>
        {/if}
        <form class="row" onsubmit={(e) => { e.preventDefault(); saveCatalog(); }}>
          <input class="input" placeholder="Name (optional)" bind:value={saveName} aria-label="Catalog name" />
          <button class="btn" type="submit" disabled={regrouping || !summary.total_items}><Save size={15} /> Save to my catalogs</button>
        </form>
      </div>
    {/if}

    <div class="card panel">
      <h2>My catalogs</h2>
      {#if savedError}
        <p class="muted small">{savedError}</p>
      {:else if !saved.length}
        <p class="muted small">Saved catalogs show here, including ones from the classic catalog tool.</p>
      {:else}
        <div class="saved">
          {#each saved as item (item.id)}
            <div class="saved-row">
              <button class="link" onclick={() => openSaved(item)} title="Build it again from the bucket with the same options">{item.name}</button>
              <span class="mono muted truncate">{item.gcs_prefix}</span>
              <span class="muted">{count(item.total_items)} files · {count(item.total_groups)} groups</span>
              <span class="muted">{item.created_at ? relativeTime(item.created_at) : ''}</span>
              <button class="btn ghost sm icon" aria-label="Delete {item.name}" onclick={() => deleteSaved(item)}><Trash2 size={14} /></button>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</section>

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
  h2 {
    font-size: 15px;
    margin: 0;
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
  .setup,
  .panel,
  .result {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    margin-bottom: 16px;
  }
  .locationbar,
  .row {
    display: flex;
    gap: 8px;
  }
  .chips,
  .kinds {
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
  .kind {
    height: 30px;
    padding: 0 12px;
    border-radius: 999px;
    border: 1px solid var(--border-strong);
    background: var(--surface);
    color: var(--text-2);
    font: inherit;
    font-size: 13px;
    cursor: pointer;
  }
  .kind.on {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--accent);
    font-weight: 600;
  }
  .group {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .inline {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--text-2);
  }
  .inline input {
    max-width: 260px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(190px, 1fr));
    gap: 10px 14px;
    align-items: start;
  }
  .field.wide {
    grid-column: span 2;
  }
  .field .label {
    font-weight: 500;
    font-size: 12.5px;
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
  .status {
    padding: 12px 16px;
    margin-bottom: 16px;
    font-size: 13.5px;
  }
  .totals {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 6px 22px;
  }
  .big {
    font-size: 22px;
    font-weight: 650;
    font-variant-numeric: tabular-nums;
  }
  .warn {
    margin: 0;
    padding: 8px 12px;
    border-radius: var(--radius-sm);
    background: var(--warning-soft, var(--surface-2));
    font-size: 13px;
  }
  .table-head {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .sort {
    font-size: 12.5px;
    color: var(--muted);
    display: flex;
    gap: 4px;
    align-items: center;
    flex: 1;
  }
  .sort button {
    border: none;
    background: none;
    color: var(--text-2);
    cursor: pointer;
    font: inherit;
    padding: 2px 6px;
    border-radius: 6px;
  }
  .sort button.on {
    background: var(--surface-2);
    color: var(--text);
    font-weight: 600;
  }
  .groups {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }
  .tr {
    display: grid;
    grid-template-columns: 32px minmax(160px, 1.4fr) 80px 90px 60px minmax(200px, 2fr);
    align-items: center;
    gap: 10px;
    padding: 6px 12px;
    border-top: 1px solid var(--border);
    font-size: 13px;
  }
  .groups:not(:has(input[aria-label^='Include'])) .tr {
    grid-template-columns: minmax(160px, 1.4fr) 80px 90px 60px minmax(200px, 2fr);
  }
  .tr.th {
    border-top: none;
    background: var(--surface-2);
    font-size: 11.5px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--muted);
  }
  .tr.out {
    opacity: 0.5;
  }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .key {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .samples {
    display: flex;
    gap: 6px;
  }
  .sample {
    width: 52px;
    padding: 0;
    border: none;
    background: none;
    cursor: pointer;
  }
  .link {
    border: none;
    background: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font: inherit;
    text-align: left;
  }
  .actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
  }
  .saved {
    display: flex;
    flex-direction: column;
  }
  .saved-row {
    display: grid;
    grid-template-columns: minmax(140px, 1fr) minmax(160px, 1.5fr) auto auto 32px;
    gap: 12px;
    align-items: center;
    padding: 6px 0;
    border-top: 1px solid var(--border);
    font-size: 13px;
  }
  .saved-row:first-child {
    border-top: none;
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
    margin-bottom: 16px;
  }
</style>
