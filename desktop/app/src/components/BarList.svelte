<script>
  // Horizontal bars, one hue (magnitude only), value at the bar end, details on hover.
  let { items, value = (i) => i.bytes, label = (i) => i.name, format = String, detail = () => '', onselect = null } = $props();

  let max = $derived(Math.max(1, ...items.map(value)));
</script>

<ul class="bars">
  {#each items as item, index (label(item) + index)}
    {@const v = value(item)}
    <li>
      <svelte:element
        this={onselect ? 'button' : 'div'}
        class="row"
        class:clickable={!!onselect}
        title={`${label(item)} — ${format(v)}${detail(item) ? ` · ${detail(item)}` : ''}`}
        onclick={onselect ? () => onselect(item) : undefined}
        role={onselect ? undefined : 'listitem'}
      >
        <span class="name truncate">{label(item)}</span>
        <span class="track">
          <span class="bar" style:width={v > 0 ? `${Math.max(1.5, (v / max) * 100)}%` : '0'}></span>
        </span>
        <span class="value">{format(v)}</span>
      </svelte:element>
    </li>
  {/each}
</ul>

<style>
  .bars {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .row {
    display: grid;
    grid-template-columns: minmax(90px, 34%) 1fr 76px;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 5px 6px;
    border: none;
    border-radius: 6px;
    background: transparent;
    text-align: left;
    color: var(--text);
  }
  .row.clickable {
    cursor: pointer;
  }
  .row:hover {
    background: var(--surface-2);
  }
  .name {
    font-size: 13px;
  }
  .track {
    height: 10px;
  }
  .bar {
    display: block;
    height: 100%;
    background: var(--chart-1);
    border-radius: 0 4px 4px 0;
  }
  .value {
    text-align: right;
    font-size: 12.5px;
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }
</style>
