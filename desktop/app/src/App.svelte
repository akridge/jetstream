<script>
  import { onMount } from 'svelte';
  import Sidebar from './components/Sidebar.svelte';
  import Toasts from './components/Toasts.svelte';
  import OpsTray from './components/OpsTray.svelte';
  import DropOverlay from './components/DropOverlay.svelte';
  import FolderDialog from './components/FolderDialog.svelte';
  import FoldersView from './views/FoldersView.svelte';
  import ActivityView from './views/ActivityView.svelte';
  import SettingsView from './views/SettingsView.svelte';
  import MoreView from './views/MoreView.svelte';
  import CloudView from './views/CloudView.svelte';
  import InsightsView from './views/InsightsView.svelte';
  import AnalyticsView from './views/AnalyticsView.svelte';
  import AuditView from './views/AuditView.svelte';
  import { app } from './lib/state.svelte.js';
  import { onFolderDrop, serverLog } from './lib/platform.js';

  let dragging = $state(false);
  let dialogRef = $state();
  let log = $state('');

  onMount(() => {
    app.start();
    const unlisten = onFolderDrop(
      (paths) => {
        if (app.server.phase !== 'ready') return;
        if (app.folderDialog && dialogRef) dialogRef.setFolder(paths[0]);
        else if (app.view === 'insights') app.scanRequest = paths[0];
        else app.addFolder(paths[0]);
      },
      (hovering) => (dragging = hovering && app.server.phase === 'ready'),
    );
    return () => unlisten.then((fn) => fn());
  });

  $effect(() => {
    if (app.server.phase === 'failed') serverLog().then((text) => (log = text));
  });
</script>

{#if app.server.phase === 'ready'}
  <div class="shell">
    <Sidebar />
    <main>
      {#if app.view === 'folders'}
        <FoldersView />
      {:else if app.view === 'activity'}
        <ActivityView />
      {:else if app.view === 'cloud'}
        <CloudView />
      {:else if app.view === 'audit'}
        <AuditView />
      {:else if app.view === 'insights'}
        <InsightsView />
      {:else if app.view === 'analytics'}
        <AnalyticsView />
      {:else if app.view === 'settings'}
        <SettingsView />
      {:else}
        <MoreView />
      {/if}
    </main>
  </div>
  {#if app.folderDialog}
    {#key app.folderDialog}
      <FolderDialog bind:this={dialogRef} dialog={app.folderDialog} />
    {/key}
  {/if}
  {#if dragging}<DropOverlay />{/if}
  <OpsTray />
  <Toasts />
{:else}
  <div class="splash">
    <img src="/icon.png" alt="" />
    <h1>JetStream</h1>
    {#if app.server.phase === 'failed'}
      <div class="failed">
        <p class="error">{app.server.message || 'JetStream could not start.'}</p>
        <pre>{log || '(no server output)'}</pre>
        <p class="hint">Quit JetStream from the tray icon and open it again. If it keeps failing, send the text above to your data manager.</p>
      </div>
    {:else}
      <div class="loader"></div>
      <p class="hint">Starting…</p>
    {/if}
  </div>
{/if}

<style>
  .shell {
    display: flex;
    height: 100%;
  }
  main {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
  }
  .splash {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 24px;
  }
  .splash img {
    width: 96px;
    height: 96px;
    border-radius: 22px;
    background: #fff;
    padding: 4px;
    box-shadow: var(--shadow-lg);
  }
  .splash h1 {
    font-size: 22px;
    margin-top: 6px;
  }
  .loader {
    width: 30px;
    height: 30px;
    margin-top: 10px;
    border: 3px solid var(--border-strong);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  .hint {
    color: var(--muted);
  }
  .failed {
    max-width: 720px;
    width: 100%;
  }
  .error {
    color: var(--danger);
    font-weight: 600;
    text-align: center;
  }
  pre {
    max-height: 320px;
    overflow: auto;
    padding: 12px;
    border-radius: var(--radius-sm);
    background: var(--surface-2);
    font-family: var(--mono);
    font-size: 12px;
    white-space: pre-wrap;
  }
</style>
