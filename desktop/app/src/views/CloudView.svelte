<script>
  import { onMount, tick } from 'svelte';
  import { SvelteSet } from 'svelte/reactivity';
  import Folder from '@lucide/svelte/icons/folder';
  import File from '@lucide/svelte/icons/file';
  import FileImage from '@lucide/svelte/icons/file-image';
  import FileVideo from '@lucide/svelte/icons/file-video-camera';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import RefreshCw from '@lucide/svelte/icons/refresh-cw';
  import Copy from '@lucide/svelte/icons/copy';
  import Link from '@lucide/svelte/icons/link';
  import ExternalLink from '@lucide/svelte/icons/external-link';
  import ArrowRightLeft from '@lucide/svelte/icons/arrow-right-left';
  import FolderSync from '@lucide/svelte/icons/folder-sync';
  import FolderPlus from '@lucide/svelte/icons/folder-plus';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import Pencil from '@lucide/svelte/icons/pencil';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import History from '@lucide/svelte/icons/history';
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw';
  import Sigma from '@lucide/svelte/icons/sigma';
  import Cloud from '@lucide/svelte/icons/cloud';
  import Lock from '@lucide/svelte/icons/lock';
  import X from '@lucide/svelte/icons/x';
  import GitCompare from '@lucide/svelte/icons/git-compare-arrows';
  import Laptop from '@lucide/svelte/icons/laptop';
  import CloudOpDialog from '../components/CloudOpDialog.svelte';
  import UploadDropDialog from '../components/UploadDropDialog.svelte';
  import Download from '@lucide/svelte/icons/download';
  import Upload from '@lucide/svelte/icons/upload';
  import FileOpDialog from '../components/FileOpDialog.svelte';
  import ContextMenu from '../components/ContextMenu.svelte';
  import { app } from '../lib/state.svelte.js';
  import { cloudOps } from '../lib/cloudops.svelte.js';
  import { bytes, count, relativeTime, dateTime } from '../lib/format.js';
  import {
    bucketAccess,
    folderTotals,
    isDesktop,
    listDeleted,
    listFolder,
    newFolder,
    newOpId,
    downloadCloud,
    openUrl,
    pickFolder,
    planCloudOp,
    revealPath,
    restoreObjects,
    runCloudOp,
  } from '../lib/platform.js';

  const QUICK_BUCKETS = ['nmfs_odp_pifsc', 'nmfs_odp_afsc', 'nmfs_odp_swfsc', 'nmfs_odp_sefsc', 'nmfs_odp_nefsc', 'nmfs_odp_nwfsc'];

  let location = $state('');
  let bucket = $state('');
  let prefix = $state('');
  let entries = $state([]);
  let nextToken = $state(null);
  let loading = $state(false);
  let error = $state('');
  let access = $state(null);
  let totals = $state(null);
  let totalsFor = $state('');
  let counting = $state(false);
  let syncDialog = $state(null);
  let fileOp = $state(null);
  let menu = $state(null);

  // Selection (Explorer-style: click, Ctrl/Cmd+click, Shift+click, arrow keys).
  const selected = new SvelteSet();
  let anchor = $state(-1);
  let focusIndex = $state(-1);
  let table = $state();

  // Inline editing.
  let renaming = $state(null);
  let renameValue = $state('');
  let renameBusy = $state(false);
  let creating = $state(false);
  let newName = $state('');
  let inlineError = $state('');

  // Recently deleted (soft delete).
  let trash = $state(null);
  const trashSelected = new SvelteSet();

  let crumbs = $derived(prefix.split('/').filter(Boolean));
  let shortcuts = $derived(app.prefs.recent_buckets?.length ? app.prefs.recent_buckets : QUICK_BUCKETS);
  let selectedEntries = $derived(entries.filter((e) => selected.has(e.path)));
  let single = $derived(selectedEntries.length === 1 ? selectedEntries[0] : null);
  let keepsDeleted = $derived(!access?.known || access.soft_delete_seconds > 0);
  let syncedFrom = $derived(bucket ? app.presetForCloudPath(bucket, prefix) : null);
  let readOnly = $derived(!!access?.permissions && !can('storage.objects.create') && !can('storage.objects.delete'));

  /** Permission check; unknown permissions count as allowed (the call itself will say no). */
  function can(permission) {
    return !access?.permissions || access.permissions.includes(permission);
  }
  const noPermission = (permission) => (can(permission) ? '' : "You don't have permission to do this in this bucket");

  function parse(value) {
    const clean = value.trim().replace(/^gs:\/\//, '').replace(/^\/+/, '');
    const [b, ...rest] = clean.split('/');
    const p = rest.join('/');
    return { bucket: b, prefix: p && !p.endsWith('/') ? `${p}/` : p };
  }

  async function open(b, p = '', { remember = true, keepSelection = false } = {}) {
    if (!b) return;
    if (b !== bucket) access = await bucketAccess(b).catch(() => null);
    bucket = b;
    prefix = p;
    location = `gs://${b}/${p}`;
    app.cloudLocation = { bucket: b, prefix: p };
    if (!keepSelection) clearSelection();
    totals = null;
    error = '';
    trash = null;
    renaming = null;
    creating = false;
    loading = true;
    try {
      const listing = await listFolder(b, p, null, !!access?.hierarchical_namespace);
      entries = listing.entries;
      nextToken = listing.next_page_token;
      if (remember) app.remember('recent_buckets', b);
      for (const path of [...selected]) if (!entries.some((e) => e.path === path)) selected.delete(path);
    } catch (err) {
      entries = [];
      nextToken = null;
      error = String(err?.message || err);
    } finally {
      loading = false;
    }
  }

  // Come back to where you were after visiting another view.
  onMount(() => {
    if (isDesktop && app.cloudLocation) open(app.cloudLocation.bucket, app.cloudLocation.prefix, { remember: false });
  });

  const refresh = (opts = {}) => open(bucket, prefix, { remember: false, ...opts });

  // Refresh after a copy/move/delete finishes.
  let seenVersion = cloudOps.version;
  $effect(() => {
    if (cloudOps.version !== seenVersion) {
      seenVersion = cloudOps.version;
      if (bucket) (trash ? loadTrash() : refresh());
    }
  });

  async function loadMore() {
    loading = true;
    try {
      const listing = await listFolder(bucket, prefix, nextToken, !!access?.hierarchical_namespace);
      entries = [...entries, ...listing.entries];
      nextToken = listing.next_page_token;
    } catch (err) {
      error = String(err?.message || err);
    } finally {
      loading = false;
    }
  }

  function go() {
    const { bucket: b, prefix: p } = parse(location);
    open(b, p);
  }

  function goUp() {
    if (!crumbs.length) return;
    open(bucket, crumbs.length > 1 ? crumbs.slice(0, -1).join('/') + '/' : '', { remember: false });
  }

  const crumbPath = (index) => crumbs.slice(0, index + 1).join('/') + '/';
  const displayName = (entry) => entry.name.replace(/\/$/, '');
  const gsPath = (path) => `gs://${bucket}/${path}`;

  // ----- selection -------------------------------------------------------------------------

  function clearSelection() {
    selected.clear();
    anchor = -1;
    focusIndex = -1;
  }

  function selectOnly(index) {
    selected.clear();
    if (entries[index]) selected.add(entries[index].path);
    anchor = index;
    focusIndex = index;
  }

  function selectRange(to) {
    const from = anchor < 0 ? to : anchor;
    selected.clear();
    for (let i = Math.min(from, to); i <= Math.max(from, to); i++) selected.add(entries[i].path);
    focusIndex = to;
  }

  function rowClick(e, index) {
    if (e.shiftKey) selectRange(index);
    else if (e.ctrlKey || e.metaKey) {
      const path = entries[index].path;
      selected.has(path) ? selected.delete(path) : selected.add(path);
      anchor = index;
      focusIndex = index;
    } else selectOnly(index);
  }

  function rowMenu(e, index) {
    e.preventDefault();
    e.stopPropagation();
    if (!selected.has(entries[index].path)) selectOnly(index);
    menu = { x: e.clientX, y: e.clientY, items: itemMenu() };
  }

  function backgroundMenu(e) {
    e.preventDefault();
    e.stopPropagation();
    clearSelection();
    menu = { x: e.clientX, y: e.clientY, items: folderMenu() };
  }

  async function focusRow(index) {
    focusIndex = index;
    await tick();
    table?.querySelector(`[data-index="${index}"]`)?.scrollIntoView({ block: 'nearest' });
  }

  function keydown(e) {
    if (e.target.tagName === 'INPUT' || trash) return;
    const mod = e.ctrlKey || e.metaKey;
    const last = entries.length - 1;
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const next = Math.max(0, Math.min(last, focusIndex + (e.key === 'ArrowDown' ? 1 : -1)));
      e.shiftKey ? selectRange(next) : selectOnly(next);
      focusRow(next);
    } else if (e.key === 'Home' || e.key === 'End') {
      e.preventDefault();
      const next = e.key === 'Home' ? 0 : last;
      e.shiftKey ? selectRange(next) : selectOnly(next);
      focusRow(next);
    } else if (e.key === 'Enter' && single?.is_folder) {
      open(bucket, single.path, { remember: false });
    } else if (e.key === 'Backspace') {
      e.preventDefault();
      goUp();
    } else if (e.key === 'F2' && single) {
      e.preventDefault();
      startRename(single);
    } else if (e.key === 'Delete' && selectedEntries.length) {
      e.preventDefault();
      askDelete();
    } else if (mod && e.key.toLowerCase() === 'a') {
      e.preventDefault();
      entries.forEach((entry) => selected.add(entry.path));
    } else if (mod && e.key.toLowerCase() === 'c' && selectedEntries.length) {
      e.preventDefault();
      copyPaths();
    } else if (mod && e.shiftKey && e.key.toLowerCase() === 'n') {
      e.preventDefault();
      startNewFolder();
    } else if (e.key === 'Escape') {
      clearSelection();
    }
  }

  // ----- menus -----------------------------------------------------------------------------

  function itemMenu() {
    const n = selectedEntries.length;
    const one = n === 1 ? selectedEntries[0] : null;
    return [
      ...(one?.is_folder ? [{ label: 'Open', icon: FolderOpen, shortcut: 'Enter', run: () => open(bucket, one.path, { remember: false }) }] : []),
      { label: n > 1 ? `Copy ${n} paths` : 'Copy path', icon: Copy, shortcut: 'Ctrl+C', run: copyPaths },
      { label: n > 1 ? `Download ${n} items…` : 'Download…', icon: Download, disabled: !can('storage.objects.get'), run: () => download() },
      ...(one && !one.is_folder ? [{ label: 'Copy public link', icon: Link, run: () => copyText(publicUrl(one.path), 'Link copied') }] : []),
      ...(one ? [{ label: 'Open in Cloud Console', icon: ExternalLink, run: () => openUrl(consoleUrl(one)) }] : []),
      'separator',
      ...(one
        ? [{ label: 'Rename', icon: Pencil, shortcut: 'F2', disabled: !can('storage.objects.delete'), title: noPermission('storage.objects.delete'), run: () => startRename(one) }]
        : []),
      { label: 'Move or copy to…', icon: ArrowRightLeft, disabled: !can('storage.objects.get'), run: () => (fileOp = { kind: 'transfer', paths: selectedPaths() }) },
      ...(one?.is_folder ? [{ label: 'Sync to… (gcloud rsync)', icon: FolderSync, run: () => (syncDialog = itemFor(one)) }] : []),
      ...(one?.is_folder ? [{ label: 'Total size', icon: Sigma, run: () => computeTotals(one.path) }] : []),
      'separator',
      {
        label: n > 1 ? `Delete ${n} items` : 'Delete',
        icon: Trash2,
        shortcut: 'Del',
        danger: true,
        disabled: !can('storage.objects.delete'),
        title: noPermission('storage.objects.delete'),
        run: askDelete,
      },
    ];
  }

  function folderMenu() {
    return [
      { label: 'New folder', icon: FolderPlus, shortcut: 'Ctrl+Shift+N', disabled: !can('storage.objects.create'), title: noPermission('storage.objects.create'), run: startNewFolder },
      { label: 'Upload a folder here…', icon: Upload, disabled: !can('storage.objects.create'), run: pickFilesToUpload },
      { label: 'Refresh', icon: RefreshCw, run: () => refresh() },
      'separator',
      { label: 'Copy path of this folder', icon: Copy, run: () => copyText(gsPath(prefix), 'Path copied') },
      { label: 'Total size of this folder', icon: Sigma, run: () => computeTotals(prefix) },
      ...(keepsDeleted ? [{ label: 'Recently deleted', icon: History, run: loadTrash }] : []),
    ];
  }

  const selectedPaths = () => selectedEntries.map((e) => e.path);
  const itemFor = (entry) => ({ bucket, path: entry.path, isFolder: entry.is_folder, name: displayName(entry) });

  function copyText(text, message) {
    navigator.clipboard?.writeText(text);
    app.toast(message, 'info', { timeout: 2000 });
  }

  function copyPaths() {
    const paths = selectedEntries.length ? selectedPaths() : [prefix];
    copyText(paths.map(gsPath).join('\n'), paths.length > 1 ? `${paths.length} paths copied` : 'Path copied');
  }

  const encodePath = (path) => path.split('/').map(encodeURIComponent).join('/');
  const publicUrl = (path) => `https://storage.googleapis.com/${bucket}/${encodePath(path)}`;
  const consoleUrl = (entry) =>
    entry.is_folder
      ? `https://console.cloud.google.com/storage/browser/${bucket}/${encodePath(entry.path)}`
      : `https://console.cloud.google.com/storage/browser/_details/${bucket}/${encodePath(entry.path)}`;

  async function computeTotals(path) {
    counting = true;
    totals = null;
    totalsFor = gsPath(path);
    try {
      totals = await folderTotals(bucket, path);
    } catch (err) {
      app.toast(String(err?.message || err), 'error');
    } finally {
      counting = false;
    }
  }

  // ----- rename / new folder / delete -------------------------------------------------------

  async function startRename(entry) {
    if (!can('storage.objects.delete')) return;
    inlineError = '';
    renaming = entry.path;
    renameValue = displayName(entry);
    await tick();
    const input = table?.querySelector('input.rename');
    if (input) {
      input.focus();
      // Select the name without its extension, as Explorer does.
      const dot = entry.is_folder ? -1 : renameValue.lastIndexOf('.');
      input.setSelectionRange(0, dot > 0 ? dot : renameValue.length);
    }
  }

  async function commitRename(entry) {
    const value = renameValue.trim();
    if (!value || value === displayName(entry)) return (renaming = null);
    renameBusy = true;
    inlineError = '';
    try {
      const opId = newOpId();
      const summary = await planCloudOp(opId, { kind: 'rename', bucket, path: entry.path, new_name: value });
      if (summary.conflicts) throw new Error(`Something named “${value}” already exists here`);
      renaming = null;
      const warnings = summary.held || summary.early_deletion.length || summary.already_copied;
      if (entry.is_folder || warnings) {
        // Folders can hold thousands of files: show what it involves first.
        fileOp = { kind: 'rename', paths: [entry.path], newName: value, planned: { opId, summary } };
      } else {
        cloudOps.start(opId, `Rename to ${value}`, summary.objects, 'Renamed', (onProgress) => runCloudOp(opId, onProgress));
      }
    } catch (err) {
      inlineError = String(err?.message || err);
    } finally {
      renameBusy = false;
    }
  }

  function renameKey(e, entry) {
    if (e.key === 'Enter') commitRename(entry);
    else if (e.key === 'Escape') {
      e.stopPropagation();
      renaming = null;
      inlineError = '';
      table?.focus();
    }
  }

  async function startNewFolder() {
    if (!bucket || !can('storage.objects.create')) return;
    clearSelection();
    inlineError = '';
    creating = true;
    newName = 'New folder';
    await tick();
    table?.querySelector('input.new-folder')?.select();
  }

  async function commitNewFolder() {
    const name = newName.trim();
    if (!name) return (creating = false);
    renameBusy = true;
    inlineError = '';
    try {
      const path = await newFolder(bucket, prefix, name);
      creating = false;
      await refresh();
      const index = entries.findIndex((e) => e.path === path);
      if (index >= 0) {
        selectOnly(index);
        focusRow(index);
      }
    } catch (err) {
      inlineError = String(err?.message || err);
    } finally {
      renameBusy = false;
    }
  }

  async function download(paths = selectedPaths()) {
    if (!paths.length) return;
    const dest = await pickFolder(app.prefs.last_download_dir || '', 'Choose where to save');
    if (!dest) return;
    app.savePrefs({ last_download_dir: dest });
    const name = paths.length === 1 ? `“${paths[0].replace(/\/$/, '').split('/').pop()}”` : `${count(paths.length)} items`;
    const id = newOpId();
    cloudOps.start(id, `Download ${name}`, 0, 'Downloaded', (onProgress) => downloadCloud(id, bucket, paths, dest, onProgress), {
      action: { label: 'Show folder', run: () => revealPath(dest) },
    });
  }

  async function pickFilesToUpload() {
    // The folder picker is the only native picker wired up; files can be dropped onto the window.
    const folder = await pickFolder('', 'Choose a folder to upload here');
    if (folder) app.cloudDrop = [folder];
  }

  function askDelete() {
    if (!selectedEntries.length || !can('storage.objects.delete')) return;
    fileOp = { kind: 'delete', paths: selectedPaths() };
  }

  // ----- recently deleted ------------------------------------------------------------------

  async function loadTrash(more = false) {
    trashSelected.clear();
    const previous = more ? trash : null;
    trash = { objects: previous?.objects || [], next: null, loading: true, error: '' };
    try {
      const page = await listDeleted(bucket, prefix, more ? previous.next : null);
      trash = { objects: [...(previous?.objects || []), ...page.objects], next: page.next_page_token, loading: false, error: '' };
    } catch (err) {
      trash = { objects: [], next: null, loading: false, error: String(err?.message || err) };
    }
  }

  const trashKey = (o) => `${o.name}#${o.generation}`;

  function restoreSelected() {
    const objects = trash.objects.filter((o) => trashSelected.has(trashKey(o))).map((o) => ({ name: o.name, generation: o.generation }));
    if (!objects.length) return;
    const opId = newOpId();
    cloudOps.start(opId, `Restore ${count(objects.length)} ${objects.length === 1 ? 'file' : 'files'}`, objects.length, 'Restored', (onProgress) =>
      restoreObjects(opId, bucket, objects, onProgress),
    );
  }

  function iconFor(entry) {
    if (entry.is_folder) return Folder;
    const type = entry.content_type || '';
    if (type.startsWith('image/') || /\.(jpe?g|png|tiff?|dng|cr2|nef)$/i.test(entry.name)) return FileImage;
    if (type.startsWith('video/') || /\.(mp4|mov|avi|mkv)$/i.test(entry.name)) return FileVideo;
    return File;
  }
</script>

<section class="view">
  <header class="view-head">
    <div>
      <h1>Cloud storage</h1>
      <p class="sub">Browse buckets and manage files: rename, move, copy, delete and restore. Right-click for more.</p>
    </div>
  </header>

  <form class="locationbar" onsubmit={(e) => { e.preventDefault(); go(); }}>
    <Cloud size={17} />
    <input class="mono" bind:value={location} placeholder="gs://bucket/folder/" aria-label="Cloud location" />
    <button class="btn primary sm" type="submit" disabled={!location.trim() || !isDesktop}>Open</button>
  </form>
  <div class="shortcuts">
    {#each shortcuts as b (b)}
      <button class="chip" onclick={() => open(b)}>{b}</button>
    {/each}
  </div>

  {#if !isDesktop}
    <div class="card notice">Browsing buckets runs in the JetStream desktop app.</div>
  {:else if bucket}
    <div class="card browser">
      <div class="toolbar">
        <nav class="crumbs" aria-label="Folder path">
          <button onclick={() => open(bucket, '')}>{bucket}</button>
          {#each crumbs as crumb, i (i)}
            <ChevronRight size={14} />
            <button onclick={() => open(bucket, crumbPath(i), { remember: false })}>{crumb}</button>
          {/each}
          {#if readOnly}<span class="pill" title="You can view this bucket but not change it"><Lock size={11} /> read-only</span>{/if}
          {#if access?.hierarchical_namespace}<span class="pill" title="Hierarchical namespace: folders are real and rename instantly">folders</span>{/if}
        </nav>
        <div class="actions">
          {#if trash}
            <button class="btn sm" onclick={() => refresh()}><X size={14} /> Close recently deleted</button>
          {:else if selectedEntries.length}
            <span class="count">{count(selectedEntries.length)} selected</span>
            <button class="btn ghost sm icon" title="Copy path (Ctrl+C)" aria-label="Copy path" onclick={copyPaths}><Copy size={14} /></button>
            {#if single}
              <button class="btn ghost sm" disabled={!can('storage.objects.delete')} title={noPermission('storage.objects.delete') || 'Rename (F2)'} onclick={() => startRename(single)}><Pencil size={14} /> Rename</button>
            {/if}
            <button class="btn ghost sm" title="Download to this computer" onclick={() => download()}><Download size={14} /> Download</button>
            <button class="btn ghost sm" onclick={() => (fileOp = { kind: 'transfer', paths: selectedPaths() })}><ArrowRightLeft size={14} /> Move/copy…</button>
            {#if single?.is_folder}
              <button class="btn ghost sm" onclick={() => (syncDialog = itemFor(single))}><FolderSync size={14} /> Sync to…</button>
            {/if}
            <button class="btn ghost sm danger" disabled={!can('storage.objects.delete')} title={noPermission('storage.objects.delete') || 'Delete (Del)'} onclick={askDelete}><Trash2 size={14} /> Delete</button>
            <button class="btn ghost sm icon" title="Clear selection (Esc)" aria-label="Clear selection" onclick={clearSelection}><X size={14} /></button>
            <span class="divider"></span>
            <button class="btn ghost sm icon" disabled={!can('storage.objects.create')} title={noPermission('storage.objects.create') || 'New folder (Ctrl+Shift+N)'} aria-label="New folder" onclick={startNewFolder}><FolderPlus size={14} /></button>
          {:else}
            <button class="btn ghost sm" disabled={!can('storage.objects.create')} title={noPermission('storage.objects.create') || 'New folder (Ctrl+Shift+N)'} onclick={startNewFolder}><FolderPlus size={14} /> New folder</button>
            <button class="btn ghost sm" disabled={counting} onclick={() => computeTotals(prefix)}><Sigma size={14} /> Size</button>
            {#if keepsDeleted}
              <button class="btn ghost sm" title="Files deleted from this folder that can still be restored" onclick={() => loadTrash()}><History size={14} /> Recently deleted</button>
            {/if}
          {/if}
          <button class="btn ghost sm icon" aria-label="Refresh" title="Refresh" onclick={() => (trash ? loadTrash() : refresh())}><RefreshCw size={14} class={loading ? 'spin' : ''} /></button>
        </div>
      </div>

      {#if syncedFrom && !trash}
        <div class="synced">
          <Laptop size={15} />
          <span class="truncate">
            Synced from <strong>{syncedFrom.name}</strong> on this computer · <span class="mono" title={syncedFrom.source_path}>{syncedFrom.source_path}</span>
          </span>
          <button class="btn ghost sm" onclick={() => app.compareFolder(syncedFrom)}><GitCompare size={14} /> Compare with this computer</button>
        </div>
      {/if}

      {#if counting || totals}
        <div class="totals">
          {#if counting}
            Counting everything in <span class="mono">{totalsFor}</span>…
          {:else}
            <span class="mono truncate">{totalsFor}</span>
            <strong>{bytes(totals.bytes)}</strong>
            <span>{count(totals.objects)}{totals.truncated ? '+' : ''} objects</span>
            {#each Object.entries(totals.by_storage_class) as [cls, b] (cls)}
              <span class="pill">{cls.toLowerCase()} · {bytes(b)}</span>
            {/each}
            {#if totals.newest}<span class="muted">updated {relativeTime(totals.newest)}</span>{/if}
            <button class="btn ghost sm icon" aria-label="Hide" onclick={() => (totals = null)}><X size={13} /></button>
          {/if}
        </div>
      {/if}

      {#if error}
        <div class="error">
          {error}
          {#if /sign|auth|credential|401|403/i.test(error)}
            <button class="btn sm" onclick={() => (app.view = 'settings')}>Check sign-in</button>
          {/if}
        </div>
      {/if}
      {#if inlineError}
        <div class="error">{inlineError}<button class="btn ghost sm icon" aria-label="Dismiss" onclick={() => (inlineError = '')}><X size={13} /></button></div>
      {/if}

      {#if trash}
        <div class="trash-head">
          <div>
            <strong>Recently deleted</strong> in <span class="mono">{gsPath(prefix)}</span>
            <p class="muted">
              Kept for {access?.soft_delete_seconds ? Math.round(access.soft_delete_seconds / 86400) : 'a few'} days after deletion, then removed for good. Restoring never replaces a file that exists now.
            </p>
          </div>
          <button class="btn primary sm" disabled={!trashSelected.size || !can('storage.objects.restore')} onclick={restoreSelected}>
            <RotateCcw size={14} /> Restore{trashSelected.size ? ` ${count(trashSelected.size)}` : ''}
          </button>
        </div>
        <div class="table trash" role="table">
          <div class="thead" role="row">
            <span role="columnheader">Name</span>
            <span role="columnheader" class="num">Size</span>
            <span role="columnheader">Deleted</span>
            <span role="columnheader">Gone for good</span>
          </div>
          {#each trash.objects as o (trashKey(o))}
            <label class="tr" class:selected={trashSelected.has(trashKey(o))} role="row">
              <span class="name" role="cell">
                <input type="checkbox" checked={trashSelected.has(trashKey(o))} onchange={(e) => (e.currentTarget.checked ? trashSelected.add(trashKey(o)) : trashSelected.delete(trashKey(o)))} />
                <span class="truncate" title={o.name}>{o.name.slice(prefix.length)}</span>
              </span>
              <span class="num" role="cell">{bytes(o.size)}</span>
              <span class="muted" role="cell" title={dateTime(o.soft_delete_time)}>{relativeTime(o.soft_delete_time)}</span>
              <span class="muted" role="cell">{o.hard_delete_time ? new Date(o.hard_delete_time).toLocaleDateString() : ''}</span>
            </label>
          {:else}
            <div class="empty">
              {#if trash.loading}Loading…{:else if trash.error}<span class="bad">{trash.error}</span>{:else}Nothing deleted here recently.{/if}
            </div>
          {/each}
        </div>
        {#if trash.next}
          <div class="more"><button class="btn sm" disabled={trash.loading} onclick={() => loadTrash(true)}>Load more</button></div>
        {/if}
      {:else}
        <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
        <div class="table" role="grid" aria-multiselectable="true" tabindex="0" bind:this={table} onkeydown={keydown} oncontextmenu={backgroundMenu}>
          <div class="thead" role="row">
            <span role="columnheader">Name</span>
            <span role="columnheader" class="num">Size</span>
            <span role="columnheader">Updated</span>
            <span role="columnheader">Class</span>
          </div>
          {#if creating}
            <div class="tr editing" role="row">
              <span class="name" role="cell">
                <FolderPlus size={16} color="var(--accent)" />
                <input
                  class="input mono new-folder"
                  aria-label="New folder name"
                  bind:value={newName}
                  disabled={renameBusy}
                  onkeydown={(e) => { if (e.key === 'Enter') commitNewFolder(); else if (e.key === 'Escape') { e.stopPropagation(); creating = false; } }}
                  onblur={() => !renameBusy && commitNewFolder()}
                />
              </span>
            </div>
          {/if}
          {#each entries as entry, index (entry.path)}
            {@const Icon = iconFor(entry)}
            <div
              class="tr"
              class:selected={selected.has(entry.path)}
              class:focused={focusIndex === index}
              role="row"
              aria-selected={selected.has(entry.path)}
              data-index={index}
              onclick={(e) => rowClick(e, index)}
              ondblclick={() => entry.is_folder && open(bucket, entry.path, { remember: false })}
              oncontextmenu={(e) => rowMenu(e, index)}
            >
              <span class="name" role="cell">
                <Icon size={16} color={entry.is_folder ? 'var(--accent)' : 'var(--muted)'} />
                {#if renaming === entry.path}
                  <input
                    class="input mono rename"
                    aria-label="New name"
                    bind:value={renameValue}
                    disabled={renameBusy}
                    onclick={(e) => e.stopPropagation()}
                    onkeydown={(e) => renameKey(e, entry)}
                    onblur={() => !renameBusy && renaming === entry.path && commitRename(entry)}
                  />
                {:else if entry.is_folder}
                  <button class="link" tabindex="-1" onclick={(e) => { e.stopPropagation(); open(bucket, entry.path, { remember: false }); }}>{displayName(entry)}</button>
                {:else}
                  <span class="truncate">{entry.name}</span>
                {/if}
              </span>
              <span class="num" role="cell">{entry.size !== null && entry.size !== undefined ? bytes(entry.size) : ''}</span>
              <span class="muted" role="cell" title={entry.updated ? dateTime(entry.updated) : ''}>{entry.updated ? relativeTime(entry.updated) : ''}</span>
              <span class="muted" role="cell">{entry.storage_class ? entry.storage_class.toLowerCase() : ''}</span>
            </div>
          {:else}
            {#if !loading && !error && !creating}
              <div class="empty">This folder is empty.{#if can('storage.objects.create')} Right-click to create a folder.{/if}</div>
            {/if}
          {/each}
          {#if loading && !entries.length}
            <div class="empty">Loading…</div>
          {/if}
        </div>
        {#if nextToken}
          <div class="more"><button class="btn sm" disabled={loading} onclick={loadMore}>Load more</button></div>
        {/if}
      {/if}
    </div>
    <p class="hints muted">
      Click to select · Ctrl+click or Shift+click for several · Enter opens · Backspace goes up · F2 renames · Del deletes · Ctrl+C copies paths · Drop files or folders here to upload
    </p>
  {:else}
    <div class="card notice">Open a bucket above, or pick one of the shortcuts.</div>
  {/if}
</section>

{#if menu}
  <ContextMenu x={menu.x} y={menu.y} items={menu.items} onclose={() => (menu = null)} />
{/if}
{#if syncDialog}
  <CloudOpDialog item={syncDialog} mode="copy" onclose={() => (syncDialog = null)} />
{/if}
{#if app.cloudDrop && bucket}
  <UploadDropDialog {bucket} {prefix} paths={app.cloudDrop} onclose={() => (app.cloudDrop = null)} />
{/if}
{#if fileOp}
  <FileOpDialog
    kind={fileOp.kind}
    {bucket}
    paths={fileOp.paths}
    here={prefix}
    newName={fileOp.newName}
    planned={fileOp.planned}
    onclose={() => { fileOp = null; table?.focus(); }}
  />
{/if}

<style>
  .view {
    padding: 28px 32px 32px;
    max-width: 1280px;
  }
  .view-head {
    margin-bottom: 18px;
  }
  h1 {
    font-size: 24px;
  }
  .sub {
    margin: 4px 0 0;
    color: var(--muted);
  }
  .locationbar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 6px 6px 12px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--muted);
  }
  .locationbar input {
    flex: 1;
    border: none;
    outline: none;
    background: transparent;
    min-width: 0;
    color: var(--text);
  }
  .shortcuts {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 10px 0 16px;
  }
  .chip {
    height: 26px;
    padding: 0 10px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--surface);
    font-size: 12px;
    color: var(--text-2);
    cursor: pointer;
    font-family: var(--mono);
  }
  .chip:hover {
    border-color: var(--accent);
    color: var(--accent);
  }
  .notice {
    padding: 28px;
    color: var(--muted);
    text-align: center;
  }
  .browser {
    overflow: hidden;
  }
  .toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 12px;
    min-height: 50px;
    border-bottom: 1px solid var(--border);
    flex-wrap: wrap;
  }
  .crumbs {
    display: flex;
    align-items: center;
    gap: 2px;
    color: var(--muted);
    min-width: 0;
    flex-wrap: wrap;
  }
  .crumbs button {
    border: none;
    background: none;
    padding: 4px 6px;
    border-radius: 6px;
    font-weight: 600;
    color: var(--text-2);
    cursor: pointer;
  }
  .crumbs button:hover {
    background: var(--surface-2);
    color: var(--text);
  }
  .crumbs .pill {
    margin-left: 6px;
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-wrap: wrap;
  }
  .divider {
    width: 1px;
    height: 18px;
    margin: 0 4px;
    background: var(--border);
  }
  .count {
    font-size: 13px;
    font-weight: 600;
    color: var(--accent);
    margin-right: 4px;
  }
  .danger {
    color: var(--danger);
  }
  .synced {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 8px 6px 14px;
    background: var(--surface-2);
    border-bottom: 1px solid var(--border);
    font-size: 13px;
    color: var(--text-2);
  }
  .synced .truncate {
    flex: 1;
    min-width: 0;
  }
  .totals {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    padding: 8px 14px;
    background: var(--accent-soft);
    font-size: 13px;
  }
  .totals .mono {
    max-width: 380px;
  }
  .error {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 14px;
    background: var(--danger-soft);
    color: var(--danger);
  }
  .table:focus {
    outline: none;
  }
  .table:focus-visible .tr.focused {
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .thead,
  .tr {
    display: grid;
    grid-template-columns: 1fr 110px 140px 110px;
    gap: 12px;
    align-items: center;
    padding: 0 14px;
  }
  .thead {
    height: 34px;
    color: var(--muted);
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    border-bottom: 1px solid var(--border);
  }
  .tr {
    height: 38px;
    border-bottom: 1px solid var(--border);
    cursor: default;
    user-select: none;
  }
  .tr:hover {
    background: var(--surface-hover);
  }
  .tr.selected {
    background: var(--accent-soft);
  }
  .tr.editing {
    background: var(--surface-2);
  }
  .trash .tr {
    cursor: pointer;
  }
  .name {
    display: flex;
    align-items: center;
    gap: 9px;
    min-width: 0;
  }
  .name input.input {
    height: 28px;
    padding: 0 8px;
    flex: 1;
    max-width: 480px;
  }
  .name input[type='checkbox'] {
    accent-color: var(--accent);
  }
  .link {
    border: none;
    background: none;
    padding: 0;
    font-weight: 600;
    color: var(--text);
    cursor: pointer;
  }
  .link:hover {
    color: var(--accent);
    text-decoration: underline;
  }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .muted {
    color: var(--muted);
    font-size: 13px;
  }
  .bad {
    color: var(--danger);
  }
  .empty {
    padding: 36px;
    text-align: center;
    color: var(--muted);
  }
  .more {
    padding: 12px;
    display: flex;
    justify-content: center;
  }
  .trash-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 12px;
    padding: 12px 14px;
    background: var(--surface-2);
    border-bottom: 1px solid var(--border);
  }
  .trash-head p {
    margin: 2px 0 0;
  }
  .hints {
    margin: 10px 2px 0;
    font-size: 12px;
  }
</style>
