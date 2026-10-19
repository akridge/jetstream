<script>
  let { value = 0, tone = 'accent', indeterminate = false } = $props();
</script>

<div class="track" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow={indeterminate ? undefined : Math.round(value)}>
  <div class="fill {tone}" class:indeterminate style:width={indeterminate ? '35%' : `${Math.min(100, Math.max(0, value))}%`}></div>
</div>

<style>
  .track {
    height: 6px;
    border-radius: 999px;
    background: var(--surface-2);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    border-radius: 999px;
    background: var(--accent);
    transition: width 0.4s ease;
  }
  .fill.success {
    background: var(--success);
  }
  .fill.danger {
    background: var(--danger);
  }
  .indeterminate {
    animation: slide 1.3s ease-in-out infinite;
  }
  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(300%);
    }
  }
</style>
