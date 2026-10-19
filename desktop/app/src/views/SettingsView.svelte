<script>
  import LogIn from '@lucide/svelte/icons/log-in';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import ExternalLink from '@lucide/svelte/icons/external-link';
  import Sun from '@lucide/svelte/icons/sun';
  import Moon from '@lucide/svelte/icons/moon';
  import Monitor from '@lucide/svelte/icons/monitor';
  import { app } from '../lib/state.svelte.js';
  import { api, apiBase } from '../lib/api.js';
  import { ensureNotificationPermission, openClassic, openUrl } from '../lib/platform.js';

  let checking = $state(false);
  let signingIn = $state(false);
  let auth = $derived(app.auth);

  async function refresh() {
    checking = true;
    try {
      await app.refreshAuth(true);
    } finally {
      checking = false;
    }
  }

  async function signIn() {
    signingIn = true;
    try {
      await api.signIn();
      app.toast('Finish signing in in your browser, then come back here.', 'info', { timeout: 8000 });
      // Re-check until the sign-in window is done (up to ~5 minutes).
      for (let i = 0; i < 60; i++) {
        await new Promise((r) => setTimeout(r, 5000));
        await app.refreshAuth(true);
        if (!app.auth?.login_in_progress) break;
      }
    } catch (err) {
      app.toast(err.message, 'error', { timeout: 10000 });
    } finally {
      signingIn = false;
    }
  }

  function setNotifications(on) {
    app.savePrefs({ notifications: on });
    if (on) ensureNotificationPermission();
  }
</script>

<section class="view">
  <header class="view-head">
    <h1>Settings</h1>
  </header>

  <div class="stack">
    <div class="card panel">
      <div class="panel-head">
        <div>
          <h2>Google Cloud account</h2>
          <p class="sub">JetStream uses your own Google sign-in. No shared keys are stored.</p>
        </div>
        <button class="btn ghost sm" disabled={checking} onclick={refresh}><RefreshCw size={14} class={checking ? 'spin' : ''} /> Check again</button>
      </div>
      {#if !auth}
        <p class="muted">Checking…</p>
      {:else if !auth.gcloud_installed && !auth.adc_available}
        <div class="callout warning">
          The Google Cloud SDK isn't installed. It's needed to sign in and for gcloud uploads.
          <button class="btn sm" onclick={() => openUrl('https://cloud.google.com/sdk/docs/install')}><ExternalLink size={14} /> Install it</button>
        </div>
      {:else}
        <div class="account-grid">
          <div>
            <div class="label">Signed in as</div>
            <div class="value">{auth.gcloud_account || auth.service_account || '—'}</div>
          </div>
          <div>
            <div class="label">Uploads (gcloud)</div>
            <div class="value">{#if auth.gcloud_account}<span class="pill success">Ready</span>{:else}<span class="pill warning">Not signed in</span>{/if}</div>
          </div>
          <div>
            <div class="label">Bucket browsing &amp; engine</div>
            <div class="value">{#if auth.adc_available}<span class="pill success">Ready</span>{:else}<span class="pill warning">Not signed in</span>{/if}</div>
          </div>
        </div>
        {#if !auth.gcloud_account || !auth.adc_available || auth.login_in_progress}
          <button class="btn primary" disabled={signingIn || auth.login_in_progress} onclick={signIn}>
            <LogIn size={16} /> {auth.login_in_progress ? 'Waiting for browser sign-in…' : 'Sign in with Google'}
          </button>
        {/if}
      {/if}
    </div>

    <div class="card panel">
      <h2>Uploads</h2>
      <p class="sub">The tool new folders use. Each folder can override this when you edit it.</p>
      <div class="choices">
        {#each app.tools as tool (tool.id)}
          <label class="choice" class:selected={app.prefs.default_upload_tool === tool.id} class:disabled={!tool.available}>
            <input
              type="radio"
              name="tool"
              value={tool.id}
              disabled={!tool.available}
              checked={app.prefs.default_upload_tool === tool.id}
              onchange={() => app.savePrefs({ default_upload_tool: tool.id })}
            />
            <span>
              <strong>{tool.label}</strong>
              <small>{tool.available ? tool.description : tool.detail}</small>
            </span>
          </label>
        {/each}
      </div>
    </div>

    <div class="card panel">
      <h2>Appearance &amp; notifications</h2>
      <div class="row">
        <span class="label">Theme</span>
        <div class="segmented">
          {#each [['system', 'System', Monitor], ['light', 'Light', Sun], ['dark', 'Dark', Moon]] as [id, label, Icon] (id)}
            <button class:active={app.prefs.theme === id} onclick={() => app.savePrefs({ theme: id })}><Icon size={14} /> {label}</button>
          {/each}
        </div>
      </div>
      <label class="row toggle">
        <span>
          <span class="label">Notify me when jobs finish</span>
          <small class="muted">A desktop notification when an upload, preview or cloud operation completes or fails.</small>
        </span>
        <input type="checkbox" checked={app.prefs.notifications} onchange={(e) => setNotifications(e.currentTarget.checked)} />
      </label>
    </div>

    <div class="card panel">
      <h2>More settings</h2>
      <p class="sub">Filters, Drive and other options are still on the classic settings page.</p>
      <button class="btn" onclick={() => openClassic('static/settings.html', apiBase())}><ExternalLink size={15} /> Open classic settings</button>
    </div>
  </div>
</section>

<style>
  .view {
    padding: 28px 32px 40px;
    max-width: 860px;
  }
  .view-head {
    margin-bottom: 20px;
  }
  h1 {
    font-size: 24px;
  }
  h2 {
    font-size: 16px;
  }
  .stack {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .panel {
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .panel-head {
    display: flex;
    justify-content: space-between;
    gap: 12px;
  }
  .sub,
  .muted {
    color: var(--muted);
    margin: 3px 0 0;
  }
  .label {
    font-weight: 600;
    font-size: 13px;
    color: var(--text-2);
  }
  .value {
    margin-top: 4px;
    word-break: break-all;
  }
  .panel > .btn {
    align-self: flex-start;
  }
  .account-grid {
    display: grid;
    grid-template-columns: 1.4fr 1fr 1fr;
    gap: 16px;
  }
  .callout {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 14px;
    border-radius: var(--radius-sm);
  }
  .callout.warning {
    background: var(--warning-soft);
    color: var(--warning);
  }
  .choices {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .choice {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    padding: 12px 14px;
    border: 1.5px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .choice.selected {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .choice.disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }
  .choice input {
    margin-top: 3px;
    accent-color: var(--accent);
  }
  .choice span {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .choice small {
    color: var(--muted);
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }
  .toggle {
    cursor: pointer;
  }
  .toggle > span {
    display: flex;
    flex-direction: column;
  }
  .toggle input {
    width: 18px;
    height: 18px;
    accent-color: var(--accent);
  }
  .segmented {
    display: inline-flex;
    padding: 3px;
    border-radius: 9px;
    background: var(--surface-2);
    border: 1px solid var(--border);
  }
  .segmented button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 12px;
    border: none;
    border-radius: 7px;
    background: transparent;
    color: var(--text-2);
    font-weight: 550;
    cursor: pointer;
  }
  .segmented button.active {
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow);
  }
</style>
