<script>
  // Daily jobs, stacked: completed (blue) under failed (red). One axis, hover tooltip,
  // legend with labels, and a table view for exact numbers.
  let { days } = $props();

  let width = $state(600);
  const height = 200;
  const pad = { top: 10, right: 8, bottom: 24, left: 34 };

  let max = $derived(Math.max(1, ...days.map((d) => d.completed + d.failed)));
  let niceMax = $derived(max <= 5 ? max : Math.ceil(max / 5) * 5);
  let ticks = $derived(niceMax <= 5 ? [...Array(niceMax + 1).keys()] : [0, niceMax / 5, (2 * niceMax) / 5, (3 * niceMax) / 5, (4 * niceMax) / 5, niceMax]);
  let plotW = $derived(Math.max(10, width - pad.left - pad.right));
  let plotH = height - pad.top - pad.bottom;
  let slot = $derived(plotW / Math.max(1, days.length));
  let barW = $derived(Math.min(24, Math.max(3, slot - 4)));
  let hover = $state(null);
  // Label every Nth day, counted back from today so the latest day is always labeled.
  let labelEvery = $derived(Math.max(1, Math.ceil(days.length / Math.max(2, Math.floor(plotW / 70)))));

  const y = (v) => pad.top + plotH - (v / niceMax) * plotH;

  /** Column path with 4px rounded top (square at the baseline). */
  function column(x, y0, y1, w, roundTop) {
    const h = y0 - y1;
    if (h <= 0) return '';
    const r = roundTop ? Math.min(4, w / 2, h) : 0;
    return `M${x},${y0} V${y1 + r} Q${x},${y1} ${x + r},${y1} H${x + w - r} Q${x + w},${y1} ${x + w},${y1 + r} V${y0} Z`;
  }

  function label(date) {
    return new Date(`${date}T00:00:00`).toLocaleDateString([], { month: 'short', day: 'numeric' });
  }
</script>

<div class="chart" bind:clientWidth={width}>
  <svg {width} {height} role="img" aria-label="Jobs per day, completed and failed">
    {#each ticks as t (t)}
      <line x1={pad.left} x2={width - pad.right} y1={y(t)} y2={y(t)} class="grid" />
      <text x={pad.left - 8} y={y(t)} class="tick" text-anchor="end" dominant-baseline="middle">{t}</text>
    {/each}
    {#each days as d, i (d.date)}
      {@const x = pad.left + i * slot + (slot - barW) / 2}
      {@const base = y(0)}
      {@const top1 = y(d.completed)}
      {@const top2 = y(d.completed + d.failed)}
      <path d={column(x, base, top1, barW, d.failed === 0)} class="completed" />
      {#if d.failed}
        <!-- 2px surface gap between stacked segments -->
        <path d={column(x, top1 - (d.completed ? 2 : 0), top2 - (d.completed ? 2 : 0), barW, true)} class="failed" />
      {/if}
      <rect
        x={pad.left + i * slot}
        y={pad.top}
        width={slot}
        height={plotH}
        class="hit"
        role="presentation"
        onmouseenter={() => (hover = { d, x: pad.left + i * slot + slot / 2 })}
        onmouseleave={() => (hover = null)}
      />
      {#if (days.length - 1 - i) % labelEvery === 0}
        <text x={x + barW / 2} y={height - 6} class="tick" text-anchor="middle">{label(d.date)}</text>
      {/if}
    {/each}
  </svg>
  {#if hover}
    <div class="tooltip" style:left={`${Math.min(Math.max(hover.x, 70), width - 70)}px`}>
      <strong>{label(hover.d.date)}</strong>
      <span><i class="swatch completed"></i>Completed <b>{hover.d.completed}</b></span>
      <span><i class="swatch failed"></i>Failed <b>{hover.d.failed}</b></span>
      {#if hover.d.gb}<span class="muted">{hover.d.gb} GB uploaded</span>{/if}
    </div>
  {/if}
  <div class="legend">
    <span><i class="swatch completed"></i>Completed</span>
    <span><i class="swatch failed"></i>Failed</span>
  </div>
</div>

<style>
  .chart {
    position: relative;
  }
  svg {
    display: block;
    overflow: visible;
  }
  .grid {
    stroke: var(--chart-grid);
    stroke-width: 1;
  }
  .tick {
    fill: var(--muted);
    font-size: 11px;
  }
  .completed {
    fill: var(--chart-1);
  }
  .failed {
    fill: var(--chart-fail);
  }
  .hit {
    fill: transparent;
  }
  .hit:hover {
    fill: color-mix(in srgb, var(--text) 5%, transparent);
  }
  .tooltip {
    position: absolute;
    top: 0;
    transform: translateX(-50%);
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--surface);
    border: 1px solid var(--border);
    box-shadow: var(--shadow-lg);
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: 12.5px;
    pointer-events: none;
    white-space: nowrap;
  }
  .tooltip b {
    font-variant-numeric: tabular-nums;
    margin-left: 4px;
  }
  .legend {
    display: flex;
    gap: 16px;
    margin-top: 6px;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .swatch {
    display: inline-block;
    width: 10px;
    height: 10px;
    border-radius: 3px;
    margin-right: 6px;
    vertical-align: -1px;
  }
  .swatch.completed {
    background: var(--chart-1);
  }
  .swatch.failed {
    background: var(--chart-fail);
  }
  .muted {
    color: var(--muted);
  }
</style>
