export function bytes(n) {
  const value = Number(n) || 0;
  if (value < 1024) return `${value} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = value / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${units[i]}`;
}

export function count(n) {
  return (Number(n) || 0).toLocaleString();
}

export function duration(seconds) {
  const s = Math.max(0, Math.round(Number(seconds) || 0));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

export function relativeTime(iso) {
  if (!iso) return '';
  const then = new Date(iso);
  const diff = (Date.now() - then.getTime()) / 1000;
  if (diff < 45) return 'just now';
  if (diff < 3600) return `${Math.round(diff / 60)} min ago`;
  if (diff < 86400) return `${Math.round(diff / 3600)} h ago`;
  const days = Math.round(diff / 86400);
  if (days === 1) return `yesterday ${then.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`;
  if (days < 7) return `${days} days ago`;
  return then.toLocaleDateString();
}

export function dateTime(iso) {
  return iso ? new Date(iso).toLocaleString() : '—';
}

/** "bucket/path" without gs:// or trailing slashes, for comparing destinations. */
export function normalizeDestination(value) {
  return (value || '').trim().replace(/^gs:\/\//, '').replace(/\/+$/, '');
}

export function jobDestination(job) {
  return normalizeDestination(`${job.destination_bucket}/${job.destination_path || ''}`);
}

export function basename(path) {
  return (path || '').replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path;
}

/** "today 18:30", "tomorrow 09:00", "Mon 09:00" or a date, for upcoming times. */
export function upcoming(iso) {
  if (!iso) return '';
  const then = new Date(iso);
  const time = then.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  const startOfDay = (d) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((startOfDay(then) - startOfDay(new Date())) / 86400000);
  if (then.getTime() <= Date.now()) return 'any moment';
  if (days === 0) return `today ${time}`;
  if (days === 1) return `tomorrow ${time}`;
  if (days < 7) return `${then.toLocaleDateString([], { weekday: 'short' })} ${time}`;
  return `${then.toLocaleDateString()} ${time}`;
}

/** Short description of a folder's schedule. */
export function scheduleText(schedule) {
  if (!schedule || !schedule.mode || schedule.mode === 'off') return '';
  if (schedule.mode === 'hourly') {
    const n = Number(schedule.every_hours) || 1;
    return n === 1 ? 'Every hour' : `Every ${n} hours`;
  }
  const names = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
  const days = Array.isArray(schedule.days) && schedule.days.length && schedule.days.length < 7 ? schedule.days : null;
  const which = !days ? 'Daily' : days.join() === '0,1,2,3,4' ? 'Weekdays' : days.map((d) => names[d]).join(', ');
  return `${which} at ${schedule.time || '18:00'}`;
}
