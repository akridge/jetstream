// Release notes for "What's new", read from desktop/CHANGELOG.md (bundled with the app).

/**
 * The `## <version>` section as { version, intro, groups: [{ title, items: [[{ text, bold }]] }] },
 * or null. Items are runs of plain and **bold** text (no HTML, so nothing is injected).
 */
export function releaseNotes(markdown, version) {
  const lines = markdown.split(/\r?\n/);
  const start = lines.findIndex((l) => new RegExp(`^##\\s+v?${version.replace(/\./g, '\\.')}\\s*$`).test(l));
  if (start < 0) return null;
  const notes = { version, intro: '', groups: [] };
  let group = null;
  for (const line of lines.slice(start + 1)) {
    if (/^##\s/.test(line)) break;
    const heading = line.match(/^###\s+(.*)/);
    const item = line.match(/^\s*[-*]\s+(.*)/);
    if (heading) notes.groups.push((group = { title: heading[1].trim(), items: [] }));
    else if (item) {
      if (!group) notes.groups.push((group = { title: '', items: [] }));
      group.items.push(runs(item[1].trim()));
    } else if (line.trim() && !group) notes.intro = `${notes.intro} ${line.trim()}`.trim();
  }
  return notes;
}

/** "a **b** c" -> [{ text: 'a ' }, { text: 'b', bold: true }, { text: ' c' }]; `code` loses its ticks. */
function runs(text) {
  return text
    .replace(/`([^`]*)`/g, '$1')
    .split(/(\*\*[^*]+\*\*)/)
    .filter(Boolean)
    .map((part) => (part.startsWith('**') ? { text: part.slice(2, -2), bold: true } : { text: part }));
}
