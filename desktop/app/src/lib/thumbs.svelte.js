// Thumbnails for the cloud browser's optional tile view. Only tiles on screen ask for one, at
// most 4 load at a time (the desktop core limits it too), and leaving a folder drops whatever
// was waiting along with the folder's thumbnails in memory (the disk cache covers revisits).

import { thumbnail } from './platform.js';
import { app } from './state.svelte.js';

const AT_ONCE = 4;
const IMAGE_EXT = /\.(jpe?g|jpe|png|gif|webp|bmp|tiff?)$/i;

/** Images JetStream can make thumbnails of (the same rule as the desktop core). */
export function isPreviewable(entry) {
  if (entry.is_folder) return false;
  const type = entry.content_type || '';
  if (type.startsWith('image/') && !/svg|heic|heif/.test(type)) return true;
  return IMAGE_EXT.test(entry.name);
}

/** Download cap for previews, in MiB (Settings). */
export const previewCapMib = () => Number(app.prefs.thumb_max_mib) || 20;

class ThumbLoader {
  /** key -> { loading: true } | { data_url } | { skip, ... } */
  results = $state({});
  #queue = [];
  #active = 0;
  #epoch = 0;

  key(bucket, entry) {
    return `${bucket}/${entry.path}#${entry.generation || ''}`;
  }

  /** Ask for a thumbnail (again, with allowCold or a higher cap, after a click). */
  request(bucket, entry, { allowCold = false, maxMib = previewCapMib(), force = false } = {}) {
    const key = this.key(bucket, entry);
    if (this.results[key] && !force) return;
    this.results = { ...this.results, [key]: { loading: true } };
    this.#queue.push({ key, bucket, entry, allowCold, maxMib, epoch: this.#epoch });
    this.#pump();
  }

  /** A tile scrolled away before its turn: don't fetch it (it asks again if it comes back). */
  cancel(bucket, entry) {
    const key = this.key(bucket, entry);
    const index = this.#queue.findIndex((job) => job.key === key);
    if (index < 0) return;
    this.#queue.splice(index, 1);
    const { [key]: _, ...rest } = this.results;
    this.results = rest;
  }

  /** Leaving the folder (or turning thumbnails off). */
  reset() {
    this.#epoch++;
    this.#queue = [];
    this.results = {};
  }

  #pump() {
    while (this.#active < AT_ONCE && this.#queue.length) {
      const job = this.#queue.shift();
      this.#active++;
      const { entry } = job;
      thumbnail({
        bucket: job.bucket,
        path: entry.path,
        generation: entry.generation || null,
        size: entry.size || 0,
        contentType: entry.content_type || null,
        storageClass: entry.storage_class || null,
        allowCold: job.allowCold,
        maxMib: job.maxMib,
      })
        .catch((err) => ({ skip: 'error', message: String(err?.message || err) }))
        .then((result) => {
          if (job.epoch === this.#epoch) this.results = { ...this.results, [job.key]: result };
        })
        .finally(() => {
          this.#active--;
          this.#pump();
        });
    }
  }
}

export const thumbLoader = new ThumbLoader();
