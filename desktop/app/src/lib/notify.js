// When desktop notifications are wanted (Settings → Appearance & notifications).

/** 'all' | 'problems' | 'off' (older settings had a single on/off switch). */
export function notifyLevel(prefs) {
  return prefs.notify_level || (prefs.notifications === false ? 'off' : 'all');
}

/** Quiet hours { on, from: "22:00", to: "07:00" }; the range can cross midnight. */
export function inQuietHours(quiet, now = new Date()) {
  if (!quiet?.on) return false;
  const mins = (t) => {
    const [h, m] = String(t || '0:0').split(':').map(Number);
    return (h || 0) * 60 + (m || 0);
  };
  const [from, to, at] = [mins(quiet.from), mins(quiet.to), now.getHours() * 60 + now.getMinutes()];
  if (from === to) return false;
  return from < to ? at >= from && at < to : at >= from || at < to;
}

