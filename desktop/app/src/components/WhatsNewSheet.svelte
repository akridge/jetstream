<script>
  // "What's new" for a version, from the bundled changelog. Shown once after an update, and
  // from Settings → About or the command palette any time.
  import Sparkles from '@lucide/svelte/icons/sparkles';
  import Sheet from './Sheet.svelte';
  import changelog from '../../../CHANGELOG.md?raw';
  import { app } from '../lib/state.svelte.js';
  import { releaseNotes } from '../lib/changelog.js';

  let { version } = $props();
  let notes = $derived(releaseNotes(changelog, version));
</script>

<Sheet title="What's new in JetStream {version}" width={640} onclose={() => (app.whatsNew = null)}>
  {#snippet icon()}<span class="badge"><Sparkles size={18} /></span>{/snippet}
  {#if !notes}
    <p class="muted">No notes for this version.</p>
  {:else}
    {#if notes.intro}<p class="intro">{notes.intro}</p>{/if}
    {#each notes.groups as group (group.title)}
      <section>
        {#if group.title}<h3>{group.title}</h3>{/if}
        <ul>
          {#each group.items as item, i (i)}
            <li>{#each item as run, j (j)}{#if run.bold}<strong>{run.text}</strong>{:else}{run.text}{/if}{/each}</li>
          {/each}
        </ul>
      </section>
    {/each}
  {/if}
  {#snippet footer()}
    <span class="grow"></span>
    <button class="btn primary" onclick={() => (app.whatsNew = null)}>Got it</button>
  {/snippet}
</Sheet>

<style>
  .badge {
    width: 34px;
    height: 34px;
    border-radius: 10px;
    display: grid;
    place-items: center;
    background: linear-gradient(135deg, var(--accent-soft), var(--success-soft));
    color: var(--accent);
  }
  .intro {
    margin: 0;
    font-size: 14.5px;
    color: var(--text-2);
  }
  h3 {
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted);
    margin: 4px 0 6px;
  }
  ul {
    margin: 0;
    padding-left: 18px;
    display: grid;
    gap: 6px;
    font-size: 13.5px;
    line-height: 1.5;
  }
  .grow {
    flex: 1;
  }
  .muted {
    color: var(--muted);
  }
</style>
