<script>
  // Optional dataset information for a catalog: from NOAA InPort (an item number or page) or
  // any ISO 19115 record (a URL or an XML file). Goes into detailed exports and saved
  // catalogs; the OASIS JSON is never changed.
  import FileUp from '@lucide/svelte/icons/file-up';
  import Search from '@lucide/svelte/icons/search';
  import X from '@lucide/svelte/icons/x';
  import ExternalLink from '@lucide/svelte/icons/external-link';
  import { catalogDatasetInfo, openUrl, pickXmlFile } from '../lib/platform.js';

  let { dataset = $bindable(null) } = $props();
  let source = $state('');
  let busy = $state(false);
  let error = $state('');

  async function lookUp(from = source) {
    if (!from?.trim()) return;
    busy = true;
    error = '';
    try {
      dataset = await catalogDatasetInfo(from.trim());
    } catch (err) {
      error = String(err?.message || err);
    } finally {
      busy = false;
    }
  }

  async function chooseFile() {
    const file = await pickXmlFile();
    if (file) lookUp(file);
  }

  const place = (b) => `${b.south.toFixed(2)}° to ${b.north.toFixed(2)}° N, ${b.west.toFixed(2)}° to ${b.east.toFixed(2)}° E`;
</script>

<div class="card panel">
  <h2>Dataset information <span class="optional">optional</span></h2>
  {#if dataset}
    <div class="info">
      <div class="top">
        <strong>{dataset.title || '(untitled record)'}</strong>
        <button class="btn ghost sm icon" aria-label="Remove dataset information" title="Remove" onclick={() => (dataset = null)}><X size={14} /></button>
      </div>
      <dl>
        {#if dataset.inport_id}<dt>InPort</dt><dd><button class="link" onclick={() => openUrl(dataset.inport_url)}>Item {dataset.inport_id} <ExternalLink size={12} /></button></dd>{/if}
        {#if dataset.doi}<dt>DOI</dt><dd><button class="link" onclick={() => openUrl(dataset.doi)}>{dataset.doi.replace('https://doi.org/', '')}</button></dd>{/if}
        {#if dataset.time_begin || dataset.time_end}<dt>Time</dt><dd>{dataset.time_begin || '?'} to {dataset.time_end || '?'}</dd>{/if}
        {#if dataset.bounding_box}<dt>Area</dt><dd>{place(dataset.bounding_box)}</dd>{/if}
        {#if dataset.contacts?.length}<dt>Contacts</dt><dd>{dataset.contacts.slice(0, 3).map((c) => c.name || c.organization).join('; ')}{dataset.contacts.length > 3 ? `; +${dataset.contacts.length - 3}` : ''}</dd>{/if}
        {#if dataset.keywords?.length}<dt>Keywords</dt><dd>{dataset.keywords.slice(0, 8).join(', ')}{dataset.keywords.length > 8 ? ', …' : ''}</dd>{/if}
      </dl>
      {#if dataset.abstract}<p class="abstract">{dataset.abstract}</p>{/if}
      <p class="muted small">Included in detailed exports and saved catalogs, and optionally saved next to the OASIS JSON. The OASIS JSON itself doesn't change.</p>
    </div>
  {:else}
    <p class="sub">Attach the dataset's record so the catalog carries its title, extent, contacts and citation: a NOAA InPort catalog item number, an ISO 19115 record address, or an ISO XML file.</p>
    <form class="row" onsubmit={(e) => { e.preventDefault(); lookUp(); }}>
      <input class="input mono" placeholder="InPort item (e.g. 27377) or ISO record URL" bind:value={source} disabled={busy} aria-label="InPort item or ISO record URL" />
      <button class="btn" type="submit" disabled={busy || !source.trim()}><Search size={15} /> {busy ? 'Looking up…' : 'Look up'}</button>
      <button class="btn ghost" type="button" disabled={busy} onclick={chooseFile}><FileUp size={15} /> XML file…</button>
    </form>
    {#if error}<div class="error">{error}</div>{/if}
  {/if}
</div>

<style>
  .panel {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  h2 {
    font-size: 15px;
    margin: 0;
  }
  .optional {
    font-weight: 400;
    font-size: 12px;
    color: var(--muted);
    margin-left: 6px;
  }
  .sub {
    margin: 0;
    color: var(--text-2);
    font-size: 13px;
  }
  .row {
    display: flex;
    gap: 8px;
  }
  .top {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    align-items: flex-start;
  }
  dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 4px 12px;
    margin: 8px 0 0;
    font-size: 13px;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
  }
  .abstract {
    font-size: 12.5px;
    color: var(--text-2);
    margin: 8px 0 0;
    max-height: 5.4em;
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 4;
    line-clamp: 4;
    -webkit-box-orient: vertical;
  }
  .link {
    border: none;
    background: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font: inherit;
    display: inline-flex;
    align-items: center;
    gap: 3px;
  }
  .small {
    font-size: 12px;
    margin: 6px 0 0;
  }
  .muted {
    color: var(--muted);
  }
  .error {
    color: var(--danger);
    padding: 8px 12px;
    border-radius: var(--radius-sm);
    background: var(--danger-soft);
    font-size: 13px;
  }
</style>
