// What the Cloud storage preview pane can show for a file, and a small CSV reader for the
// first 64 KB of a table.

const TEXT_EXT = new Set([
  'txt', 'text', 'csv', 'tsv', 'json', 'geojson', 'jsonl', 'ndjson', 'xml', 'kml', 'gpx', 'md', 'markdown', 'log', 'yaml', 'yml',
  'ini', 'cfg', 'conf', 'toml', 'py', 'r', 'm', 'jl', 'sh', 'bat', 'ps1', 'js', 'ts', 'html', 'htm', 'css', 'sql', 'asc', 'hdr', 'prj',
  'meta', 'nmea', 'readme', 'cnv', 'ctd', 'btl', 'dat',
]);
const IMAGE_EXT = /\.(jpe?g|jpe|png|gif|webp|bmp|tiff?)$/i;

const ext = (name) => {
  const base = name.split('/').pop() || '';
  const dot = base.lastIndexOf('.');
  return dot > 0 ? base.slice(dot + 1).toLowerCase() : base.toLowerCase() === 'readme' ? 'readme' : '';
};

/** "image", "table" (CSV/TSV), "text", or null (details only). */
export function previewKind(entry) {
  if (!entry || entry.is_folder) return null;
  const type = (entry.content_type || '').toLowerCase();
  const e = ext(entry.name);
  if ((type.startsWith('image/') && !/svg|heic|heif/.test(type)) || IMAGE_EXT.test(entry.name)) return 'image';
  if (e === 'csv' || e === 'tsv' || type === 'text/csv' || type === 'text/tab-separated-values') return 'table';
  if (TEXT_EXT.has(e) || type.startsWith('text/') || /json|xml|yaml|javascript/.test(type)) return 'text';
  return null;
}

/** The delimiter a table uses: tab for .tsv, else whichever of , ; tab is most common in its first line. */
export function sniffDelimiter(text, name = '') {
  if (ext(name) === 'tsv') return '\t';
  const first = text.slice(0, text.indexOf('\n') < 0 ? text.length : text.indexOf('\n'));
  let best = ',';
  let most = -1;
  for (const d of [',', ';', '\t', '|']) {
    const n = first.split(d).length - 1;
    if (n > most) [best, most] = [d, n];
  }
  return best;
}

/**
 * Rows of a CSV (quotes, "" escapes and line breaks inside quotes). With `truncated` (the text
 * is only the start of the file), the last row is dropped: it may be cut off.
 * Returns { rows, more } where `more` means rows past `maxRows` (or the cut) exist.
 */
export function parseCsv(text, { delimiter = ',', truncated = false, maxRows = 200 } = {}) {
  const rows = [];
  let row = [];
  let field = '';
  let quoted = false;
  let i = 0;
  const endRow = () => {
    row.push(field);
    rows.push(row);
    row = [];
    field = '';
  };
  while (i < text.length) {
    const c = text[i];
    if (quoted) {
      if (c === '"') {
        if (text[i + 1] === '"') {
          field += '"';
          i++;
        } else quoted = false;
      } else field += c;
    } else if (c === '"' && field === '') quoted = true;
    else if (c === delimiter) {
      row.push(field);
      field = '';
    } else if (c === '\n' || c === '\r') {
      if (c === '\r' && text[i + 1] === '\n') i++;
      endRow();
      if (rows.length > maxRows) break;
    } else field += c;
    i++;
  }
  const atEnd = i >= text.length;
  // A last row without a line break: complete only if the whole file is here.
  if (atEnd && (field !== '' || row.length)) {
    if (truncated) row = [];
    else endRow();
  }
  let more = truncated || rows.length > maxRows || !atEnd;
  if (rows.length > maxRows) rows.length = maxRows;
  if (truncated && atEnd && quoted) more = true;
  return { rows, more };
}

/** Pretty-print complete JSON; anything else (or cut-off JSON) as it is. */
export function prettyText(text, entry, truncated) {
  const e = ext(entry.name);
  if (!truncated && (e === 'json' || e === 'geojson' || /json/.test(entry.content_type || ''))) {
    try {
      return JSON.stringify(JSON.parse(text), null, 2);
    } catch {
      /* not valid JSON: show as is */
    }
  }
  return text;
}

/** The pane's details from Cloud Storage's object resource. */
export function objectFacts(raw) {
  if (!raw) return null;
  const meta = raw.metadata || {};
  const secs = (v) => (v && /^-?\d+$/.test(String(v).trim()) ? Number(v) * 1000 : null);
  const custom = Object.entries(meta).filter(([k]) => k !== 'goog-reserved-file-mtime' && k !== 'jetstream-file-created');
  return {
    type: raw.contentType || '',
    size: Number(raw.size || 0),
    storageClass: raw.storageClass || '',
    created: secs(meta['jetstream-file-created']),
    modified: secs(meta['goog-reserved-file-mtime']),
    uploaded: raw.timeCreated || null,
    // Details edited after the upload (e.g. by Fix file details); not the same moment.
    updated: raw.updated && raw.timeCreated && Date.parse(raw.updated) - Date.parse(raw.timeCreated) > 60_000 ? raw.updated : null,
    generation: raw.generation || '',
    crc32c: raw.crc32c || '',
    md5: raw.md5Hash || '',
    composite: Number(raw.componentCount || 0) > 0,
    encoding: raw.contentEncoding || '',
    cacheControl: raw.cacheControl || '',
    custom,
  };
}
