<script>
  // The picture area of one tile in the cloud browser's thumbnail view. Asks for a thumbnail
  // only while on screen (and stops asking if scrolled away before its turn).
  import Folder from '@lucide/svelte/icons/folder';
  import File from '@lucide/svelte/icons/file';
  import FileImage from '@lucide/svelte/icons/file-image';
  import FileVideo from '@lucide/svelte/icons/file-video-camera';
  import Snowflake from '@lucide/svelte/icons/snowflake';
  import { thumbLoader, isPreviewable } from '../lib/thumbs.svelte.js';
  import { bytes } from '../lib/format.js';

  let { bucket, entry } = $props();

  let el = $state();
  let previewable = $derived(isPreviewable(entry));
  let result = $derived(previewable ? thumbLoader.results[thumbLoader.key(bucket, entry)] : null);
  let Icon = $derived(
    entry.is_folder ? Folder : previewable ? FileImage : (entry.content_type || '').startsWith('video/') || /\.(mp4|mov|avi|mkv)$/i.test(entry.name) ? FileVideo : File,
  );
  const className = (c) => (c ? c.charAt(0) + c.slice(1).toLowerCase() : '');

  $effect(() => {
    if (!previewable || !el) return;
    const node = el;
    const observer = new IntersectionObserver(
      ([seen]) => {
        if (seen.isIntersecting) thumbLoader.request(bucket, entry);
        else if (thumbLoader.results[thumbLoader.key(bucket, entry)]?.loading) thumbLoader.cancel(bucket, entry);
      },
      { rootMargin: '300px' },
    );
    observer.observe(node);
    return () => observer.disconnect();
  });

  function loadAnyway(event, options) {
    event.stopPropagation();
    thumbLoader.request(bucket, entry, { ...options, force: true });
  }
</script>

<div class="pic" bind:this={el}>
  {#if result?.data_url}
    <img src={result.data_url} alt="" draggable="false" />
  {:else if result?.loading}
    <div class="shimmer" aria-label="Loading preview"></div>
  {:else if result?.skip === 'cold'}
    <button class="ask" title="Retrieving from {className(result.storage_class)} storage is charged" onclick={(e) => loadAnyway(e, { allowCold: true })}>
      <Snowflake size={18} />
      <span>{className(result.storage_class)}</span>
      <small>Click to preview</small>
    </button>
  {:else if result?.skip === 'too_large'}
    <button class="ask" title="Download the whole file ({bytes(result.size)}) to preview it" onclick={(e) => loadAnyway(e, { maxMib: Math.ceil(result.size / 1048576) + 1 })}>
      <FileImage size={18} />
      <span>{bytes(result.size)}</span>
      <small>Too large · click to preview</small>
    </button>
  {:else if result?.skip === 'error'}
    <button class="ask" title={result.message} onclick={(e) => loadAnyway(e, {})}>
      <FileImage size={18} />
      <small>Couldn't load · click to retry</small>
    </button>
  {:else}
    <span class="icon" title={result?.skip === 'unreadable' ? `No preview: ${result.message}` : ''}>
      <Icon size={34} color={entry.is_folder ? 'var(--accent)' : 'var(--muted)'} strokeWidth={1.5} />
    </span>
  {/if}
</div>

<style>
  .pic {
    aspect-ratio: 4 / 3;
    width: 100%;
    border-radius: 8px;
    background: var(--surface-2);
    display: grid;
    place-items: center;
    overflow: hidden;
  }
  img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .shimmer {
    width: 100%;
    height: 100%;
    background: linear-gradient(90deg, var(--surface-2) 0%, var(--surface-hover, var(--surface)) 50%, var(--surface-2) 100%);
    background-size: 200% 100%;
    animation: shimmer 1.2s infinite;
  }
  @keyframes shimmer {
    to {
      background-position: -200% 0;
    }
  }
  .ask {
    border: none;
    background: none;
    color: var(--text-2);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    cursor: pointer;
    font-size: 12px;
    padding: 8px;
    text-align: center;
  }
  .ask:hover {
    color: var(--accent);
  }
  .ask small {
    color: var(--muted);
    font-size: 11px;
  }
  .icon {
    display: grid;
    place-items: center;
  }
</style>
