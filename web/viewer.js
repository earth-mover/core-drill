// UI for the Icechunk metadata viewer. All repo data is untrusted: the DOM is
// built with createElement/textContent only, and every object path is
// checked with `checkKey` before it is fetched.
(function () {
  'use strict';

  const D = window.IcechunkDecoder;
  const PAGE = 50;
  const HEX_PAGE = 4096;

  const $ = (id) => document.getElementById(id);
  const view = $('view');

  const state = {
    decoder: null,
    zstdError: null,
    source: null, // {type: 'served' | 'dir' | 'url' | 'file', label, input?, region?}
    repo: null,
    trail: [],
    renderToken: 0,
  };

  // ─── DOM helpers ────────────────────────────────────────

  function h(tag, props, ...kids) {
    const el = document.createElement(tag);
    if (props) {
      for (const [k, v] of Object.entries(props)) {
        if (v === undefined || v === null || v === false) continue;
        if (k === 'class') el.className = v;
        else if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
        else if (k.includes('-')) el.setAttribute(k, v);
        else el[k] = v;
      }
    }
    append(el, kids);
    return el;
  }

  function append(el, kids) {
    for (const k of kids.flat(Infinity)) {
      if (k === null || k === undefined || k === false) continue;
      el.append(k instanceof Node ? k : document.createTextNode(String(k)));
    }
    return el;
  }

  const errorBox = (e) => h('div', { class: 'error' }, e && e.message ? e.message : String(e));
  const notice = (...kids) => h('div', { class: 'notice' }, ...kids);
  const shortId = (id) => (id && id.length > 12 ? id.slice(0, 8) + '…' : id);
  const field = (t, name) => {
    if (!t || t.k !== 'table') return undefined;
    const f = t.fields.find(([n]) => n === name);
    return f ? f[1] : undefined;
  };
  const scalarText = (v) => (v === undefined ? '' : v.k === 'time' ? `${v.iso}` : v.k === 'str' ? v.v : v.v !== undefined ? String(v.v) : '');

  function fmtBytes(n) {
    if (n === null || n === undefined) return '—';
    if (n < 1024) return `${n} B`;
    const units = ['KiB', 'MiB', 'GiB'];
    let v = n / 1024, u = 0;
    while (v >= 1024 && u < units.length - 1) { v /= 1024; u++; }
    return `${v.toFixed(1)} ${units[u]} (${n.toLocaleString()} B)`;
  }

  // ─── Routing ────────────────────────────────────────────
  //
  // `#<path>?ctx=<snapshot>&at=<selector>&src=<url>&region=<region>`

  function parseHash() {
    const raw = location.hash.slice(1);
    const q = raw.indexOf('?');
    const path = decodeURIComponent(q < 0 ? raw : raw.slice(0, q)) || null;
    const params = new URLSearchParams(q < 0 ? '' : raw.slice(q + 1));
    return { path, ctx: params.get('ctx'), at: params.get('at'), src: params.get('src'), region: params.get('region') };
  }

  function hashFor({ path, ctx, at }) {
    const params = new URLSearchParams();
    if (ctx) params.set('ctx', ctx);
    if (at) params.set('at', at);
    if (state.source && state.source.type === 'url') {
      params.set('src', state.source.input);
      if (state.source.region) params.set('region', state.source.region);
    }
    const qs = params.toString();
    return '#' + path + (qs ? '?' + qs : '');
  }

  function go(route) {
    const hash = hashFor(route);
    if (location.hash === hash) onRoute();
    else location.hash = hash;
  }

  function linkTo(text, path, { ctx, at, title } = {}) {
    const ok = state.repo && D.checkKey(path);
    return h('button', {
      class: 'link', type: 'button', disabled: !ok,
      title: ok ? (title || path) : state.repo ? 'not a valid object path' : 'open the repository to follow links',
      onclick: () => go({ path, ctx, at }),
    }, text);
  }

  async function onRoute() {
    const { path, ctx, at, src, region } = parseHash();
    if (src && !(state.source && state.source.type === 'url' && state.source.input === src && (state.source.region || '') === (region || ''))) {
      $('url-input').value = src;
      $('region-input').value = region || '';
      if (!(await openUrl(src, region || '', false))) return;
    }
    if (!state.repo) {
      if (state.source && state.source.type === 'file') return;
      showOpen(true);
      view.replaceChildren(path ? notice(`Open the repository to view ${path}.`) : '');
      return;
    }
    showOpen(false);
    await renderPath(path || 'repo', ctx, at);
  }

  // ─── Sources ────────────────────────────────────────────

  function showOpen(show) {
    $('open').hidden = !show;
    $('toggle-open').textContent = show ? 'Hide' : 'Open…';
  }

  function setSource(source, repo) {
    state.source = source;
    state.repo = repo;
    state.trail = [];
    $('source-label').textContent = source ? source.label : '';
    renderCrumbs();
  }

  function urlBase(input, region) {
    const s = input.trim();
    const m = /^s3:\/\/([^/]+)\/?(.*)$/.exec(s);
    let base;
    if (m) {
      const host = region ? `${m[1]}.s3.${region}.amazonaws.com` : `${m[1]}.s3.amazonaws.com`;
      base = `https://${host}/${m[2]}`;
    } else {
      base = new URL(s, location.href).href;
    }
    const url = new URL(base);
    if (url.protocol !== 'https:' && url.protocol !== 'http:') throw new Error(`unsupported URL scheme ${url.protocol}`);
    if (!url.pathname.endsWith('/')) url.pathname += '/';
    url.search = '';
    url.hash = '';
    return url.href;
  }

  function httpFetcher(base) {
    return async (rel) => {
      let res;
      try { res = await fetch(base + rel, { mode: 'cors', credentials: 'omit' }); } catch (e) {
        throw new Error(`could not fetch ${base + rel}: ${e.message}. If the repository is on another origin, its server must allow CORS.`);
      }
      if (!res.ok) throw new Error(`${base + rel}: HTTP ${res.status}`);
      return new Uint8Array(await res.arrayBuffer());
    };
  }

  async function openUrl(input, region, navigate = true) {
    let base;
    try { base = urlBase(input, region); } catch (e) { view.replaceChildren(errorBox(e)); return false; }
    await ready;
    setSource({ type: 'url', label: base, input: input.trim(), region: region.trim() }, state.decoder.repo(httpFetcher(base)));
    if (navigate) go({ path: 'repo' });
    return true;
  }

  async function openServed() {
    const base = new URL(served.source, location.href).href;
    const name = typeof served.name === 'string' && served.name ? served.name : base;
    setSource({ type: 'served', label: name }, state.decoder.repo(httpFetcher(base)));
  }

  async function openDirectory(files) {
    const list = [...files];
    if (!list.length) return;
    // The picked directory may contain the repo deeper down; use the
    // shallowest folder holding a `repo` file (or `snapshots/`).
    const rels = list.map((f) => f.webkitRelativePath || f.name);
    let base = null;
    for (const r of rels) {
      const parts = r.split('/');
      const i = parts.lastIndexOf('repo') === parts.length - 1 ? parts.length - 1 : parts.indexOf('snapshots');
      if (i < 0) continue;
      const prefix = parts.slice(0, i).join('/');
      if (base === null || prefix.length < base.length) base = prefix;
    }
    if (base === null) {
      view.replaceChildren(errorBox('No Icechunk repository found in that directory (expected a `repo` file).'));
      return;
    }
    const byPath = new Map();
    list.forEach((f, i) => {
      const r = rels[i];
      if (base === '' || r.startsWith(base + '/')) byPath.set(base === '' ? r : r.slice(base.length + 1), f);
    });
    const fetchBytes = async (rel) => {
      const f = byPath.get(rel);
      if (!f) throw new Error(`${rel}: not in the selected directory`);
      return new Uint8Array(await f.arrayBuffer());
    };
    await ready;
    setSource({ type: 'dir', label: `${base || '(selected directory)'} — local` }, state.decoder.repo(fetchBytes));
    go({ path: 'repo' });
  }

  async function openSingleFile(file) {
    await ready;
    setSource({ type: 'file', label: `${file.name} — single file` }, null);
    if (location.hash) history.replaceState(null, '', location.pathname + location.search);
    showOpen(false);
    const token = ++state.renderToken;
    view.replaceChildren(h('p', { class: 'muted' }, 'Decoding…'));
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      const opened = state.decoder.open(file.name, bytes);
      if (token !== state.renderToken) return;
      view.replaceChildren(...(await fileView(opened, { nodePaths: new Map() })));
    } catch (e) {
      view.replaceChildren(errorBox(e));
    }
  }

  // ─── File view ──────────────────────────────────────────

  function crumbLabel(path) {
    const [dir, rest] = path.split('/');
    const names = { snapshots: 'snapshot', transactions: 'tx log', manifests: 'manifest', chunks: 'chunk', overwritten: 'old repo' };
    return rest === undefined ? dir : `${names[dir] || dir} ${shortId(rest)}`;
  }

  function renderCrumbs(current) {
    const nav = $('crumbs');
    nav.replaceChildren();
    state.trail.forEach((r, i) => {
      if (i) nav.append(h('span', { class: 'sep' }, '›'));
      nav.append(r.path === current
        ? h('span', { class: 'current', title: r.path }, crumbLabel(r.path))
        : linkTo(crumbLabel(r.path), r.path, { ctx: r.ctx, at: r.at, title: r.path }));
    });
  }

  function updateTrail(route) {
    const i = state.trail.findIndex((r) => r.path === route.path);
    if (i >= 0) state.trail[i] = route; else state.trail.push(route);
    if (i >= 0) state.trail.length = i + 1;
    renderCrumbs(route.path);
  }

  async function renderPath(path, ctx, at) {
    const token = ++state.renderToken;
    view.replaceChildren(h('p', { class: 'muted' }, `Loading ${path}…`));
    try {
      const file = await state.repo.open(path);
      if (token !== state.renderToken) return;
      updateTrail({ path, ctx, at });
      let context = ctx;
      if (file.kind === 'Manifest' && !context) {
        try { context = await state.repo.branchTip('main'); } catch (_) { context = null; }
      }
      let nodePaths = new Map();
      const warnings = [];
      try { nodePaths = await state.repo.nodeContext(file, context); } catch (e) {
        warnings.push(notice(`Node IDs are unlabeled: ${e.message}`));
      }
      const parts = await fileView(file, { nodePaths, ctx, context, at });
      if (token !== state.renderToken) return;
      view.replaceChildren(...warnings, ...parts);
      document.title = `${crumbLabel(path)} · Icechunk Metadata Viewer`;
    } catch (e) {
      if (token !== state.renderToken) return;
      updateTrail({ path, ctx, at });
      view.replaceChildren(errorBox(e));
    }
  }

  async function fileView(file, opts) {
    const out = [headerPanel(file)];
    const decoded = file.walk({ maxItems: PAGE, nodePaths: opts.nodePaths, lenient: true });
    const ctx = { file, ...opts };
    try {
      if (file.kind === 'RepoInfo' && file.hasSchema) out.push(repoPanel(file));
      else if (file.kind === 'Snapshot' && file.hasSchema) out.push(await snapshotPanel(file));
      else if (file.kind === 'TransactionLog' && file.hasSchema) out.push(await transactionPanel(file, opts.nodePaths));
      else if (file.kind === 'Manifest' && file.hasSchema) out.push(manifestPanel(file, opts));
    } catch (e) {
      out.push(errorBox(e));
    }
    if (opts.at) out.push(selectedPanel(file, opts));
    out.push(treePanel(decoded, ctx));
    return out;
  }

  function headerPanel(file) {
    const hd = file.header;
    const dl = h('dl', { class: 'summary' });
    const row = (k, v) => dl.append(h('dt', null, k), h('dd', null, v));
    row('path', file.path);
    row('kind', file.kind);
    if (hd) {
      row('spec version', String(hd.spec_version));
      row('file type', `${hd.file_type} (${hd.file_type_name})`);
      row('compression', `${hd.compression} (${hd.compression_name})`);
      row('implementation', hd.implementation);
      row('stored size', fmtBytes(file.storedBytes));
      row('decoded size', fmtBytes(file.body.length));
    } else {
      row('header', 'none (raw bytes)');
      row('size', fmtBytes(file.storedBytes));
    }
    const copy = h('button', { type: 'button', title: 'Same JSON as `core-drill <repo> --output json object <path> -n 0`', onclick: async () => {
      try {
        const nodePaths = state.repo ? await state.repo.nodeContext(file).catch(() => new Map()) : new Map();
        const v = file.walk({ nodePaths });
        const plain = new Map(Object.entries({ path: v.path, kind: v.kind, stored_bytes: BigInt(v.stored_bytes) }));
        plain.set('value', D.toPlain(v.value));
        await navigator.clipboard.writeText(D.stringify(plain));
        copy.textContent = 'Copied';
      } catch (e) {
        copy.textContent = `Copy failed: ${e.message}`;
      }
      setTimeout(() => { copy.textContent = 'Copy JSON'; }, 2000);
    } }, 'Copy JSON');
    return h('div', { class: 'panel' }, h('div', { class: 'row', style: 'justify-content: space-between' }, h('h2', null, file.path), copy), dl);
  }

  // ─── Repo info ──────────────────────────────────────────

  function repoPanel(file) {
    const v = file.walk({ lenient: true }).value;
    const snaps = (field(v, 'snapshots') || { items: [] }).items;
    const ids = snaps.map((s) => (field(s, 'id') || {}).id);
    const refsTable = (name) => {
      const refs = (field(v, name) || { items: [] }).items;
      if (!refs.length) return h('p', { class: 'muted' }, `No ${name}.`);
      return h('div', { class: 'table-wrap' }, h('table', null,
        h('tr', null, h('th', null, 'name'), h('th', null, 'snapshot')),
        refs.map((r) => {
          const idx = field(r, 'snapshot_index');
          const id = ids[idx ? Number(idx.v) : 0];
          return h('tr', null, h('td', { class: 'mono' }, scalarText(field(r, 'name'))),
            h('td', { class: 'mono' }, id ? linkTo(id, `snapshots/${id}`) : `index ${idx ? idx.v : 0} (missing)`));
        })));
    };
    const rows = snaps.map((s, i) => {
      const po = field(s, 'parent_offset');
      const parentIndex = po ? Number(po.v) : 0;
      const at = field(s, 'flushed_at');
      return { i, id: ids[i], parentIndex, parent: parentIndex >= 0 ? ids[parentIndex] : null, at, message: scalarText(field(s, 'message')) };
    }).sort((a, b) => (b.at && a.at ? (b.at.raw > a.at.raw ? 1 : b.at.raw < a.at.raw ? -1 : 0) : 0));
    const body = h('tbody');
    const showRows = (from, to) => {
      for (const r of rows.slice(from, to)) {
        body.append(h('tr', null,
          h('td', { class: 'mono' }, r.at ? r.at.iso || String(r.at.v) : ''),
          h('td', { class: 'mono' }, r.id ? linkTo(r.id, `snapshots/${r.id}`, { title: 'snapshot file' }) : '?'),
          h('td', null, r.id ? linkTo('tx log', `transactions/${r.id}`, { title: 'transaction log: what this commit changed' }) : ''),
          h('td', { class: 'mono' }, r.parentIndex < 0 ? '—' : r.parent ? linkTo(shortId(r.parent), `snapshots/${r.parent}`, { title: r.parent }) : `index ${r.parentIndex}?`),
          h('td', null, r.message)));
      }
    };
    showRows(0, PAGE);
    const more = rows.length > PAGE ? moreButton(rows.length, PAGE, (a, b) => showRows(a, b)) : null;
    return h('div', { class: 'panel' },
      h('h2', null, 'Repository'),
      h('h3', null, 'Branches'), refsTable('branches'),
      h('h3', null, 'Tags'), refsTable('tags'),
      h('h3', null, `Snapshots (${snaps.length}, newest first)`),
      h('div', { class: 'table-wrap' }, h('table', null,
        h('thead', null, h('tr', null, h('th', null, 'flushed at'), h('th', null, 'snapshot'), h('th', null, ''), h('th', null, 'parent'), h('th', null, 'message'))),
        body)),
      more);
  }

  function moreButton(total, shown, load) {
    let n = shown;
    const btn = h('button', { type: 'button', onclick: () => {
      load(n, n + PAGE);
      n = Math.min(total, n + PAGE);
      if (n >= total) btn.remove(); else btn.textContent = `Show more (${n} of ${total})`;
    } }, `Show more (${shown} of ${total})`);
    return btn;
  }

  async function commitInfo(snapshotId) {
    const kv = h('div', { class: 'kv' });
    const add = (k, ...v) => kv.append(h('div', null, k), h('div', null, ...v));
    try {
      const info = await state.repo.repoInfo();
      const s = info.snapshots.find((x) => x.id === snapshotId);
      if (!s) { add('repo info', 'this snapshot is not listed in the repo file'); return kv; }
      add('message', s.message);
      add('flushed at', s.flushedAt || '—');
      const parent = s.parentIndex >= 0 ? info.snapshots[s.parentIndex] : null;
      add('parent', parent ? [linkTo(parent.id, `snapshots/${parent.id}`), ' ', parent.message ? `(${parent.message})` : ''] : 'none (first snapshot)');
      const branches = [...info.branches].filter(([, id]) => id === snapshotId).map(([n]) => n);
      const tags = [...info.tags].filter(([, id]) => id === snapshotId).map(([n]) => n);
      if (branches.length) add('tip of', branches.join(', '));
      if (tags.length) add('tagged', tags.join(', '));
    } catch (e) {
      add('repo info', `unavailable: ${e.message}`);
    }
    return kv;
  }

  // ─── Snapshot ───────────────────────────────────────────

  async function snapshotPanel(file) {
    const id = file.path.startsWith('snapshots/') ? file.path.slice('snapshots/'.length) : null;
    const v = file.walk({ lenient: true }).value;
    const nodes = (field(v, 'nodes') || { items: [] }).items;
    const panel = h('div', { class: 'panel' }, h('h2', null, 'Snapshot'));
    if (state.repo && id) {
      panel.append(await commitInfo(id), h('p', null, linkTo('Transaction log for this commit', `transactions/${id}`)));
    }
    panel.append(h('h3', null, `Nodes (${nodes.length})`));
    const body = h('tbody');
    const showRows = (a, b) => {
      for (const node of nodes.slice(a, b)) body.append(...nodeRows(node, id));
    };
    showRows(0, PAGE);
    panel.append(h('div', { class: 'table-wrap' }, h('table', null,
      h('thead', null, h('tr', null, h('th', null, 'path'), h('th', null, 'type'), h('th', null, 'shape (chunks)'), h('th', null, 'manifests'))), body)));
    if (nodes.length > PAGE) panel.append(moreButton(nodes.length, PAGE, showRows));
    return panel;
  }

  function nodeRows(node, snapshotId) {
    const data = field(node, 'node_data');
    const nodeId = (field(node, 'id') || {}).id;
    const path = scalarText(field(node, 'path'));
    const isArray = data && data.type === 'ArrayNodeData';
    const shape = isArray ? (field(data, 'shape_v2') || { items: [] }).items.map((d) =>
      `${scalarText(field(d, 'array_length'))} (${scalarText(field(d, 'num_chunks'))})`).join(' × ') : '';
    const manifests = isArray ? (field(data, 'manifests') || { items: [] }).items : [];
    const row = h('tr', null,
      h('td', { class: 'mono' }, path, h('div', { class: 'muted' }, nodeId || '')),
      h('td', null, isArray ? 'array' : data && data.type === 'GroupNodeData' ? 'group' : '?'),
      h('td', { class: 'mono' }, shape),
      h('td', { class: 'mono' }, manifests.length ? manifests.map((m) => {
        const mid = (field(m, 'object_id') || {}).id;
        const ext = (field(m, 'extents') || { items: [] }).items.map((r) => `${scalarText(field(r, 'from'))}..${scalarText(field(r, 'to'))}`).join(', ');
        return h('div', null, mid ? linkTo(shortId(mid), `manifests/${mid}`, { ctx: snapshotId, title: mid }) : '?', h('span', { class: 'muted' }, ` [${ext}]`));
      }) : isArray ? h('span', { class: 'muted' }, 'none (all chunks are fill value)') : ''));
    if (!isArray || !state.repo || !snapshotId) return [row];
    return [row, h('tr', null, h('td', { colSpan: 4 }, findChunkForm(snapshotId, nodeId, path, data)))];
  }

  function parseCoords(text) {
    const parts = text.split(/[\s,]+/).filter(Boolean);
    if (!parts.length) throw new Error('enter chunk coordinates, e.g. 0,3,1');
    return parts.map((p) => {
      if (!/^\d+$/.test(p) || Number(p) > 0xffffffff) throw new Error(`'${p}' is not a chunk index (a non-negative integer)`);
      return Number(p);
    });
  }

  function findChunkForm(snapshotId, nodeId, path, data) {
    const ndim = (field(data, 'shape_v2') || { items: [] }).items.length;
    const input = h('input', { type: 'text', placeholder: Array(Math.max(ndim, 1)).fill('0').join(','), size: 14, 'aria-label': `chunk coordinates for ${path}` });
    const result = h('div', { class: 'find-result' });
    const run = async (ev) => {
      ev.preventDefault();
      result.replaceChildren(h('span', { class: 'muted' }, 'Looking up…'));
      try {
        const coords = parseCoords(input.value);
        if (ndim && coords.length !== ndim) throw new Error(`${path} has ${ndim} dimensions; you gave ${coords.length} coordinates.`);
        const loc = await state.repo.locate(snapshotId, nodeId, coords);
        const parts = [];
        if (loc) parts.push(chunkRefView(loc, snapshotId, coords));
        else {
          parts.push(h('div', { class: 'callout fill' },
            h('strong', null, `No chunk ref for [${coords.join(', ')}]. `),
            'No manifest for these coordinates has a ref. The chunk reads as the array\'s fill value.'));
        }
        result.replaceChildren(...parts);
      } catch (e) {
        result.replaceChildren(errorBox(e));
      }
    };
    return h('form', { class: 'find', onsubmit: run },
      h('div', { class: 'row' }, h('span', { class: 'muted' }, 'Find chunk'), input, h('button', { type: 'submit' }, 'Find')), result);
  }

  // A located ref: where it is, what kind it is, and its checksum fields.
  function chunkRefView(loc, snapshotId, coords) {
    return h('div', { class: 'callout' },
      h('div', null, h('strong', null, `Chunk [${coords.join(', ')}]`), ' is in manifest ',
        linkTo(loc.manifest, `manifests/${loc.manifest}`, { ctx: snapshotId, at: loc.at }), ' at ', h('span', { class: 'mono' }, loc.at)),
      refFacts(loc.chunk_ref),
      h('div', { class: 'tree' }, renderValue('chunk_ref', loc.chunk_ref, { open: 1, ctx: snapshotId })));
  }

  function refFacts(ref) {
    const kv = h('div', { class: 'kv', style: 'margin: 6px 0' });
    const add = (k, ...v) => kv.append(h('div', null, k), h('div', null, ...v));
    const inline = field(ref, 'inline');
    const chunkId = field(ref, 'chunk_id');
    const location = field(ref, 'location');
    const compressed = field(ref, 'compressed_location');
    const etag = field(ref, 'checksum_etag');
    const lastModified = field(ref, 'checksum_last_modified');
    const num = (name) => { const x = field(ref, name); return x ? String(x.v) : '0 (default)'; };
    if (inline) {
      add('type', 'inline');
      add('stored bytes', `${inline.len}`);
    } else if (chunkId) {
      add('type', 'native');
      add('chunk file', chunkId.links && chunkId.links[0] ? linkTo(chunkId.id, chunkId.links[0]) : chunkId.id);
      add('offset / length', `${num('offset')} / ${num('length')}`);
    } else if (location || compressed) {
      add('type', 'virtual');
      add('location', location ? location.v : compressed.decoded && compressed.decoded.text !== undefined ? compressed.decoded.text : '(compressed; no dictionary)');
      if (compressed) add('stored as', `compressed_location, ${compressed.len} bytes (zstd with the manifest's location_dictionary)`);
      add('offset / length', `${num('offset')} / ${num('length')}`);
      add('checksum_etag', etag ? etag.v : h('span', { class: 'muted' }, 'not stored'));
      add('checksum_last_modified', lastModified
        ? (lastModified.k === 'time' ? `${lastModified.raw} s = ${lastModified.iso}` : String(lastModified.v))
        : h('span', { class: 'muted' }, 'not stored'));
      if (!etag && !lastModified) add('', h('span', { class: 'muted' }, 'No checksum stored for the target object.'));
    } else {
      add('type', 'unknown (no inline, chunk_id, or location field)');
    }
    return kv;
  }

  // ─── Transaction log ────────────────────────────────────

  async function transactionPanel(file, nodePaths) {
    const id = file.path.startsWith('transactions/') ? file.path.slice('transactions/'.length) : null;
    const v = file.walk({ lenient: true, nodePaths }).value;
    const updated = (field(v, 'updated_chunks') || { items: [] }).items;
    const panel = h('div', { class: 'panel' }, h('h2', null, 'Transaction log'));
    if (state.repo && id) panel.append(await commitInfo(id), h('p', null, linkTo('Snapshot for this commit', `snapshots/${id}`)));
    panel.append(h('p', { class: 'muted' },
      'The log lists changed chunk coordinates without the kind of change. A manifest has no entry for a deleted chunk. ',
      'Classify looks up each coordinate in the parent\'s and this snapshot\'s manifests.'));
    panel.append(h('h3', null, `Updated chunks (${updated.length} array${updated.length === 1 ? '' : 's'})`));
    if (!updated.length) panel.append(h('p', { class: 'muted' }, 'This commit changed no chunk refs.'));
    for (const entry of updated) {
      const node = field(entry, 'node_id') || {};
      const chunks = field(entry, 'chunks') || { len: 0, items: [] };
      const out = h('div');
      const btn = h('button', { type: 'button', class: 'primary', disabled: !state.repo || !id, onclick: () => classify(file, id, node.id, out, btn) },
        `Classify ${chunks.len} change${chunks.len === 1 ? '' : 's'}`);
      panel.append(h('div', { style: 'margin: 8px 0' },
        h('div', { class: 'row' }, h('span', { class: 'mono' }, node.path || '(unknown path)'), h('span', { class: 'muted mono' }, node.id || ''), btn), out));
    }
    return panel;
  }

  async function classify(file, snapshotId, nodeId, out, btn) {
    btn.disabled = true;
    const progress = h('span', { class: 'muted' }, 'Classifying…');
    out.replaceChildren(progress);
    try {
      const sel = file.walk({ lenient: false }).value;
      const entry = field(sel, 'updated_chunks').items.find((e) => field(e, 'node_id').id === nodeId);
      const coordsList = field(entry, 'chunks').items.map((c) => field(c, 'coords').items.map((x) => Number(x.v)));
      const parent = await state.repo.parentOf(snapshotId);
      const results = [];
      for (const coords of coordsList) {
        results.push(await state.repo.classify(parent, snapshotId, nodeId, coords));
        if (results.length % 25 === 0) {
          progress.textContent = `Classifying ${results.length} of ${coordsList.length}…`;
          await new Promise((r) => setTimeout(r));
        }
      }
      out.replaceChildren(changesTable(results, parent, snapshotId));
    } catch (e) {
      out.replaceChildren(errorBox(e));
    } finally {
      btn.disabled = false;
    }
  }

  function changesTable(results, parent, snapshotId) {
    const counts = {};
    for (const r of results) counts[r.kind] = (counts[r.kind] || 0) + 1;
    const where = (loc, snap) => (loc
      ? h('span', { class: 'mono' }, linkTo(shortId(loc.manifest), `manifests/${loc.manifest}`, { ctx: snap, at: loc.at, title: `${loc.manifest} ${loc.at}` }), ` ${loc.at}`)
      : h('span', { class: 'muted' }, 'no ref'));
    const body = h('tbody');
    const showRows = (a, b) => {
      for (const r of results.slice(a, b)) {
        const detail = h('details', null, h('summary', null, 'refs'),
          r.before ? h('div', null, h('div', { class: 'muted' }, 'before (parent)'), refFacts(r.before.chunk_ref), h('div', { class: 'tree' }, renderValue('chunk_ref', r.before.chunk_ref, { open: 1 }))) : null,
          r.after ? h('div', null, h('div', { class: 'muted' }, 'after (this commit)'), refFacts(r.after.chunk_ref), h('div', { class: 'tree' }, renderValue('chunk_ref', r.after.chunk_ref, { open: 1 }))) : null);
        body.append(h('tr', null,
          h('td', { class: 'mono' }, `[${r.coords.join(', ')}]`),
          h('td', null, h('span', { class: `badge k-${r.kind}` }, r.kind)),
          h('td', null, where(r.before, parent)),
          h('td', null, where(r.after, snapshotId)),
          h('td', null, r.before || r.after ? detail : '')));
      }
    };
    showRows(0, PAGE);
    return h('div', null,
      h('p', null, Object.entries(counts).map(([k, n]) => [h('span', { class: `badge k-${k}` }, `${k} ${n}`), ' '])),
      h('div', { class: 'table-wrap' }, h('table', null,
        h('thead', null, h('tr', null, h('th', null, 'coords'), h('th', null, 'kind'), h('th', null, 'before (parent manifest)'), h('th', null, 'after (this manifest)'), h('th', null, ''))),
        body)),
      results.length > PAGE ? moreButton(results.length, PAGE, showRows) : null);
  }

  // ─── Manifest ───────────────────────────────────────────

  function manifestPanel(file, opts) {
    const v = file.walk({ maxItems: 0, nodePaths: opts.nodePaths, lenient: true, selector: null });
    const arrays = (field(v.value, 'arrays') || { items: [] }).items;
    const dict = field(v.value, 'location_dictionary');
    const panel = h('div', { class: 'panel' }, h('h2', null, 'Manifest'));
    const ctxLine = opts.context
      ? ['Node IDs labeled from snapshot ', linkTo(opts.context, `snapshots/${opts.context}`), opts.ctx ? ' (the snapshot you came from).' : ' (tip of main).']
      : ['Node IDs are unlabeled (no snapshot context).'];
    panel.append(h('p', { class: 'muted' }, ...ctxLine));
    if (dict) panel.append(h('p', null, `Location dictionary: ${dict.len} bytes. Virtual refs store compressed_location, decoded with it.`));
    panel.append(h('div', { class: 'table-wrap' }, h('table', null,
      h('tr', null, h('th', null, '#'), h('th', null, 'array'), h('th', null, 'refs')),
      arrays.map((a, i) => {
        const node = field(a, 'node_id') || {};
        const refs = field(a, 'refs') || { len: 0 };
        return h('tr', null, h('td', null, String(i)), h('td', { class: 'mono' }, node.path || '(unlabeled)', ' ', h('span', { class: 'muted' }, node.id || '')),
          h('td', null, String(refs.len)));
      }))));
    return panel;
  }

  function selectedPanel(file, opts) {
    const panel = h('div', { class: 'panel' }, h('h2', null, 'Selected: ', h('span', { class: 'mono' }, opts.at)));
    try {
      const v = file.walk({ selector: opts.at, nodePaths: opts.nodePaths, lenient: true }).value;
      if (v.k === 'table' && v.type === 'ChunkRef') panel.append(refFacts(v));
      panel.append(h('div', { class: 'tree' }, renderValue(opts.at.split('/').pop(), v, { open: 3, ctx: opts.ctx })));
    } catch (e) {
      panel.append(errorBox(e));
    }
    return panel;
  }

  // ─── Field tree ─────────────────────────────────────────

  function treePanel(decoded, ctx) {
    const fileSnapshot = ctx.file.kind === 'Snapshot' && ctx.file.path.startsWith('snapshots/') ? ctx.file.path.slice(10) : ctx.ctx;
    return h('div', { class: 'panel' }, h('h2', null, 'Fields'),
      h('div', { class: 'tree' }, renderValue(null, decoded.value, { open: 2, ctx: fileSnapshot })));
  }

  const nameEl = (name) => (name === null ? null : [h('span', { class: 'name' }, name), ': ']);

  function leaf(name, ...content) {
    return h('div', { class: 'leaf' }, nameEl(name), ...content);
  }

  // Render one decoded value. `opts.open` is how many levels start expanded;
  // `opts.ctx` is the snapshot passed along when following manifest links.
  function renderValue(name, v, opts, depth = 0) {
    const open = depth < opts.open;
    switch (v.k) {
      case 'table': {
        const d = h('details', { open }, h('summary', null, nameEl(name), h('span', { class: 'type' }, v.type), tableHint(v)));
        let filled = false;
        const fill = () => {
          if (filled) return;
          filled = true;
          for (const [n, x] of v.fields) d.append(renderValue(n, x, opts, depth + 1));
          if (!v.fields.length) d.append(h('div', { class: 'leaf muted' }, '(no fields set)'));
        };
        if (open) fill(); else d.addEventListener('toggle', fill);
        return d;
      }
      case 'vector': {
        const d = h('details', { open: open && v.len > 0 }, h('summary', null, nameEl(name), h('span', { class: 'type' }, `[${v.len}]`)));
        let filled = false;
        const fill = () => {
          if (filled) return;
          filled = true;
          v.items.forEach((x, i) => d.append(renderValue(`[${v.start + i}]`, x, opts, depth + 1)));
          if (!v.len) d.append(h('div', { class: 'leaf muted' }, '(empty)'));
          const loaded = v.start + v.items.length;
          if (loaded < v.len && v.window) {
            let n = loaded;
            const more = h('button', { type: 'button', class: 'link', onclick: () => {
              const items = v.window(n, n + PAGE);
              items.forEach((x, i) => more.before(renderValue(`[${n + i}]`, x, opts, depth + 1)));
              n += items.length;
              if (n >= v.len || !items.length) more.remove(); else more.textContent = `show more (${n} of ${v.len})`;
            } }, `show more (${n} of ${v.len})`);
            d.append(h('div', { class: 'leaf' }, more));
          }
        };
        if (open) fill(); else d.addEventListener('toggle', fill);
        return d;
      }
      case 'scalars': {
        const text = (items) => items.map((x) => (x.k === 'float' ? String(x.v) : x.k === 'bool' ? String(x.v) : String(x.v))).join(', ');
        if (v.items.length <= PAGE) return leaf(name, h('span', { class: 'num' }, `[${text(v.items)}]`));
        const span = h('span', { class: 'num' }, `[${text(v.items.slice(0, PAGE))}, …]`);
        return leaf(name, span, ' ', h('button', { type: 'button', class: 'link', onclick: (e) => {
          span.textContent = `[${text(v.items)}]`;
          e.target.remove();
        } }, `show all ${v.items.length}`));
      }
      case 'bytes': return bytesView(name, v, open);
      case 'id': {
        const links = v.links.map((l) => {
          const dir = l.split('/')[0];
          const label = { snapshots: 'snapshot', transactions: 'tx log', manifests: 'manifest', chunks: 'chunk', overwritten: 'file' }[dir] || dir;
          return [' ', linkTo(`→ ${label}`, l, { ctx: dir === 'manifests' ? opts.ctx : undefined })];
        });
        return leaf(name, h('span', { class: 'str' }, v.id), ...links);
      }
      case 'node':
        return leaf(name, h('span', { class: 'str' }, v.id), ' ', v.path !== null ? h('span', { class: 'label' }, v.path) : h('span', { class: 'muted' }, '(path unknown)'));
      case 'time':
        return leaf(name, h('span', { class: 'num' }, String(v.raw)), ' ', h('span', { class: 'label' }, v.iso));
      case 'enum':
        return leaf(name, h('span', { class: 'str' }, v.name !== null ? v.name : '(unknown)'), ' ', h('span', { class: 'muted' }, `(${v.value})`));
      case 'str':
        return leaf(name, h('span', { class: 'str' }, JSON.stringify(v.v)));
      case 'int': case 'float':
        return leaf(name, h('span', { class: 'num' }, String(v.v)));
      case 'bool':
        return leaf(name, h('span', { class: 'num' }, String(v.v)));
      case 'error':
        return leaf(name, h('span', { class: 'err' }, `decode error: ${v.message}`));
      default:
        return leaf(name, h('span', { class: 'err' }, `unknown value ${v.k}`));
    }
  }

  // A short inline description for common tables, so collapsed rows say
  // what they are.
  function tableHint(v) {
    const parts = [];
    const idx = field(v, 'index');
    if (v.type === 'ChunkRef' && idx && idx.k === 'scalars') parts.push(`[${idx.items.map((x) => x.v).join(', ')}]`);
    const path = field(v, 'path');
    if (path && path.k === 'str') parts.push(path.v);
    const nodeId = field(v, 'node_id');
    if (nodeId && nodeId.k === 'node' && nodeId.path) parts.push(nodeId.path);
    const nm = field(v, 'name');
    if (nm && nm.k === 'str') parts.push(nm.v);
    const msg = field(v, 'message');
    if (msg && msg.k === 'str') parts.push(JSON.stringify(msg.v.length > 60 ? msg.v.slice(0, 60) + '…' : msg.v));
    const id = field(v, 'id');
    if (id && id.k === 'id') parts.push(id.id);
    const coords = field(v, 'coords');
    if (coords && coords.k === 'scalars') parts.push(`[${coords.items.map((x) => x.v).join(', ')}]`);
    return parts.length ? h('span', { class: 'label' }, ' ' + parts.join(' ')) : null;
  }

  function bytesView(name, v, open) {
    const preview = D.hex(v.data.subarray(0, Math.min(v.len, D.PREVIEW_BYTES)));
    const summary = h('summary', null, nameEl(name), h('span', { class: 'type' }, `bytes[${v.len}] `),
      h('span', { class: 'num' }, preview + (v.len > D.PREVIEW_BYTES ? '…' : '')));
    if (v.decoded && v.decoded.text !== undefined && !v.decoded.text.includes('\n') && v.decoded.text.length < 200) {
      summary.append(' ', h('span', { class: 'label' }, '→ '), h('span', { class: 'str' }, v.decoded.text));
    }
    const d = h('details', { open: open && !!v.decoded }, summary);
    if (v.decoded) {
      d.append(h('div', { class: 'leaf muted' }, 'decoded:'),
        h('pre', null, v.decoded.json !== undefined ? D.stringify(v.decoded.json) : v.decoded.text));
    }
    if (v.len > D.PREVIEW_BYTES) {
      let shown = 0;
      const pre = h('pre');
      const more = h('button', { type: 'button', class: 'link', onclick: () => {
        const next = Math.min(v.len, shown + HEX_PAGE);
        pre.textContent += hexDump(v.data, shown, next);
        shown = next;
        if (shown >= v.len) more.remove(); else more.textContent = `more hex (${shown} of ${v.len} bytes)`;
      } }, 'show hex');
      d.append(pre, h('div', { class: 'leaf' }, more));
    }
    return d;
  }

  function hexDump(data, from, to) {
    let out = '';
    for (let i = from; i < to; i += 16) {
      const row = data.subarray(i, Math.min(i + 16, to));
      out += i.toString(16).padStart(8, '0') + '  ' + [...row].map((b) => b.toString(16).padStart(2, '0')).join(' ') + '\n';
    }
    return out;
  }

  // ─── Wiring ─────────────────────────────────────────────

  const ready = (async () => {
    let zstd = null;
    try {
      if (!window.IcechunkZstd) throw new Error('vendor/zstd.js did not load');
      await window.IcechunkZstd.init();
      zstd = D.zstdFromLib(window.IcechunkZstd);
    } catch (e) {
      state.zstdError = e && e.message ? e.message : String(e);
      $('status').replaceChildren(notice(`The zstd decoder failed to start (${state.zstdError}). `,
        'Compressed files cannot be decoded; uncompressed files still work.'));
    }
    state.decoder = D.create({ schema: window.ICECHUNK_SCHEMA, zstd });
  })();

  // Set by `core-drill <repo> web`, which serves the repository's objects
  // next to this page.
  const served = window.CORE_DRILL && typeof window.CORE_DRILL.source === 'string' ? window.CORE_DRILL : null;

  $('toggle-open').addEventListener('click', () => showOpen($('open').hidden));
  $('dir-input').addEventListener('change', (e) => openDirectory(e.target.files));
  $('file-input').addEventListener('change', (e) => { if (e.target.files[0]) openSingleFile(e.target.files[0]); });
  $('url-form').addEventListener('submit', (e) => {
    e.preventDefault();
    openUrl($('url-input').value, $('region-input').value);
  });
  const drop = $('drop');
  drop.addEventListener('dragover', (e) => { e.preventDefault(); drop.classList.add('over'); });
  drop.addEventListener('dragleave', () => drop.classList.remove('over'));
  drop.addEventListener('drop', (e) => {
    e.preventDefault();
    drop.classList.remove('over');
    const f = e.dataTransfer.files[0];
    if (f) openSingleFile(f);
  });
  window.addEventListener('hashchange', onRoute);
  ready.then(async () => {
    if (served && !parseHash().src) await openServed();
    onRoute();
  });
})();
