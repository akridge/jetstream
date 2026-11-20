<script>
  // A modal sheet: backdrop, title, body and footer. Esc or a click outside closes it unless
  // `locked` (e.g. while it's changing things); focus moves into it when it opens.
  import { onMount } from 'svelte';
  import X from '@lucide/svelte/icons/x';

  let { title, subtitle = '', width = 620, locked = false, onclose, children, footer, icon } = $props();
  let box = $state();
  const id = `sheet-${Math.random().toString(36).slice(2, 8)}`;

  onMount(() => {
    const previous = document.activeElement;
    box?.focus();
    return () => previous?.focus?.();
  });

  function close() {
    if (!locked) onclose?.();
  }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && close()} />

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && close()}>
  <div class="sheet card" role="dialog" aria-modal="true" aria-labelledby={id} tabindex="-1" bind:this={box} style:width="min({width}px, 100%)">
    <header>
      {#if icon}<div class="icon">{@render icon()}</div>{/if}
      <div class="titles">
        <h2 {id}>{title}</h2>
        {#if subtitle}<p class="subtitle mono">{subtitle}</p>{/if}
      </div>
      <button class="btn ghost icon" aria-label="Close" disabled={locked} title={locked ? 'Stop it first' : 'Close'} onclick={close}><X size={18} /></button>
    </header>
    <div class="body">{@render children()}</div>
    {#if footer}<footer>{@render footer()}</footer>{/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 50;
    background: rgba(8, 15, 25, 0.45);
    backdrop-filter: blur(3px);
    display: grid;
    place-items: center;
    padding: 24px;
    animation: fade 0.14s ease-out;
  }
  .sheet {
    max-height: calc(100vh - 48px);
    display: flex;
    flex-direction: column;
    box-shadow: var(--shadow-lg);
    outline: none;
    animation: rise 0.18s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(8px) scale(0.985);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .backdrop,
    .sheet {
      animation: none;
    }
  }
  header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 16px 18px 14px 20px;
    border-bottom: 1px solid var(--border);
  }
  .icon {
    display: grid;
    place-items: center;
    flex: none;
  }
  .titles {
    flex: 1;
    min-width: 0;
  }
  h2 {
    font-size: 17px;
    margin: 0;
  }
  .subtitle {
    margin: 2px 0 0;
    font-size: 12px;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .body {
    padding: 18px 20px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 14px;
    min-height: 0;
  }
  footer {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 14px 20px;
    border-top: 1px solid var(--border);
  }
</style>
