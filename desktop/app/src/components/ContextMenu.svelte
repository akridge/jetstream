<script>
  /** items: [{ label, icon?, shortcut?, danger?, disabled?, title?, run }] or 'separator'. */
  let { x, y, items, onclose } = $props();

  let menu = $state();
  let pos = $state({ left: 0, top: 0 });

  // Keep the menu inside the window.
  $effect(() => {
    if (!menu) return;
    const { width, height } = menu.getBoundingClientRect();
    pos = { left: Math.min(x, window.innerWidth - width - 8), top: Math.min(y, window.innerHeight - height - 8) };
    menu.querySelector('button:not(:disabled)')?.focus();
  });

  function keydown(e) {
    if (e.key === 'Escape') return onclose();
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    e.preventDefault();
    const buttons = [...menu.querySelectorAll('button:not(:disabled)')];
    const i = buttons.indexOf(document.activeElement);
    buttons[(i + (e.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length]?.focus();
  }
</script>

<svelte:window onclick={onclose} oncontextmenu={onclose} onblur={onclose} onresize={onclose} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="menu card"
  role="menu"
  tabindex="-1"
  bind:this={menu}
  style:left="{pos.left}px"
  style:top="{pos.top}px"
  onkeydown={keydown}
  onclick={(e) => e.stopPropagation()}
  oncontextmenu={(e) => { e.preventDefault(); e.stopPropagation(); }}
>
  {#each items as item, i (i)}
    {#if item === 'separator'}
      <div class="sep" role="separator"></div>
    {:else}
      {@const Icon = item.icon}
      <button
        role="menuitem"
        class:danger={item.danger}
        disabled={item.disabled}
        title={item.title || ''}
        onclick={() => { onclose(); item.run(); }}
      >
        <span class="icon">{#if Icon}<Icon size={15} />{/if}</span>
        <span class="text">{item.label}</span>
        {#if item.shortcut}<kbd>{item.shortcut}</kbd>{/if}
      </button>
    {/if}
  {/each}
</div>

<style>
  .menu {
    position: fixed;
    z-index: 70;
    min-width: 220px;
    padding: 5px;
    display: flex;
    flex-direction: column;
    box-shadow: var(--shadow-lg);
  }
  button {
    display: flex;
    align-items: center;
    gap: 9px;
    border: none;
    background: none;
    text-align: left;
    padding: 7px 10px;
    border-radius: 6px;
    cursor: pointer;
    color: var(--text);
    font: inherit;
  }
  button:hover:not(:disabled),
  button:focus-visible {
    background: var(--surface-2);
    outline: none;
  }
  button:disabled {
    color: var(--muted);
    cursor: default;
    opacity: 0.6;
  }
  .danger {
    color: var(--danger);
  }
  .icon {
    width: 16px;
    display: grid;
    place-items: center;
    color: var(--muted);
  }
  .danger .icon {
    color: var(--danger);
  }
  .text {
    flex: 1;
  }
  kbd {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--muted);
  }
  .sep {
    height: 1px;
    margin: 4px 6px;
    background: var(--border);
  }
</style>
