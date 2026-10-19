<script>
  import X from '@lucide/svelte/icons/x';
  import { app } from '../lib/state.svelte.js';
</script>

<div class="toasts" aria-live="polite">
  {#each app.toasts as toast (toast.id)}
    <div class="toast {toast.kind}">
      <span class="msg">{toast.message}</span>
      {#if toast.action}
        <button class="action" onclick={() => { toast.action.run(); app.dismissToast(toast.id); }}>{toast.action.label}</button>
      {/if}
      <button class="close" aria-label="Dismiss" onclick={() => app.dismissToast(toast.id)}><X size={14} /></button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 20px;
    bottom: 20px;
    z-index: 60;
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: 360px;
  }
  .toast {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 11px 12px 11px 14px;
    border-radius: var(--radius);
    background: var(--surface);
    border: 1px solid var(--border);
    border-left: 4px solid var(--accent);
    box-shadow: var(--shadow-lg);
    animation: up 0.18s ease-out;
  }
  .toast.success {
    border-left-color: var(--success);
  }
  .toast.error {
    border-left-color: var(--danger);
  }
  @keyframes up {
    from {
      transform: translateY(8px);
      opacity: 0;
    }
  }
  .msg {
    flex: 1;
    font-size: 13px;
  }
  .action {
    border: none;
    background: none;
    color: var(--accent);
    font-weight: 650;
    cursor: pointer;
  }
  .close {
    border: none;
    background: none;
    color: var(--muted);
    cursor: pointer;
    display: grid;
    place-items: center;
  }
</style>
