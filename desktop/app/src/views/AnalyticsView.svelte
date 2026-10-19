<script>
  import Download from '@lucide/svelte/icons/download';
  import Table from '@lucide/svelte/icons/table';
  import ChartColumn from '@lucide/svelte/icons/chart-column';
  import JobsChart from '../components/JobsChart.svelte';
  import BarList from '../components/BarList.svelte';
  import { app } from '../lib/state.svelte.js';
  import { api, csvExportUrl } from '../lib/api.js';
  import { bytes, count, duration } from '../lib/format.js';
  import { openUrl } from '../lib/platform.js';

  let data = $state(null);
  let error = $state('');
  let asTable = $state(false);
  const DAYS = 30;

  async function load() {
    try {
      const [stats, trends, success, perf, dests] = await Promise.all([
        api.stats(),
        api.uploadTrends(DAYS),
        api.successRate(),
        api.performance(),
        api.topDestinations(8),
      ]);
      const byDate = new Map(trends.trends.map((t) => [t.date, t]));
      const days = [];
      for (let i = DAYS - 1; i >= 0; i--) {
        const d = new Date();
        d.setDate(d.getDate() - i);
        const key = d.toISOString().slice(0, 10);
        const t = byDate.get(key);
        days.push({ date: key, completed: t?.completed || 0, failed: t?.failed || 0, gb: t?.total_gb || 0 });
      }
      const statusCount = (s) => success.by_status.find((x) => x.status === s)?.count || 0;
      const finished = statusCount('completed') + statusCount('failed');
      data = {
        stats,
        days,
        perf,
        destinations: dests.destinations,
        successPct: finished ? Math.round((statusCount('completed') / finished) * 100) : null,
      };
      error = '';
    } catch (err) {
      error = err.message;
    }
  }

  // Reload when a job finishes (not on every progress update).
  let finishedCount = $derived(app.jobList.filter((j) => j.status === 'completed' || j.status === 'failed').length);
  $effect(() => {
    void finishedCount;
    load();
  });

  let periodTotals = $derived(
    data ? data.days.reduce((acc, d) => ({ completed: acc.completed + d.completed, failed: acc.failed + d.failed }), { completed: 0, failed: 0 }) : null,
  );
</script>

<section class="view">
  <header class="view-head">
    <div>
      <h1>Analytics</h1>
      <p class="sub">How uploads and transfers have gone over time.</p>
    </div>
    <button class="btn" onclick={() => openUrl(csvExportUrl())}><Download size={15} /> Export CSV</button>
  </header>

  {#if error}
    <div class="card error">{error}</div>
  {:else if !data}
    <div class="card loading">Loading…</div>
  {:else}
    <div class="tiles">
      <div class="card tile hero">
        <span class="label">Data uploaded</span>
        <span class="value">{bytes(data.stats.total_uploaded_bytes)}</span>
        <span class="muted">{count(data.stats.total_files_uploaded)} files, all time</span>
      </div>
      <div class="card tile">
        <span class="label">Jobs</span>
        <span class="value">{count(data.stats.total_jobs)}</span>
        <span class="muted">{data.stats.active_jobs} active now</span>
      </div>
      <div class="card tile">
        <span class="label">Success rate</span>
        <span class="value">{data.successPct === null ? '—' : `${data.successPct}%`}</span>
        <span class="muted">completed vs failed</span>
      </div>
      <div class="card tile">
        <span class="label">Average speed</span>
        <span class="value">{data.perf.avg_speed_mbps ? `${data.perf.avg_speed_mbps} Mb/s` : '—'}</span>
        <span class="muted">{data.perf.avg_duration_seconds ? `typical job ${duration(data.perf.avg_duration_seconds)}` : 'no completed uploads yet'}</span>
      </div>
    </div>

    <div class="card panel">
      <div class="panel-head">
        <div>
          <h3>Jobs per day</h3>
          <p class="caption">Last {DAYS} days · {periodTotals.completed} completed, {periodTotals.failed} failed</p>
        </div>
        <button class="btn ghost sm" onclick={() => (asTable = !asTable)}>
          {#if asTable}<ChartColumn size={14} /> Chart{:else}<Table size={14} /> Table{/if}
        </button>
      </div>
      {#if asTable}
        <div class="table">
          <div class="tr th"><span>Date</span><span>Completed</span><span>Failed</span><span>Uploaded</span></div>
          {#each data.days.filter((d) => d.completed || d.failed).reverse() as d (d.date)}
            <div class="tr"><span>{d.date}</span><span>{d.completed}</span><span>{d.failed}</span><span>{d.gb} GB</span></div>
          {:else}
            <div class="empty">No jobs in this period.</div>
          {/each}
        </div>
      {:else}
        <JobsChart days={data.days} />
      {/if}
    </div>

    <div class="card panel">
      <h3>Top destinations</h3>
      <p class="caption">Buckets by data uploaded</p>
      {#if data.destinations.length}
        <BarList
          items={data.destinations}
          label={(d) => d.bucket}
          value={(d) => d.total_gb * 1024 ** 3}
          format={bytes}
          detail={(d) => `${d.job_count} jobs`}
        />
      {:else}
        <div class="empty">No completed uploads yet.</div>
      {/if}
    </div>
  {/if}
</section>

<style>
  .view {
    padding: 28px 32px 40px;
    max-width: 1100px;
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
  h3 {
    font-size: 14.5px;
  }
  .sub {
    margin: 4px 0 0;
    color: var(--muted);
  }
  .error,
  .loading {
    padding: 20px;
  }
  .error {
    color: var(--danger);
    background: var(--danger-soft);
  }
  .tiles {
    display: grid;
    grid-template-columns: 1.4fr repeat(3, 1fr);
    gap: 12px;
    margin-bottom: 16px;
  }
  .tile {
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .label {
    color: var(--muted);
    font-size: 12.5px;
    font-weight: 600;
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
  .caption {
    margin: 2px 0 12px;
  }
  .panel {
    padding: 16px 18px;
    margin-bottom: 16px;
  }
  .panel-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
  }
  .table {
    display: flex;
    flex-direction: column;
    max-height: 260px;
    overflow-y: auto;
  }
  .tr {
    display: grid;
    grid-template-columns: 1.2fr 1fr 1fr 1fr;
    padding: 6px 4px;
    border-top: 1px solid var(--border);
    font-variant-numeric: tabular-nums;
  }
  .th {
    color: var(--muted);
    font-size: 12px;
    font-weight: 600;
    border-top: none;
  }
  .empty {
    padding: 20px;
    color: var(--muted);
    text-align: center;
  }
</style>
