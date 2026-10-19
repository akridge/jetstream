<script>
  import FolderSync from '@lucide/svelte/icons/folder-sync';
  import Activity from '@lucide/svelte/icons/activity';
  import Settings from '@lucide/svelte/icons/settings';
  import LayoutGrid from '@lucide/svelte/icons/layout-grid';
  import Cloud from '@lucide/svelte/icons/cloud';
  import FolderSearch from '@lucide/svelte/icons/folder-search';
  import ChartColumn from '@lucide/svelte/icons/chart-column';
  import Brush from '@lucide/svelte/icons/brush';
  import { app } from '../lib/state.svelte.js';

  const groups = [
    {
      title: 'Sync',
      items: [
        { id: 'folders', label: 'Synced folders', icon: FolderSync },
        { id: 'activity', label: 'Activity', icon: Activity },
      ],
    },
    {
      title: 'Cloud',
      items: [
        { id: 'cloud', label: 'Cloud storage', icon: Cloud },
        { id: 'audit', label: 'Cloud cleanup', icon: Brush },
      ],
    },
    {
      title: 'Insights',
      items: [
        { id: 'insights', label: 'Folder insights', icon: FolderSearch },
        { id: 'analytics', label: 'Analytics', icon: ChartColumn },
      ],
    },
    {
      title: '',
      items: [
        { id: 'more', label: 'More tools', icon: LayoutGrid },
        { id: 'settings', label: 'Settings', icon: Settings },
      ],
    },
  ];

  let auth = $derived(app.auth);
  let authState = $derived(
    !auth ? 'unknown' : auth.gcloud_account && auth.adc_available ? 'ok' : auth.signed_in ? 'partial' : 'none',
  );
</script>

<aside>
  <div class="brand">
    <img src="/icon.png" alt="" />
    <div>
      <div class="name">JetStream</div>
      <div class="tag">NOAA cloud data transfer</div>
    </div>
  </div>

  <nav>
    {#each groups as group, g (g)}
      <div class="group" class:spacer={!group.title}>
        {#if group.title}<div class="group-title">{group.title}</div>{/if}
        {#each group.items as item (item.id)}
          {@const Icon = item.icon}
          <button class="nav-item" class:active={app.view === item.id} onclick={() => (app.view = item.id)}>
            <Icon size={18} />
            <span>{item.label}</span>
            {#if item.id === 'activity' && app.activeCount}
              <span class="badge">{app.activeCount}</span>
            {:else if item.id === 'activity' && app.failedCount}
              <span class="badge danger" title="Failed jobs">{app.failedCount}</span>
            {/if}
          </button>
        {/each}
      </div>
    {/each}
  </nav>

  <div class="footer">
    <button class="account" onclick={() => (app.view = 'settings')} title="Google Cloud sign-in">
      <span class="dot {authState}"></span>
      <span class="truncate">
        {#if authState === 'ok' || authState === 'partial'}
          {auth.gcloud_account || auth.service_account || 'Signed in'}
        {:else if authState === 'none'}
          Not signed in
        {:else}
          Checking sign-in…
        {/if}
      </span>
    </button>
    <div class="meta">
      <span class="live" class:on={app.live} title={app.live ? 'Live updates connected' : 'Reconnecting…'}></span>
      {app.live ? 'Live' : 'Offline'}{app.version ? ` · v${app.version}` : ''}
    </div>
  </div>
</aside>

<style>
  aside {
    width: 232px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    background: var(--sidebar);
    color: var(--sidebar-text);
    padding: 16px 12px 12px;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 8px 20px;
  }
  .brand img {
    width: 38px;
    height: 38px;
    border-radius: 9px;
    background: #fff;
    padding: 2px;
  }
  .name {
    color: #fff;
    font-weight: 700;
    font-size: 16px;
    letter-spacing: -0.01em;
  }
  .tag {
    color: var(--sidebar-muted);
    font-size: 11.5px;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 14px;
    flex: 1;
  }
  .group {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .group.spacer {
    margin-top: auto;
    padding-bottom: 8px;
  }
  .group-title {
    padding: 0 12px 4px;
    color: var(--sidebar-muted);
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .nav-item {
    display: flex;
    align-items: center;
    gap: 11px;
    height: 38px;
    padding: 0 12px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--sidebar-text);
    font-weight: 550;
    text-align: left;
    cursor: pointer;
  }
  .nav-item:hover {
    background: var(--sidebar-2);
    color: #fff;
  }
  .nav-item.active {
    background: var(--sidebar-2);
    color: #fff;
    box-shadow: inset 3px 0 0 #4d9cff;
  }
  .nav-item span:first-of-type {
    flex: 1;
  }
  .badge {
    min-width: 20px;
    height: 20px;
    padding: 0 6px;
    border-radius: 999px;
    background: #4d9cff;
    color: #06121f;
    font-size: 11.5px;
    font-weight: 700;
    display: grid;
    place-items: center;
  }
  .badge.danger {
    background: #ff6b6f;
  }
  .footer {
    border-top: 1px solid rgba(255, 255, 255, 0.08);
    padding-top: 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .account {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--sidebar-text);
    font-size: 12.5px;
    cursor: pointer;
    text-align: left;
    min-width: 0;
  }
  .account:hover {
    background: var(--sidebar-2);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
    background: var(--sidebar-muted);
  }
  .dot.ok {
    background: #3ccf91;
  }
  .dot.partial,
  .dot.none {
    background: #f0a641;
  }
  .meta {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 10px;
    color: var(--sidebar-muted);
    font-size: 11.5px;
  }
  .live {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--sidebar-muted);
  }
  .live.on {
    background: #3ccf91;
    box-shadow: 0 0 0 3px rgba(60, 207, 145, 0.18);
  }
</style>
