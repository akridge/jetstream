// Cloud file operations (copy, move, rename, delete, restore) run by the Rust core: progress
// for the operations tray, a result toast with Undo, and a version counter views watch to
// refresh their listings.

import { app } from './state.svelte.js';
import { cancelCloudOp, newOpId, undoCloudOp } from './platform.js';
import { count } from './format.js';

class CloudOps {
  /** Running operations: { id, label, total, done, skipped, failed, status, shown }. Quick ones
   * never appear (their result toast is enough); the tray shows those running over a moment. */
  list = $state([]);
  /** Bumped when an operation finishes; listings refresh when it changes. */
  version = $state(0);

  /**
   * Start an operation. start(onProgress) must call the platform function that runs it.
   * verb is the past tense for the result ("Moved", "Deleted", ...).
   */
  async start(opId, label, total, verb, start) {
    const op = { id: opId, label, total, done: 0, skipped: 0, failed: 0, status: 'running', shown: false };
    this.list = [...this.list, op];
    const entry = () => this.list.find((o) => o.id === opId);
    setTimeout(() => entry() && (entry().shown = true), 700);
    try {
      const result = await start((tick) => Object.assign(entry() || {}, tick));
      Object.assign(entry() || {}, {
        done: result.done,
        skipped: result.skipped,
        failed: result.failed,
        status: result.cancelled ? 'cancelled' : result.failed ? 'failed' : 'done',
      });
      this.#report(opId, label, verb, result);
      return result;
    } catch (err) {
      Object.assign(entry() || {}, { status: 'failed' });
      app.toast(`${label}: ${err?.message || err}`, 'error', { timeout: 10000 });
      return null;
    } finally {
      this.version++;
      this.list = this.list.filter((o) => o.id !== opId);
    }
  }

  #report(opId, label, verb, result) {
    const parts = [`${verb} ${count(result.done)} ${result.done === 1 ? 'file' : 'files'}`];
    if (result.skipped) parts.push(`${count(result.skipped)} left as they were (already there)`);
    if (result.failed) parts.push(`${count(result.failed)} failed`);
    if (result.cancelled) parts.push('stopped early');
    let message = parts.join(' · ');
    if (result.failures?.length) message += `\n${result.failures[0].path}: ${result.failures[0].error}`;
    const kind = result.failed ? 'error' : result.cancelled ? 'info' : 'success';
    const action = result.undoable
      ? {
          label: 'Undo',
          run: () => {
            const id = newOpId();
            this.start(id, `Undo: ${label}`, result.done, 'Undid', (onProgress) => undoCloudOp(opId, id, onProgress));
          },
        }
      : null;
    app.toast(message, kind, { action, timeout: result.undoable ? 15000 : result.failed ? 12000 : 5000 });
  }

  cancel(opId) {
    cancelCloudOp(opId);
  }
}

export const cloudOps = new CloudOps();
