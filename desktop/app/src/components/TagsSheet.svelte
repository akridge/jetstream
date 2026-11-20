<script>
  // Tags for one or more synced folders: tick existing tags or add new ones.
  import { untrack } from 'svelte';
  import Tag from '@lucide/svelte/icons/tag';
  import Plus from '@lucide/svelte/icons/plus';
  import Sheet from './Sheet.svelte';
  import { app } from '../lib/state.svelte.js';
  import { allTags, tagsOf, updateOptions } from '../lib/folders.svelte.js';

  let { presets, onclose } = $props();
  const targets = untrack(() => presets);
  const existing = allTags(app.presets).map(([t]) => t);
  // A tag every chosen folder has is ticked; one some have is "mixed" (left as is unless clicked).
  let state = $state(Object.fromEntries(existing.map((t) => {
    const n = targets.filter((p) => tagsOf(p).includes(t)).length;
    return [t, n === targets.length ? 'on' : n ? 'mixed' : 'off'];
  })));
  let added = $state('');
  let saving = $state(false);
  let names = $derived(Object.keys(state));

  function addTag() {
    const tag = added.trim().replace(/\s+/g, ' ').slice(0, 40);
    if (tag && !names.some((n) => n.toLowerCase() === tag.toLowerCase())) state = { ...state, [tag]: 'on' };
    else if (tag) state = { ...state, [names.find((n) => n.toLowerCase() === tag.toLowerCase())]: 'on' };
    added = '';
  }

  async function save() {
    saving = true;
    try {
      for (const p of targets) {
        const now = new Set(tagsOf(p));
        for (const [tag, s] of Object.entries(state)) {
          if (s === 'on') now.add(tag);
          else if (s === 'off') now.delete(tag);
        }
        const tags = [...now].sort((a, b) => a.localeCompare(b));
        if (JSON.stringify(tags) !== JSON.stringify(tagsOf(p))) await updateOptions(p, { tags });
      }
      await app.refreshPresets();
      onclose();
    } catch (err) {
      app.toast(`Couldn't save the tags: ${err.message}`, 'error');
    } finally {
      saving = false;
    }
  }
</script>

<Sheet title="Tags" subtitle={targets.length === 1 ? targets[0].name : `${targets.length} folders`} width={440} {onclose}>
  {#snippet icon()}<span class="badge"><Tag size={17} /></span>{/snippet}
  <p class="sub">Group folders by project, cruise or season, then filter the list by tag.</p>
  {#if names.length}
    <div class="tags">
      {#each names as tag (tag)}
        <label class="tag" class:on={state[tag] === 'on'} class:mixed={state[tag] === 'mixed'}>
          <input
            type="checkbox"
            checked={state[tag] === 'on'}
            indeterminate={state[tag] === 'mixed'}
            onchange={(e) => (state = { ...state, [tag]: e.currentTarget.checked ? 'on' : 'off' })}
          />
          {tag}
        </label>
      {/each}
    </div>
  {/if}
  <form class="add" onsubmit={(e) => { e.preventDefault(); addTag(); }}>
    <input class="input" placeholder="New tag, e.g. HA2401" bind:value={added} aria-label="New tag" />
    <button class="btn" type="submit" disabled={!added.trim()}><Plus size={15} /> Add</button>
  </form>
  {#snippet footer()}
    <span class="grow"></span>
    <button class="btn ghost" onclick={onclose}>Cancel</button>
    <button class="btn primary" disabled={saving} onclick={save}>Save</button>
  {/snippet}
</Sheet>

<style>
  .badge {
    width: 32px;
    height: 32px;
    border-radius: 9px;
    display: grid;
    place-items: center;
    background: var(--accent-soft);
    color: var(--accent);
  }
  .sub {
    margin: 0;
    color: var(--text-2);
    font-size: 13px;
  }
  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .tag {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 30px;
    padding: 0 12px 0 8px;
    border-radius: 999px;
    border: 1px solid var(--border-strong);
    font-size: 13px;
    cursor: pointer;
  }
  .tag.on {
    border-color: var(--accent);
    background: var(--accent-soft);
    color: var(--accent);
  }
  .tag.mixed {
    border-style: dashed;
  }
  .add {
    display: flex;
    gap: 8px;
  }
  .grow {
    flex: 1;
  }
</style>
