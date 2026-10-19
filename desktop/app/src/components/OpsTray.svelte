<script>
  import Square from '@lucide/svelte/icons/square';
  import ProgressBar from './ProgressBar.svelte';
  import { cloudOps } from '../lib/cloudops.svelte.js';
  import { count } from '../lib/format.js';
</script>

{#if cloudOps.list.some((op) => op.shown)}
  <div class="tray" aria-live="polite">
    {#each cloudOps.list.filter((op) => op.shown) as op (op.id)}
      {@const finished = op.done + op.skipped + op.failed}
      <div class="op card">
        <div class="row">
          <span class="label truncate" title={op.label}>{op.label}</span>
          {#if op.status === 'running'}
            <button class="btn ghost sm icon" aria-label="Stop" title="Stop" onclick={() => cloudOps.cancel(op.id)}><Square size={12} /></button>
          {/if}
        </div>
        <ProgressBar value={op.total ? (finished / op.total) * 100 : 0} indeterminate={!op.total && op.status === 'running'} />
        <span class="muted">{count(finished)}{op.total ? ` of ${count(op.total)}` : ''} files{op.failed ? ` · ${count(op.failed)} failed` : ''}</span>
      </div>
    {/each}
  </div>
{/if}

<style>
  .tray {
    position: fixed;
    left: 256px;
    bottom: 20px;
    z-index: 45;
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: 320px;
  }
  .op {
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    box-shadow: var(--shadow-lg);
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .label {
    font-weight: 600;
    font-size: 13px;
  }
  .muted {
    color: var(--muted);
    font-size: 12px;
  }
</style>
