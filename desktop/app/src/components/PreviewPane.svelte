<script>
  // Cloud storage preview pane: a larger picture, the start of a text or CSV file, and every
  // detail Cloud Storage keeps about the selected file. Loads only for the selected file, after
  // a short pause (so arrowing through a folder doesn't download each file on the way).
  import X from '@lucide/svelte/icons/x';
  import Copy from '@lucide/svelte/icons/copy';
  import Download from '@lucide/svelte/icons/download';
  import ExternalLink from '@lucide/svelte/icons/external-link';
  import Sigma from '@lucide/svelte/icons/sigma';
  import Folder from '@lucide/svelte/icons/folder';
  import FileIcon from '@lucide/svelte/icons/file';
  import Snowflake from '@lucide/svelte/icons/snowflake';
  import { objectDetails, previewText, thumbnail } from '../lib/platform.js';
  import { objectFacts, parseCsv, prettyText, previewKind, sniffDelimiter } from '../lib/preview.js';
  import { previewCapMib, thumbLoader } from '../lib/thumbs.svelte.js';
  import { bytes, count, dateTime } from '../lib/format.js';

  let { bucket, entries, width = 340, onclose, oncopy, ondownload, onconsole, onsize, onresize } = $props();

  const PAUSE_MS = 250;
  let one = $derived(entries.length === 1 ? entries[0] : null);
  let kind = $derived(previewKind(one));
  let facts = $state(null);
  let factsError = $state('');
  let image = $state(null);
  let text = $state(null);
  let loadingContent = $state(false);
  let generation = 0;

  const gsPath = (path) => `gs://${bucket}/${path}`;
  const className = (c) => (c ? c.charAt(0) + c.slice(1).toLowerCase() : '');
  // The tile's small thumbnail, if it's already loaded: shown while the big one comes.
  let placeholder = $derived(one && kind === 'image' ? thumbLoader.results[thumbLoader.key(bucket, one)]?.data_url : null);
  let table = $derived.by(() => {
    if (kind !== 'table' || text?.kind !== 'text') return null;
    return parseCsv(text.text, { delimiter: sniffDelimiter(text.text, one.name), truncated: text.truncated });
  });
  let shownText = $derived(text?.kind === 'text' ? prettyText(text.text, one, text.truncated) : '');
  let totalSize = $derived(entries.reduce((n, e) => n + (e.is_folder ? 0 : e.size || 0), 0));
  let folders = $derived(entries.filter((e) => e.is_folder).length);

  $effect(() => {
    const entry = one;
    const mine = ++generation;
    facts = null;
    factsError = '';
    image = null;
    text = null;
    loadingContent = false;
    if (!entry || entry.is_folder) return;
    const timer = setTimeout(() => {
      if (mine !== generation) return;
      objectDetails(bucket, entry.path)
        .then((raw) => mine === generation && (facts = objectFacts(raw) || { gone: true }))
        .catch((err) => mine === generation && (factsError = String(err?.message || err)));
      loadContent(entry, mine, {});
    }, PAUSE_MS);
    return () => clearTimeout(timer);
  });

  async function loadContent(entry, mine, { allowCold = false, maxMib = previewCapMib() }) {
    const k = previewKind(entry);
    if (!k) return;
    loadingContent = true;
    try {
      if (k === 'image') {
        const result = await thumbnail({
          bucket,
          path: entry.path,
          generation: entry.generation || null,
          size: entry.size || 0,
          contentType: entry.content_type || null,
          storageClass: entry.storage_class || null,
          allowCold,
          maxMib,
          edge: 1024,
        }).catch((err) => ({ skip: 'error', message: String(err?.message || err) }));
        if (mine === generation) image = result;
      } else {
        const result = await previewText(bucket, entry, allowCold).catch((err) => ({ kind: 'error', message: String(err?.message || err) }));
        if (mine === generation) text = result;
      }
    } finally {
      if (mine === generation) loadingContent = false;
    }
  }

  function again(options) {
    loadContent(one, generation, options);
  }

  // Drag the left edge to resize.
  function startResize(event) {
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = width;
    const move = (e) => onresize?.(Math.min(760, Math.max(260, startWidth + (startX - e.clientX))));
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }

  const when = (ms) => (ms ? dateTime(new Date(ms).toISOString()) : '');
</script>

<aside class="pane" style:width="{width}px" aria-label="Preview">
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="grip" onpointerdown={startResize} title="Drag to resize"></div>
  <header>
    <span class="title truncate" title={one ? one.name : ''}>{one ? one.name.replace(/\/$/, '') : `${count(entries.length)} selected`}</span>
    <button class="btn ghost sm icon" aria-label="Close preview" title="Close preview (turn it back on with Preview in the toolbar)" onclick={onclose}><X size={15} /></button>
  </header>

  <div class="body">
    {#if !one}
      <p class="lead">{count(entries.length - folders)} {entries.length - folders === 1 ? 'file' : 'files'}{folders ? ` and ${count(folders)} ${folders === 1 ? 'folder' : 'folders'}` : ''}</p>
      {#if entries.length - folders}<p class="muted">{bytes(totalSize)} in the files{folders ? ' (folders not counted)' : ''}</p>{/if}
      <div class="actions">
        <button class="btn sm" onclick={oncopy}><Copy size={14} /> Copy paths</button>
        <button class="btn sm" onclick={ondownload}><Download size={14} /> Download</button>
      </div>
    {:else if one.is_folder}
      <div class="big-icon"><Folder size={44} color="var(--accent)" strokeWidth={1.5} /></div>
      <p class="mono path">{gsPath(one.path)}</p>
      <div class="actions">
        <button class="btn sm" onclick={() => onsize?.(one)}><Sigma size={14} /> Size</button>
        <button class="btn sm" onclick={oncopy}><Copy size={14} /> Copy path</button>
        <button class="btn sm" onclick={onconsole}><ExternalLink size={14} /> Console</button>
      </div>
    {:else}
      {#if kind === 'image'}
        <div class="picture">
          {#if image?.data_url}
            <img src={image.data_url} alt={one.name} />
          {:else if image?.skip === 'cold'}
            <button class="ask" onclick={() => again({ allowCold: true })}><Snowflake size={18} /> {className(image.storage_class)} storage: retrieving is charged. <strong>Load preview</strong></button>
          {:else if image?.skip === 'too_large'}
            <button class="ask" onclick={() => again({ maxMib: Math.ceil(image.size / 1048576) + 1 })}>{bytes(image.size)} is over the preview limit. <strong>Download to preview</strong></button>
          {:else if image?.skip === 'error'}
            <button class="ask" onclick={() => again({})}>Couldn't load the preview. <strong>Try again</strong><small>{image.message}</small></button>
          {:else if image?.skip}
            <p class="muted">No preview: {image.message || "this image can't be read"}</p>
          {:else if placeholder}
            <img class="blurry" src={placeholder} alt="" />
          {:else}
            <div class="shimmer" aria-label="Loading preview"></div>
          {/if}
        </div>
      {:else if kind}
        <div class="textbox">
          {#if text?.kind === 'cold'}
            <button class="ask" onclick={() => again({ allowCold: true })}><Snowflake size={18} /> {className(text.storage_class)} storage: reading is charged. <strong>Load preview</strong></button>
          {:else if text?.kind === 'binary'}
            <p class="muted pad">This file isn't text.</p>
          {:else if text?.kind === 'error'}
            <button class="ask" onclick={() => again({})}>Couldn't load the preview. <strong>Try again</strong><small>{text.message}</small></button>
          {:else if table}
            <div class="table-wrap">
              <table>
                {#if table.rows.length}
                  <thead><tr>{#each table.rows[0] as cell, c (c)}<th>{cell}</th>{/each}</tr></thead>
                {/if}
                <tbody>
                  {#each table.rows.slice(1) as row, r (r)}
                    <tr>{#each row as cell, c (c)}<td>{cell}</td>{/each}</tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {:else if text?.kind === 'text'}
            <pre>{shownText || '(empty file)'}</pre>
          {:else}
            <div class="shimmer short" aria-label="Loading preview"></div>
          {/if}
        </div>
        {#if text?.kind === 'text' && (text.truncated || table?.more)}
          <p class="muted small">
            {#if table}First {count(Math.max(0, table.rows.length - 1))} rows{text.truncated ? ` (the first ${bytes(text.shown)} of ${bytes(text.size)})` : ''}{:else}The first {bytes(text.shown)} of {bytes(text.size)}{/if}
          </p>
        {/if}
      {/if}

      <dl class="facts">
        {#if factsError}
          <dt>Details</dt><dd class="bad">{factsError}</dd>
        {:else if facts?.gone}
          <dt>Details</dt><dd class="bad">This file no longer exists. Refresh the folder.</dd>
        {:else}
          <dt>Size</dt><dd>{bytes(one.size || 0)}{one.size >= 1024 ? ` (${count(one.size)} bytes)` : ''}</dd>
          <dt>Type</dt><dd class="mono">{facts?.type || one.content_type || '…'}</dd>
          <dt>Storage class</dt><dd>{className(facts?.storageClass || one.storage_class) || '…'}</dd>
          {#if facts}
            <dt>Created</dt><dd>{facts.created ? when(facts.created) : 'not recorded'}</dd>
            <dt>Modified</dt><dd>{facts.modified ? when(facts.modified) : 'not recorded'}</dd>
            <dt>Uploaded</dt><dd>{facts.uploaded ? dateTime(facts.uploaded) : ''}</dd>
            {#if facts.updated}<dt>Details changed</dt><dd>{dateTime(facts.updated)}</dd>{/if}
            {#if !facts.modified}<dd class="note">The original file's dates weren't recorded when this was uploaded. <strong>Fix file details</strong> on its synced folder can add them.</dd>{/if}
            <dt>CRC32C</dt><dd class="mono">{facts.crc32c || '–'}</dd>
            <dt>MD5</dt><dd class="mono">{facts.md5 || (facts.composite ? 'none (uploaded in parallel parts; checked by CRC32C)' : '–')}</dd>
            <dt>Generation</dt><dd class="mono">{facts.generation}</dd>
            {#if facts.encoding}<dt>Encoding</dt><dd class="mono">{facts.encoding}</dd>{/if}
            {#if facts.cacheControl}<dt>Cache control</dt><dd class="mono">{facts.cacheControl}</dd>{/if}
            {#each facts.custom as [key, value] (key)}
              <dt class="mono" title="Custom metadata">{key}</dt><dd class="mono">{value}</dd>
            {/each}
          {/if}
        {/if}
      </dl>
      <p class="mono path">{gsPath(one.path)}</p>
      <div class="actions">
        <button class="btn sm" onclick={oncopy}><Copy size={14} /> Copy path</button>
        <button class="btn sm" onclick={ondownload}><Download size={14} /> Download</button>
        <button class="btn sm" onclick={onconsole}><ExternalLink size={14} /> Console</button>
      </div>
      {#if !kind}
        <p class="muted small"><FileIcon size={12} /> No preview for this kind of file; images, text and CSV files show here.</p>
      {/if}
    {/if}
  </div>
</aside>

<style>
  .pane {
    position: relative;
    flex-shrink: 0;
    border-left: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    min-height: 0;
    max-height: 72vh;
    background: var(--surface);
  }
  .grip {
    position: absolute;
    left: -3px;
    top: 0;
    bottom: 0;
    width: 6px;
    cursor: col-resize;
    z-index: 1;
  }
  .grip:hover {
    background: color-mix(in srgb, var(--accent) 35%, transparent);
  }
  header {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 10px 10px 10px 14px;
    border-bottom: 1px solid var(--border);
  }
  .title {
    flex: 1;
    font-weight: 600;
    font-size: 13.5px;
  }
  .body {
    overflow-y: auto;
    padding: 12px 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .lead {
    margin: 0;
    font-weight: 600;
  }
  .big-icon {
    display: grid;
    place-items: center;
    padding: 18px 0 4px;
  }
  .picture {
    border-radius: 8px;
    background: var(--surface-2);
    display: grid;
    place-items: center;
    min-height: 180px;
    overflow: hidden;
  }
  .picture img {
    max-width: 100%;
    max-height: 420px;
    display: block;
    object-fit: contain;
  }
  .picture img.blurry {
    width: 100%;
    filter: blur(6px);
    opacity: 0.7;
  }
  .shimmer {
    width: 100%;
    height: 220px;
    background: linear-gradient(90deg, var(--surface-2) 0%, var(--surface-hover, var(--surface)) 50%, var(--surface-2) 100%);
    background-size: 200% 100%;
    animation: shimmer 1.2s infinite;
  }
  .shimmer.short {
    height: 120px;
  }
  @keyframes shimmer {
    to {
      background-position: -200% 0;
    }
  }
  .ask {
    border: none;
    background: none;
    color: var(--text-2);
    padding: 18px 12px;
    cursor: pointer;
    font: inherit;
    font-size: 12.5px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    text-align: center;
    width: 100%;
  }
  .ask strong {
    color: var(--accent);
  }
  .ask small {
    color: var(--muted);
    word-break: break-word;
  }
  .textbox {
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface-2);
    max-height: 320px;
    overflow: auto;
  }
  pre {
    margin: 0;
    padding: 10px 12px;
    font-family: var(--mono);
    font-size: 11.5px;
    line-height: 1.45;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .pad {
    padding: 12px;
    margin: 0;
  }
  .table-wrap table {
    border-collapse: collapse;
    font-size: 11.5px;
    font-family: var(--mono);
    white-space: nowrap;
  }
  th,
  td {
    padding: 4px 8px;
    border-bottom: 1px solid var(--border);
    text-align: left;
  }
  th {
    position: sticky;
    top: 0;
    background: var(--surface);
    font-weight: 600;
  }
  .facts {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 5px 12px;
    margin: 0;
    font-size: 12.5px;
  }
  .facts dt {
    color: var(--muted);
  }
  .facts dd {
    margin: 0;
    word-break: break-word;
  }
  .facts dd.mono,
  .facts dt.mono {
    font-family: var(--mono);
    font-size: 11.5px;
  }
  .facts .note {
    grid-column: 1 / -1;
    color: var(--text-2);
    background: var(--surface-2);
    padding: 6px 8px;
    border-radius: var(--radius-sm);
  }
  .path {
    font-size: 11.5px;
    margin: 0;
    word-break: break-all;
    color: var(--text-2);
  }
  .actions {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .small {
    font-size: 12px;
    margin: 0;
  }
  .muted {
    color: var(--muted);
  }
  .bad {
    color: var(--danger);
  }
</style>
