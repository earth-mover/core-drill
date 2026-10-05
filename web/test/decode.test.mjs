// Compares web/decoder.js against the Rust CLI on every metadata file in the
// local test repos, and checks chunk-change classification for each commit of
// chunk-lifecycle-repo.
//
//   node web/test/decode.test.mjs
//
// Needs `cargo build` to have produced target/debug/core-drill (override with
// CORE_DRILL_BIN). zstd comes from web/vendor/zstd.js, the same file the page
// loads.

import { execFileSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..', '..');
const bin = process.env.CORE_DRILL_BIN || path.join(root, 'target', 'debug', 'core-drill');
const require = createRequire(import.meta.url);
const D = require('../decoder.js');
const sandbox = { window: {} };
vm.runInNewContext(readFileSync(path.join(root, 'web', 'schema.js'), 'utf8'), sandbox);
vm.runInNewContext(readFileSync(path.join(root, 'web', 'vendor', 'zstd.js'), 'utf8') + ';window.IcechunkZstd = IcechunkZstd;', sandbox);
const zlib = sandbox.window.IcechunkZstd;
await zlib.init();
const decoder = D.create({ schema: sandbox.window.ICECHUNK_SCHEMA, zstd: D.zstdFromLib(zlib) });

function rust(repoDir, args) {
  const run = () => execFileSync(bin, [repoDir, '--output', 'json', ...args], { encoding: 'utf8', maxBuffer: 1 << 30, stdio: ['ignore', 'pipe', 'pipe'] });
  try {
    return { ok: true, value: D.parseJson(run(), { sortKeys: false, maxDepth: 1e6 }) };
  } catch (e) {
    const stderr = String(e.stderr || e.message).replace(/\x1b\[[0-9;]*m/g, '');
    return { ok: false, error: stderr.split('\n').find((l) => l.trim() && !/^Error:\s*$/.test(l.trim())) || stderr };
  }
}

// First difference between two plain JSON values, or null. Key order counts.
function diff(a, b, at = '') {
  const kind = (v) => (v === null ? 'null' : Array.isArray(v) ? 'array' : v instanceof Map ? 'object' : typeof v);
  if (kind(a) !== kind(b)) return `${at || '/'}: ${kind(a)} ${D.stringify(a, '')} vs ${kind(b)} ${D.stringify(b, '')}`;
  if (Array.isArray(a)) {
    if (a.length !== b.length) return `${at}: length ${a.length} vs ${b.length}`;
    for (let i = 0; i < a.length; i++) { const d = diff(a[i], b[i], `${at}/${i}`); if (d) return d; }
    return null;
  }
  if (a instanceof Map) {
    const ka = [...a.keys()], kb = [...b.keys()];
    if (ka.join('\0') !== kb.join('\0')) return `${at || '/'}: keys [${ka}] vs [${kb}]`;
    for (const k of ka) { const d = diff(a.get(k), b.get(k), `${at}/${k}`); if (d) return d; }
    return null;
  }
  return a === b ? null : `${at || '/'}: ${D.stringify(a, '')} vs ${D.stringify(b, '')}`;
}

const fsFetch = (dir) => async (rel) => new Uint8Array(readFileSync(path.join(dir, rel)));

// V1 repos have no repo info file; core-drill resolves `main` from refs/.
function v1MainTip(dir) {
  const ref = path.join(dir, 'refs', 'branch.main', 'ref.json');
  return existsSync(ref) ? JSON.parse(readFileSync(ref, 'utf8')).snapshot : null;
}

const results = { pass: 0, fail: 0, skip: 0 };
const failures = [];
const skips = [];

for (const name of ['chunk-lifecycle-repo', 'virtual-chunks-repo', 'mixed-chunks-repo', 'adversarial-repo']) {
  const dir = path.join(root, 'test-data', name);
  if (!existsSync(dir)) { console.log(`skip ${name}: not present`); continue; }
  const repo = decoder.repo(fsFetch(dir));
  const isV1 = !existsSync(path.join(dir, 'repo'));
  const files = [];
  if (!isV1) files.push('repo');
  for (const sub of ['snapshots', 'manifests', 'transactions', 'overwritten']) {
    if (existsSync(path.join(dir, sub))) for (const f of readdirSync(dir + '/' + sub).sort()) files.push(`${sub}/${f}`);
  }
  let pass = 0;
  for (const rel of files) {
    const expected = rust(dir, ['object', rel, '-n', '0']);
    if (!expected.ok) {
      results.skip++;
      skips.push(`${name}/${rel}: Rust failed (${expected.error.trim()})`);
      continue;
    }
    let got;
    try {
      const file = await repo.open(rel);
      const context = isV1 && file.kind === 'Manifest' ? v1MainTip(dir) : null;
      const nodePaths = await repo.nodeContext(file, context);
      got = D.toPlain(file.walk({ nodePaths }).value);
    } catch (e) {
      results.fail++;
      failures.push(`${name}/${rel}: decoder threw ${e.stack || e}`);
      continue;
    }
    const d = diff(got, expected.value.get('value'));
    if (d) { results.fail++; failures.push(`${name}/${rel}: ${d}`); } else { results.pass++; pass++; }
  }
  console.log(`${name}: ${pass}/${files.length} files match`);
}

// Chunk-change classification for every commit of chunk-lifecycle-repo.
{
  const dir = path.join(root, 'test-data', 'chunk-lifecycle-repo');
  const repo = decoder.repo(fsFetch(dir));
  const info = await repo.repoInfo();
  for (const snap of info.snapshots) {
    const expected = rust(dir, ['chunk-changes', snap.id, '-n', '0']);
    if (!expected.ok) { results.skip++; skips.push(`chunk-changes ${snap.id}: Rust failed (${expected.error.trim()})`); continue; }
    const got = D.changesToPlain(await repo.chunkChanges(snap.id));
    const d = diff(got, expected.value);
    const kinds = got.get('arrays').flatMap((a) => a.get('changes').map((c) => `${a.get('path')}[${c.get('coords').join(',')}]=${c.get('kind')}`));
    const summary = kinds.length > 6 ? `${kinds.slice(0, 3).join(' ')} … (${kinds.length} changes)` : kinds.join(' ') || '(no chunk changes)';
    if (d) { results.fail++; failures.push(`chunk-changes ${snap.id}: ${d}`); }
    else { results.pass++; console.log(`chunk-changes ${snap.id} "${snap.message}": ${summary}`); }
  }

  // A few direct lookups, including coordinates with no ref.
  const main = await repo.branchTip('main');
  for (const [p, coords] of [['/native', [0, 0]], ['/native', [1, 1]], ['/virtual', [7]], ['/virtual', [8]], ['/virtual', [5000]]]) {
    const expected = rust(dir, ['chunk-ref', p, coords.join(','), '-r', main]);
    if (!expected.ok) { results.skip++; skips.push(`chunk-ref ${p} ${coords}: Rust failed (${expected.error.trim()})`); continue; }
    const got = await repo.chunkRef(main, p, coords);
    const loc = got.location;
    const plain = new Map([['snapshot', got.snapshot], ['path', got.path], ['node_id', got.node_id], ['coords', coords.map(BigInt)],
      ['location', loc ? new Map([['manifest', loc.manifest], ['at', loc.at], ['chunk_ref', D.toPlain(loc.chunk_ref)]]) : null]]);
    const d = diff(plain, expected.value);
    if (d) { results.fail++; failures.push(`chunk-ref ${p} ${coords}: ${d}`); }
    else { results.pass++; console.log(`chunk-ref ${p} [${coords}]: ${loc ? `${loc.manifest} ${loc.at}` : 'no ref (fill value)'}`); }
  }
}

// Sanitizer cases with known core-drill output.
for (const [input, want] of [
  ['hello world', 'hello world'], ['\x1b[31mred\x1b[0m', 'red'], ['\x1b[2Jhello', 'hello'], ['\x1b]0;evil title\x07text', 'text'],
  ['a\x00b\x01c\x7fd', 'abcd'], ['RTL trick: ' + String.fromCharCode(0x202e) + 'gnp.exe', 'RTL trick: gnp.exe'],
  [String.fromCharCode(0x200e, 0x2066) + 'x' + String.fromCharCode(0x2069, 0x200f), 'x'], ['line1\nline2\tcol', 'line1\nline2\tcol'], ['data × 100', 'data × 100'],
]) {
  const got = D.sanitize(input);
  if (got === want) results.pass++; else { results.fail++; failures.push(`sanitize ${JSON.stringify(input)}: ${JSON.stringify(got)}`); }
}

for (const s of skips) console.log(`SKIP ${s}`);
for (const f of failures) console.log(`FAIL ${f}`);
console.log(`\n${results.pass} passed, ${results.fail} failed, ${results.skip} skipped`);
process.exit(results.fail ? 1 : 0);
