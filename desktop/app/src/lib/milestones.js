// Milestones worth a small celebration: data and files uploaded with JetStream.

const GB = 1024 ** 3;
export const MILESTONES = [
  ...[1, 10, 100, 1024, 10 * 1024, 100 * 1024].map((gb) => ({ id: `bytes-${gb}`, value: gb * GB, kind: 'bytes', label: gb >= 1024 ? `${gb / 1024} TB` : `${gb} GB` })),
  ...[1_000, 10_000, 100_000, 1_000_000, 10_000_000].map((n) => ({ id: `files-${n}`, value: n, kind: 'files', label: `${n.toLocaleString('en-US')} files` })),
];

/**
 * Milestones reached by `totals` ({ bytes, files }) that `seen` (ids, or null the first time)
 * hasn't marked. The first time, everything already reached is marked without celebrating
 * (someone upgrading shouldn't get a pile of messages). Returns { seen, celebrate } where
 * celebrate is the biggest new one of each kind.
 */
export function checkMilestones(totals, seen) {
  const reached = MILESTONES.filter((m) => (m.kind === 'bytes' ? totals.bytes : totals.files) >= m.value);
  if (!seen) return { seen: reached.map((m) => m.id), celebrate: [] };
  const fresh = reached.filter((m) => !seen.includes(m.id));
  const biggest = ['bytes', 'files'].map((kind) => fresh.filter((m) => m.kind === kind).pop()).filter(Boolean);
  return { seen: [...new Set([...seen, ...fresh.map((m) => m.id)])], celebrate: biggest };
}
