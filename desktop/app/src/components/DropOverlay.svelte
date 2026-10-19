<script>
  import FolderDown from '@lucide/svelte/icons/folder-down';
</script>

<div class="overlay">
  <div class="box">
    <FolderDown size={44} />
    <strong>Drop a folder to sync it</strong>
    <span>JetStream will ask where in the cloud it should go.</span>
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 70;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    backdrop-filter: blur(3px);
    pointer-events: none;
  }
  .box {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 36px 48px;
    border: 2px dashed var(--accent);
    border-radius: 18px;
    background: var(--surface);
    color: var(--accent);
    box-shadow: var(--shadow-lg);
  }
  strong {
    font-size: 18px;
    color: var(--text);
  }
  span {
    color: var(--muted);
  }
</style>
