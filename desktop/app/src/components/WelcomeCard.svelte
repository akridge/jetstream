<script>
  // First run: three steps to a first verified upload, ticked off as they happen.
  import Check from '@lucide/svelte/icons/check';
  import X from '@lucide/svelte/icons/x';
  import PartyPopper from '@lucide/svelte/icons/party-popper';
  import { app, TERMINAL } from '../lib/state.svelte.js';
  import { pickFolder } from '../lib/platform.js';

  let signedIn = $derived(!!(app.auth && (app.auth.gcloud_account || app.auth.service_account) && app.auth.adc_available !== false));
  let hasFolder = $derived(app.presets.length > 0);
  let uploaded = $derived(app.jobList.some((j) => TERMINAL.has(j.status) && j.status === 'completed' && !j.dry_run && !j.filters?.job_type));
  let steps = $derived([
    { done: signedIn, title: 'Sign in with Google', text: 'So JetStream can reach your buckets.', action: 'Sign in', run: () => (app.view = 'settings') },
    { done: hasFolder, title: 'Add a folder', text: 'Pick a folder (or a network share) and where it goes in the cloud.', action: 'Add folder', run: add },
    { done: uploaded, title: 'Preview, then upload', text: 'Preview shows what will go up; then upload it. Every file is verified afterwards.', action: null },
  ]);
  let next = $derived(steps.findIndex((s) => !s.done));
  let allDone = $derived(next < 0);

  async function add() {
    const path = await pickFolder();
    if (path) app.addFolder(path);
  }
  function finish() {
    app.savePrefs({ welcome_done: true });
  }
</script>

<div class="welcome card" class:done={allDone} aria-label="Getting started">
  {#if allDone}
    <span class="party"><PartyPopper size={26} /></span>
    <div class="text">
      <h2>You're all set</h2>
      <p>Your first folder is in the cloud and verified. JetStream keeps watching it for new files.</p>
    </div>
    <button class="btn primary" onclick={finish}>Done</button>
  {:else}
    <div class="head">
      <div>
        <h2>Welcome to JetStream</h2>
        <p>Three steps to your first verified upload.</p>
      </div>
      <button class="btn ghost sm icon" aria-label="Hide getting started" title="Hide" onclick={finish}><X size={16} /></button>
    </div>
    <ol class="steps">
      {#each steps as step, i (step.title)}
        <li class:done={step.done} class:now={i === next}>
          <span class="mark">{#if step.done}<Check size={14} strokeWidth={3} />{:else}{i + 1}{/if}</span>
          <div>
            <strong>{step.title}</strong>
            <span>{step.text}</span>
          </div>
          {#if i === next && step.action}<button class="btn primary sm" onclick={step.run}>{step.action}</button>{/if}
        </li>
      {/each}
    </ol>
  {/if}
</div>

<style>
  .welcome {
    padding: 18px 20px;
    margin-bottom: 16px;
    background: linear-gradient(135deg, var(--accent-soft), var(--surface) 60%);
  }
  .welcome.done {
    display: flex;
    align-items: center;
    gap: 16px;
    background: linear-gradient(135deg, var(--success-soft), var(--surface) 70%);
  }
  .party {
    color: var(--success);
    display: grid;
    animation: pop 0.6s cubic-bezier(0.2, 1.6, 0.4, 1);
  }
  @keyframes pop {
    0% {
      transform: scale(0.3) rotate(-20deg);
      opacity: 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .party {
      animation: none;
    }
  }
  .text {
    flex: 1;
  }
  h2 {
    font-size: 18px;
    margin: 0;
  }
  p {
    margin: 2px 0 0;
    color: var(--text-2);
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    margin-bottom: 12px;
  }
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 12px;
  }
  .steps li {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--surface);
  }
  .steps li > div {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .steps li span:not(.mark) {
    font-size: 12.5px;
    color: var(--muted);
  }
  .steps li.now {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .steps li.done strong {
    color: var(--muted);
    text-decoration: line-through;
  }
  .steps li .btn {
    align-self: flex-start;
  }
  .mark {
    width: 24px;
    height: 24px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    font-size: 12px;
    font-weight: 700;
    border: 2px solid var(--border-strong);
    color: var(--muted);
  }
  .done .mark {
    border-color: var(--success);
    background: var(--success);
    color: #fff;
  }
  .now .mark {
    border-color: var(--accent);
    color: var(--accent);
  }
  @media (max-width: 900px) {
    .steps {
      grid-template-columns: 1fr;
    }
  }
</style>
