<script>
  import Cloud from '@lucide/svelte/icons/cloud';
  import Brush from '@lucide/svelte/icons/brush';
  import HardDrive from '@lucide/svelte/icons/hard-drive';
  import LibraryBig from '@lucide/svelte/icons/library-big';
  import FolderUp from '@lucide/svelte/icons/folder-up';
  import Upload from '@lucide/svelte/icons/upload';
  import ExternalLink from '@lucide/svelte/icons/external-link';
  import { apiBase } from '../lib/api.js';
  import { openClassic } from '../lib/platform.js';

  const tools = [
    { path: 'static/cloud-audit.html', title: 'Cloud audit history', text: 'Earlier audit runs and quarantine results (classic view).', icon: Brush },
    { path: 'static/dataset-creator.html', title: 'Cloud catalog tool', text: 'Build browsable catalogs of images and media in a bucket.', icon: LibraryBig },
    { path: 'static/gdrive.html', title: 'Google Drive upload', text: 'Upload or sync folders to Google Drive.', icon: FolderUp },
    { path: 'static/local-audit.html', title: 'Local audit reports', text: 'Saved local audit runs with per-file detail and CSV export.', icon: HardDrive },
    { path: 'static/uploads.html', title: 'Classic uploads page', text: 'The original upload form, with scheduling and split-by-subfolder.', icon: Upload },
    { path: 'static/cloud.html', title: 'Classic cloud operations', text: 'The original cloud sync and move page.', icon: Cloud },
  ];
</script>

<section class="view">
  <header class="view-head">
    <h1>More tools</h1>
    <p class="sub">Tools that haven't moved into the new app yet open in a separate window.</p>
  </header>

  <div class="grid">
    {#each tools as tool (tool.path)}
      {@const Icon = tool.icon}
      <button class="card tile" onclick={() => openClassic(tool.path, apiBase())}>
        <div class="icon"><Icon size={20} /></div>
        <div class="text">
          <strong>{tool.title}</strong>
          <span>{tool.text}</span>
        </div>
        <ExternalLink size={15} class="ext" />
      </button>
    {/each}
  </div>
</section>

<style>
  .view {
    padding: 28px 32px 40px;
    max-width: 1100px;
  }
  .view-head {
    margin-bottom: 20px;
  }
  h1 {
    font-size: 24px;
  }
  .sub {
    margin: 4px 0 0;
    color: var(--muted);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: 14px;
  }
  .tile {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 16px;
    text-align: left;
    cursor: pointer;
    color: var(--text);
  }
  .tile:hover {
    border-color: var(--accent);
  }
  .icon {
    width: 38px;
    height: 38px;
    border-radius: 9px;
    background: var(--surface-2);
    color: var(--accent);
    display: grid;
    place-items: center;
    flex-shrink: 0;
  }
  .text {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .text span {
    color: var(--muted);
    font-size: 13px;
  }
  .tile :global(.ext) {
    color: var(--muted);
  }
</style>
