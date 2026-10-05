// Icechunk V2 metadata decoder: header parsing, schema-driven flatbuffer
// walking, field annotations, and chunk-ref lookups.
//
// This is a port of core-drill's `src/raw/` and `src/fetch/raw.rs`; the JSON
// produced by `toPlain(value)` matches `core-drill <repo> --output json
// object <path>` field for field. No DOM access, so it also runs under Node
// for tests. zstd is injected (see `zstdFromLib`) because the browser and
// Node load the wasm module differently.
(function (root, factory) {
  if (typeof module === 'object' && module.exports) module.exports = factory();
  else root.IcechunkDecoder = factory();
})(typeof self !== 'undefined' ? self : this, function () {
  'use strict';

  const MAGIC = [0x49, 0x43, 0x45, 0xf0, 0x9f, 0xa7, 0x8a, 0x43, 0x48, 0x55, 0x4e, 0x4b]; // "ICE🧊CHUNK"
  const HEADER_LEN = 39;
  const MAX_DECOMPRESSED = 2 ** 31;
  const MAX_LOCATION_LEN = 64 * 1024;
  const PREVIEW_BYTES = 32;
  const MAX_DEPTH = 64;

  const FILE_TYPE_NAMES = { 1: 'Snapshot', 2: 'Manifest', 3: 'Attributes', 4: 'TransactionLog', 5: 'Chunk', 6: 'RepoInfo' };
  const KIND_FROM_HEADER = { 1: 'Snapshot', 2: 'Manifest', 4: 'TransactionLog', 5: 'Chunk', 6: 'RepoInfo' };
  const ROOT_OBJECT = { RepoInfo: 'Repo', Snapshot: 'Snapshot', Manifest: 'Manifest', TransactionLog: 'TransactionLog' };

  class DecodeError extends Error {}
  const fail = (msg) => { throw new DecodeError(msg); };

  // ─── Text ───────────────────────────────────────────────

  const utf8Lossy = new TextDecoder('utf-8', { fatal: false, ignoreBOM: true });
  const utf8Strict = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
  const utf8Encoder = new TextEncoder();

  function strictUtf8(bytes) {
    try { return utf8Strict.decode(bytes); } catch (_) { return null; }
  }

  // One step of UTF-8 validation with Rust's `Utf8Error` semantics:
  // {len} for a valid char, {err: n} for an invalid n-byte subpart,
  // {incomplete: true} when the input ends mid-character.
  function utf8Step(b, i, end) {
    const c = b[i];
    if (c < 0x80) return { len: 1 };
    let width, lo = 0x80, hi = 0xbf;
    if (c >= 0xc2 && c <= 0xdf) width = 2;
    else if (c >= 0xe0 && c <= 0xef) { width = 3; if (c === 0xe0) lo = 0xa0; if (c === 0xed) hi = 0x9f; }
    else if (c >= 0xf0 && c <= 0xf4) { width = 4; if (c === 0xf0) lo = 0x90; if (c === 0xf4) hi = 0x8f; }
    else return { err: 1 };
    for (let k = 1; k < width; k++) {
      if (i + k >= end) return { incomplete: true };
      const x = b[i + k];
      if (k === 1 ? (x < lo || x > hi) : (x < 0x80 || x > 0xbf)) return { err: k };
    }
    return { len: width };
  }

  // Byte-level port of the `vte` 0.14 state machine that
  // `strip-ansi-escapes` drives. Only printable ground-state characters
  // survive; everything a terminal would interpret is dropped.
  const S = { GROUND: 0, ESC: 1, ESC_INT: 2, CSI_ENTRY: 3, CSI_PARAM: 4, CSI_INT: 5, CSI_IGNORE: 6,
    DCS_ENTRY: 7, DCS_PARAM: 8, DCS_INT: 9, DCS_PASS: 10, DCS_IGNORE: 11, OSC: 12, SOS: 13 };
  const isC0 = (b) => b <= 0x17 || b === 0x19 || (b >= 0x1c && b <= 0x1f);

  function anywhere(state, b) {
    if (b === 0x18 || b === 0x1a) return S.GROUND;
    if (b === 0x1b) return S.ESC;
    return state;
  }

  function nextState(state, b) {
    switch (state) {
      case S.CSI_ENTRY:
        if (isC0(b)) return state;
        if (b >= 0x20 && b <= 0x2f) return S.CSI_INT;
        if (b >= 0x30 && b <= 0x3f) return S.CSI_PARAM;
        if (b >= 0x40 && b <= 0x7e) return S.GROUND;
        return anywhere(state, b);
      case S.CSI_IGNORE:
        if (isC0(b) || (b >= 0x20 && b <= 0x3f) || b === 0x7f) return state;
        if (b >= 0x40 && b <= 0x7e) return S.GROUND;
        return anywhere(state, b);
      case S.CSI_INT:
        if (isC0(b) || (b >= 0x20 && b <= 0x2f)) return state;
        if (b >= 0x30 && b <= 0x3f) return S.CSI_IGNORE;
        if (b >= 0x40 && b <= 0x7e) return S.GROUND;
        return anywhere(state, b);
      case S.CSI_PARAM:
        if (isC0(b) || (b >= 0x30 && b <= 0x3b) || b === 0x7f) return state;
        if (b >= 0x20 && b <= 0x2f) return S.CSI_INT;
        if (b >= 0x3c && b <= 0x3f) return S.CSI_IGNORE;
        if (b >= 0x40 && b <= 0x7e) return S.GROUND;
        return anywhere(state, b);
      case S.DCS_ENTRY:
        if (isC0(b) || b === 0x7f) return state;
        if (b >= 0x20 && b <= 0x2f) return S.DCS_INT;
        if (b >= 0x30 && b <= 0x3f) return S.DCS_PARAM;
        if (b >= 0x40 && b <= 0x7e) return S.DCS_PASS;
        return anywhere(state, b);
      case S.DCS_INT:
        if (isC0(b) || (b >= 0x20 && b <= 0x2f) || b === 0x7f) return state;
        if (b >= 0x30 && b <= 0x3f) return S.DCS_IGNORE;
        if (b >= 0x40 && b <= 0x7e) return S.DCS_PASS;
        return anywhere(state, b);
      case S.DCS_PARAM:
        if (isC0(b) || (b >= 0x30 && b <= 0x3b) || b === 0x7f) return state;
        if (b >= 0x20 && b <= 0x2f) return S.DCS_INT;
        if (b >= 0x3c && b <= 0x3f) return S.DCS_IGNORE;
        if (b >= 0x40 && b <= 0x7e) return S.DCS_PASS;
        return anywhere(state, b);
      case S.DCS_PASS:
        if (b === 0x18 || b === 0x1a || b === 0x9c) return S.GROUND;
        if (b === 0x1b) return S.ESC;
        return state;
      case S.ESC:
        if (b >= 0x20 && b <= 0x2f) return S.ESC_INT;
        if (b === 0x50) return S.DCS_ENTRY;
        if (b === 0x58 || b === 0x5e || b === 0x5f) return S.SOS;
        if (b === 0x5b) return S.CSI_ENTRY;
        if (b === 0x5d) return S.OSC;
        if (b >= 0x30 && b <= 0x7e) return S.GROUND;
        if (b === 0x18 || b === 0x1a) return S.GROUND;
        return state;
      case S.ESC_INT:
        if (isC0(b) || (b >= 0x20 && b <= 0x2f) || b === 0x7f) return state;
        if (b >= 0x30 && b <= 0x7e) return S.GROUND;
        return anywhere(state, b);
      case S.OSC:
        if (b === 0x07 || b === 0x18 || b === 0x1a) return S.GROUND;
        if (b === 0x1b) return S.ESC;
        return state;
      default: // DCS_IGNORE, SOS
        return anywhere(state, b);
    }
  }

  function stripAnsi(bytes) {
    let out = '';
    let state = S.GROUND;
    let i = 0;
    const n = bytes.length;
    while (i < n) {
      if (state !== S.GROUND) {
        state = nextState(state, bytes[i]);
        i++;
        continue;
      }
      let esc = bytes.indexOf(0x1b, i);
      if (esc < 0) esc = n;
      if (esc === i) { state = S.ESC; i++; continue; }
      let j = i;
      while (j < esc) {
        const step = utf8Step(bytes, j, esc);
        if (step.len) {
          const c = codePointAt(bytes, j, step.len);
          if (!(c <= 0x1f || (c >= 0x80 && c <= 0x9f))) out += String.fromCodePoint(c);
          j += step.len;
        } else if (step.err) {
          if (!(step.err === 1 && bytes[j] <= 0x9f)) out += '\uFFFD';
          j += step.err;
        } else {
          if (esc < n) { out += '\uFFFD'; state = S.ESC; j = esc + 1; }
          else j = n; // partial character at end of input is never emitted
          break;
        }
      }
      if (j === esc && esc < n) { state = S.ESC; j = esc + 1; }
      i = j;
    }
    return out;
  }

  function codePointAt(b, i, len) {
    if (len === 1) return b[i];
    if (len === 2) return ((b[i] & 0x1f) << 6) | (b[i + 1] & 0x3f);
    if (len === 3) return ((b[i] & 0x0f) << 12) | ((b[i + 1] & 0x3f) << 6) | (b[i + 2] & 0x3f);
    return ((b[i] & 0x07) << 18) | ((b[i + 1] & 0x3f) << 12) | ((b[i + 2] & 0x3f) << 6) | (b[i + 3] & 0x3f);
  }

  // Explicit directional formatting characters, which can make text display
  // in a different order than it is stored.
  const isBidiControl = (c) => c === 0x200e || c === 0x200f || (c >= 0x202a && c <= 0x202e) || (c >= 0x2066 && c <= 0x2069);

  // Port of core-drill's `sanitize`: strip terminal escape sequences, control
  // characters, and bidi controls, keeping tabs and newlines.
  function sanitize(s) {
    if (!/[\x00-\x1f\x7f-\x9f\uE000\uE001\u200E\u200F\u202A-\u202E\u2066-\u2069]/.test(s)) return s;
    const protectedText = s.replace(/\t/g, '\uE000').replace(/\n/g, '\uE001');
    const stripped = stripAnsi(utf8Encoder.encode(protectedText));
    let cleaned = '';
    for (const ch of stripped) {
      const c = ch.codePointAt(0);
      if (c <= 0x1f || (c >= 0x7f && c <= 0x9f) || isBidiControl(c)) continue;
      cleaned += ch;
    }
    return cleaned.replace(/\uE000/g, '\t').replace(/\uE001/g, '\n');
  }

  // Rust's `char::is_whitespace` set, for `trim_end`.
  const RUST_WS = /[\t\n\v\f\r \u0085\u00a0\u1680\u2000-\u200A\u2028\u2029\u202F\u205F\u3000]+$/u;

  // ─── IDs, times, hex ────────────────────────────────────

  const CROCKFORD = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';

  function encodeId(bytes) {
    let out = '', acc = 0, bits = 0;
    for (const b of bytes) {
      acc = ((acc << 8) | b) & 0xffff;
      bits += 8;
      while (bits >= 5) { out += CROCKFORD[(acc >>> (bits - 5)) & 31]; bits -= 5; }
    }
    if (bits > 0) out += CROCKFORD[(acc << (5 - bits)) & 31];
    return out;
  }

  function decodeId(s) {
    let acc = 0, bits = 0;
    const out = [];
    for (const ch of s) {
      const v = CROCKFORD.indexOf(ch);
      if (v < 0) return null;
      acc = ((acc << 5) | v) & 0xffff;
      bits += 5;
      if (bits >= 8) { out.push((acc >>> (bits - 8)) & 0xff); bits -= 8; }
    }
    return Uint8Array.from(out);
  }

  const isObjectId = (s) => typeof s === 'string' && /^[0-9A-HJKMNP-TV-Z]{20}$/.test(s);

  // Object keys are joined onto the repo base, so refuse anything that could
  // climb out of it (same rule as core-drill's `check_key`).
  function checkKey(path) {
    return typeof path === 'string' && path.length > 0 &&
      path.split('/').every((seg) => seg !== '' && seg !== '.' && seg !== '..' && /^[A-Za-z0-9._-]+$/.test(seg));
  }

  function hex(bytes) {
    let s = '';
    for (const b of bytes) s += (b < 16 ? '0' : '') + b.toString(16);
    return s;
  }

  const pad2 = (n) => (n < 10 ? '0' : '') + n;

  function civilFromDays(z) {
    z += 719468;
    const era = Math.floor(z / 146097);
    const doe = z - era * 146097;
    const yoe = Math.floor((doe - Math.floor(doe / 1460) + Math.floor(doe / 36524) - Math.floor(doe / 146096)) / 365);
    const doy = doe - (365 * yoe + Math.floor(yoe / 4) - Math.floor(yoe / 100));
    const mp = Math.floor((5 * doy + 2) / 153);
    const d = doy - Math.floor((153 * mp + 2) / 5) + 1;
    const m = mp < 10 ? mp + 3 : mp - 9;
    return [yoe + era * 400 + (m <= 2 ? 1 : 0), m, d];
  }

  const floorDiv = (a, b) => (a >= 0n ? a / b : -((-a + b - 1n) / b));

  // chrono's `to_rfc3339_opts(Secs, true)` for a signed second count, or
  // null outside chrono's representable range.
  function rfc3339(secs) {
    const days = floorDiv(secs, 86400n);
    if (days < -2147483648n || days > 2147483647n) return null;
    let sod = Number(secs - days * 86400n);
    const [y, m, d] = civilFromDays(Number(days));
    if (y < -262143 || y > 262142) return null;
    const year = y >= 0 && y <= 9999 ? String(y).padStart(4, '0') : (y < 0 ? '-' : '+') + String(Math.abs(y)).padStart(4, '0');
    const hh = Math.floor(sod / 3600); sod -= hh * 3600;
    const mm = Math.floor(sod / 60);
    const ss = sod - mm * 60;
    return `${year}-${pad2(m)}-${pad2(d)}T${pad2(hh)}:${pad2(mm)}:${pad2(ss)}Z`;
  }

  const toBig = (v) => (typeof v === 'bigint' ? v : BigInt(v));
  const fromBig = (b) => (b >= BigInt(Number.MIN_SAFE_INTEGER) && b <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(b) : b);

  // ─── JSON (serde_json-compatible) ───────────────────────
  //
  // Decoded JSON values are: null, booleans, strings, arrays, Maps (objects),
  // BigInt (integers), and Number (floats).

  function parseJson(text, { sortKeys = true, maxDepth = 127 } = {}) {
    let i = 0;
    const n = text.length;
    const ws = () => { while (i < n && (text[i] === ' ' || text[i] === '\t' || text[i] === '\n' || text[i] === '\r')) i++; };
    const err = (m) => fail(`JSON: ${m} at ${i}`);
    const value = (depth) => {
      ws();
      const c = text[i];
      if (c === '{') {
        if (depth >= maxDepth) err('recursion limit exceeded');
        i++;
        const entries = [];
        ws();
        if (text[i] === '}') { i++; return finishObject(entries); }
        for (;;) {
          ws();
          if (text[i] !== '"') err('expected key');
          const k = string();
          ws();
          if (text[i] !== ':') err('expected :');
          i++;
          entries.push([k, value(depth + 1)]);
          ws();
          if (text[i] === ',') { i++; continue; }
          if (text[i] === '}') { i++; return finishObject(entries); }
          err('expected , or }');
        }
      }
      if (c === '[') {
        if (depth >= maxDepth) err('recursion limit exceeded');
        i++;
        const items = [];
        ws();
        if (text[i] === ']') { i++; return items; }
        for (;;) {
          items.push(value(depth + 1));
          ws();
          if (text[i] === ',') { i++; continue; }
          if (text[i] === ']') { i++; return items; }
          err('expected , or ]');
        }
      }
      if (c === '"') return string();
      if (text.startsWith('true', i)) { i += 4; return true; }
      if (text.startsWith('false', i)) { i += 5; return false; }
      if (text.startsWith('null', i)) { i += 4; return null; }
      return number();
    };
    const finishObject = (entries) => {
      const m = new Map();
      if (sortKeys) {
        entries.sort((a, b) => compareUtf8(a[0], b[0]));
        // A sorted map keeps the last value for a duplicated key.
      }
      for (const [k, v] of entries) m.set(k, v);
      return m;
    };
    const string = () => {
      i++;
      let out = '';
      for (;;) {
        if (i >= n) err('unterminated string');
        const c = text.charCodeAt(i);
        if (c === 0x22) { i++; return out; }
        if (c < 0x20) err('control character in string');
        if (c !== 0x5c) { out += text[i++]; continue; }
        const e = text[i + 1];
        i += 2;
        const simple = { '"': '"', '\\': '\\', '/': '/', b: '\b', f: '\f', n: '\n', r: '\r', t: '\t' }[e];
        if (simple !== undefined) { out += simple; continue; }
        if (e !== 'u') err('bad escape');
        const hex4 = () => {
          const h = text.slice(i, i + 4);
          if (!/^[0-9a-fA-F]{4}$/.test(h)) err('bad \\u escape');
          i += 4;
          return parseInt(h, 16);
        };
        const u = hex4();
        if (u >= 0xd800 && u <= 0xdbff) {
          if (text[i] !== '\\' || text[i + 1] !== 'u') err('lone leading surrogate');
          i += 2;
          const lo = hex4();
          if (lo < 0xdc00 || lo > 0xdfff) err('invalid surrogate pair');
          out += String.fromCharCode(u, lo);
        } else if (u >= 0xdc00 && u <= 0xdfff) {
          err('lone trailing surrogate');
        } else {
          out += String.fromCharCode(u);
        }
      }
    };
    const number = () => {
      const m = /^-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?/.exec(text.slice(i, i + 400));
      if (!m || m[0] === '' || m[0] === '-') err('expected value');
      i += m[0].length;
      if (!m[2] && !m[3]) {
        const b = BigInt(m[0]);
        if (b >= -(2n ** 63n) && b < 2n ** 64n) return b;
      }
      const f = Number(m[0]);
      if (!Number.isFinite(f)) err('number out of range');
      return f;
    };
    const v = value(0);
    ws();
    if (i !== n) err('trailing characters');
    return v;
  }

  function compareUtf8(a, b) {
    const x = utf8Encoder.encode(a), y = utf8Encoder.encode(b);
    const len = Math.min(x.length, y.length);
    for (let k = 0; k < len; k++) if (x[k] !== y[k]) return x[k] - y[k];
    return x.length - y.length;
  }

  function sanitizeJson(v) {
    if (typeof v === 'string') return sanitize(v);
    if (Array.isArray(v)) return v.map(sanitizeJson);
    if (v instanceof Map) {
      const entries = [];
      for (const [k, x] of v) entries.push([sanitize(k), sanitizeJson(x)]);
      const out = new Map();
      for (const [k, x] of entries) out.set(k, x);
      return new Map([...out].sort((a, b) => compareUtf8(a[0], b[0])));
    }
    return v;
  }

  function stringify(v, indent = '  ', level = 0) {
    const pad = indent ? '\n' + indent.repeat(level + 1) : '';
    const end = indent ? '\n' + indent.repeat(level) : '';
    const sep = indent ? ': ' : ':';
    if (v === null || v === undefined) return 'null';
    if (typeof v === 'bigint') return v.toString();
    if (typeof v === 'number') return Number.isFinite(v) ? (Number.isInteger(v) && Math.abs(v) < 1e16 ? v.toFixed(1) : String(v)) : 'null';
    if (typeof v === 'boolean') return v ? 'true' : 'false';
    if (typeof v === 'string') return JSON.stringify(v);
    if (Array.isArray(v)) {
      if (!v.length) return '[]';
      return '[' + v.map((x) => pad + stringify(x, indent, level + 1)).join(',') + end + ']';
    }
    if (v instanceof Map) {
      if (!v.size) return '{}';
      return '{' + [...v].map(([k, x]) => pad + JSON.stringify(k) + sep + stringify(x, indent, level + 1)).join(',') + end + '}';
    }
    return JSON.stringify(v);
  }

  // ─── FlexBuffers ────────────────────────────────────────
  //
  // Mirrors `flexbuffers::from_slice::<serde_json::Value>`, including its
  // leniency: unreadable vector elements become null, invalid strings become
  // empty. Anything serde_json cannot represent (blobs) fails the decode.

  const FBT = { NULL: 0, INT: 1, UINT: 2, FLOAT: 3, KEY: 4, STRING: 5, IND_INT: 6, IND_UINT: 7, IND_FLOAT: 8,
    MAP: 9, VECTOR: 10, VEC_INT: 11, VEC_UINT: 12, VEC_FLOAT: 13, VEC_KEY: 14, VEC_STRING: 15, BLOB: 25, BOOL: 26, VEC_BOOL: 36 };

  function readFlexbuffer(buf) {
    const n = buf.length;
    const dv = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
    const isVector = (t) => (t >= 9 && t < 25) || t === FBT.VEC_BOOL;
    const fixedLen = (t) => (t >= 16 && t <= 24 ? Math.floor((t - 16) / 3) + 2 : 0);
    const isInline = (t) => t <= FBT.FLOAT || t === FBT.BOOL;
    const typedElem = (t) => {
      if (t === FBT.VEC_INT || t === 16 || t === 19 || t === 22) return FBT.INT;
      if (t === FBT.VEC_UINT || t === 17 || t === 20 || t === 23) return FBT.UINT;
      if (t === FBT.VEC_FLOAT || t === 18 || t === 21 || t === 24) return FBT.FLOAT;
      if (t === FBT.VEC_KEY || t === FBT.VEC_STRING) return FBT.KEY;
      if (t === FBT.VEC_BOOL) return FBT.BOOL;
      return -1;
    };
    const unpack = (b) => {
      const t = b >> 2;
      if (!(t <= 26 || t === 36)) fail('flexbuffer: invalid packed type');
      return [t, 1 << (b & 3)];
    };
    const readUsize = (addr, w) => {
      if (addr > n || (w === 1 && addr >= n)) fail('flexbuffer: out of bounds');
      if (addr + w > n) return 0;
      if (w === 1) return buf[addr];
      if (w === 2) return dv.getUint16(addr, true);
      if (w === 4) return dv.getUint32(addr, true);
      const big = dv.getBigUint64(addr, true);
      return big > BigInt(Number.MAX_SAFE_INTEGER) ? Infinity : Number(big);
    };
    const reader = (addr, t, w, parentW) => {
      if (!isInline(t)) {
        const off = readUsize(addr, parentW);
        if (off > addr) fail('flexbuffer: out of bounds');
        addr -= off;
        if (t === FBT.IND_INT) t = FBT.INT;
        else if (t === FBT.IND_UINT) t = FBT.UINT;
        else if (t === FBT.IND_FLOAT) t = FBT.FLOAT;
      }
      return { addr, t, w };
    };
    const length = (r) => {
      const f = fixedLen(r.t);
      if (f) return f;
      const hasSlot = isVector(r.t) || r.t === FBT.STRING || r.t === FBT.BLOB;
      if (hasSlot && r.addr >= r.w) return readUsize(r.addr - r.w, r.w);
      return 0;
    };
    const readInt = (r, signed) => {
      if (r.addr + r.w > n) return 0n;
      if (r.w === 1) return BigInt(signed ? dv.getInt8(r.addr) : buf[r.addr]);
      if (r.w === 2) return BigInt(signed ? dv.getInt16(r.addr, true) : dv.getUint16(r.addr, true));
      if (r.w === 4) return BigInt(signed ? dv.getInt32(r.addr, true) : dv.getUint32(r.addr, true));
      return signed ? dv.getBigInt64(r.addr, true) : dv.getBigUint64(r.addr, true);
    };
    const asStr = (r) => {
      let bytes;
      if (r.t === FBT.KEY) {
        let end = buf.indexOf(0, r.addr);
        if (end < 0) end = r.addr;
        bytes = buf.subarray(r.addr, end);
      } else {
        const len = length(r);
        if (r.addr + len > n) return '';
        bytes = buf.subarray(r.addr, r.addr + len);
      }
      const s = strictUtf8(bytes);
      return s === null ? '' : s;
    };
    // Mirrors `VectorReader::idx`: errors yield a default (null) reader.
    const element = (vec, i, len) => {
      try {
        const et = typedElem(vec.t);
        let t, w;
        if (et >= 0) { t = et; w = vec.w; } else {
          const typesAddr = vec.addr + len * vec.w;
          if (typesAddr + i >= n) fail('flexbuffer: out of bounds');
          [t, w] = unpack(buf[typesAddr + i]);
        }
        return reader(vec.addr + vec.w * i, t, w, vec.w);
      } catch (e) {
        if (e instanceof DecodeError) return { addr: 0, t: FBT.NULL, w: 1 };
        throw e;
      }
    };
    const checkLen = (len, w) => {
      if (len * w > n) fail('flexbuffer: vector longer than buffer');
    };
    let depth = 0;
    const value = (r) => {
      switch (r.t) {
        case FBT.BOOL: {
          if (r.addr + r.w > n) fail('flexbuffer: out of bounds');
          return buf.subarray(r.addr, r.addr + r.w).some((b) => b !== 0);
        }
        case FBT.UINT: return readInt(r, false);
        case FBT.INT: return readInt(r, true);
        case FBT.FLOAT: {
          if (r.w < 4) fail('flexbuffer: invalid packed type');
          if (r.addr + r.w > n) return 0;
          const f = r.w === 4 ? dv.getFloat32(r.addr, true) : dv.getFloat64(r.addr, true);
          return Number.isFinite(f) ? f : null;
        }
        case FBT.NULL: return null;
        case FBT.STRING: case FBT.KEY: return asStr(r);
        case FBT.BLOB: return fail('flexbuffer: blobs have no JSON form');
        case FBT.MAP: {
          if (3 * r.w >= r.addr) fail('flexbuffer: out of bounds');
          const kw = readUsize(r.addr - 2 * r.w, r.w);
          if (![1, 2, 4, 8].includes(kw)) fail('flexbuffer: invalid keys width');
          const keysOff = readUsize(r.addr - 3 * r.w, r.w);
          if (keysOff > r.addr - 3 * r.w) fail('flexbuffer: out of bounds');
          const keysAddr = r.addr - 3 * r.w - keysOff;
          const len = length(r);
          checkLen(len, r.w);
          const keys = { addr: keysAddr, t: FBT.VEC_KEY, w: kw };
          const entries = [];
          if (++depth > 128) fail('flexbuffer: nesting too deep');
          for (let i = 0; i < len; i++) {
            const kr = element(keys, i, len);
            if (kr.t !== FBT.KEY && kr.t !== FBT.STRING) fail('flexbuffer: map key is not a string');
            entries.push([asStr(kr), value(element(r, i, len))]);
          }
          depth--;
          const sorted = new Map();
          entries.sort((a, b) => compareUtf8(a[0], b[0]));
          for (const [k, v] of entries) sorted.set(k, v);
          return sorted;
        }
        default: {
          if (!isVector(r.t)) fail('flexbuffer: unexpected type');
          const len = length(r);
          checkLen(len, r.w);
          if (++depth > 128) fail('flexbuffer: nesting too deep');
          const out = [];
          for (let i = 0; i < len; i++) out.push(value(element(r, i, len)));
          depth--;
          return out;
        }
      }
    };
    if (n < 3) fail('flexbuffer: too short');
    const rootW = buf[n - 1];
    if (![1, 2, 4, 8].includes(rootW)) fail('flexbuffer: invalid root width');
    const [t, w] = unpack(buf[n - 2]);
    if (rootW > n - 2) fail('flexbuffer: out of bounds');
    return value(reader(n - 2 - rootW, t, w, rootW));
  }

  // ─── zstd ───────────────────────────────────────────────

  // Frame_Content_Size from a zstd frame header, or null if absent.
  function zstdContentSize(b) {
    if (b.length < 6 || b[0] !== 0x28 || b[1] !== 0xb5 || b[2] !== 0x2f || b[3] !== 0xfd) return null;
    const fhd = b[4];
    const single = (fhd >> 5) & 1;
    const fcsFlag = fhd >> 6;
    let p = 5 + (single ? 0 : 1) + [0, 1, 2, 4][fhd & 3];
    const size = [single ? 1 : 0, 2, 4, 8][fcsFlag];
    if (!size || p + size > b.length) return null;
    let v = 0;
    for (let k = size - 1; k >= 0; k--) v = v * 256 + b[p + k];
    return size === 2 ? v + 256 : v;
  }

  // Adapter over @bokuweb/zstd-wasm's `decompress` / `decompressUsingDict`.
  // That library sizes its output buffer from the frame header and falls
  // back to a fixed guess when the size is absent (icechunk's streaming
  // writer omits it), so retry with larger buffers on "destination too small".
  function zstdFromLib(lib) {
    const DST_TOO_SMALL = /code -70\b/;
    let dctx = null;
    const run = (fn, data, limit, initial) => {
      const declared = zstdContentSize(data);
      if (declared !== null && declared > limit) fail(`zstd frame declares ${declared} bytes, over the ${limit}-byte limit`);
      let size = declared !== null ? declared : Math.min(limit, initial);
      for (;;) {
        try {
          return fn(data, { defaultHeapSize: Math.max(size, 1) });
        } catch (e) {
          if (declared === null && DST_TOO_SMALL.test(String(e && e.message)) && size < limit) {
            size = Math.min(limit, size * 4);
            continue;
          }
          throw e;
        }
      }
    };
    return {
      decompress(data, limit = MAX_DECOMPRESSED) {
        return run((d, o) => lib.decompress(d, o), data, limit, Math.max(1 << 20, data.length * 8));
      },
      decompressWithDict(data, dict, limit = MAX_LOCATION_LEN) {
        if (dctx === null) dctx = lib.createDCtx();
        return run((d, o) => lib.decompressUsingDict(dctx, d, dict, o), data, limit, limit);
      },
    };
  }

  // ─── Values ─────────────────────────────────────────────
  //
  // Tagged decoded values (`k` is the kind); `toPlain` turns them into the
  // exact JSON shape core-drill emits.

  const V = {
    bool: (v) => ({ k: 'bool', v }),
    int: (v) => ({ k: 'int', v }),
    float: (v) => ({ k: 'float', v }),
    str: (v) => ({ k: 'str', v }),
  };

  function toPlain(v) {
    switch (v.k) {
      case 'bool': return v.v;
      case 'int': return toBig(v.v);
      case 'float': return Number.isFinite(v.v) ? v.v : null;
      case 'str': return v.v;
      case 'enum': return v.name !== null ? v.name : toBig(v.value);
      case 'time': return new Map([['value', toBig(v.raw)], ['time', v.iso]]);
      case 'id': return new Map([['id', v.id], ['links', v.links.slice()]]);
      case 'node': return new Map([['node_id', v.id], ['path', v.path]]);
      case 'bytes': {
        const m = new Map([['len', BigInt(v.len)], ['hex', hex(v.data.subarray(0, Math.min(v.len, PREVIEW_BYTES)))]]);
        if (v.decoded) m.set('decoded', v.decoded.json !== undefined ? v.decoded.json : v.decoded.text);
        return m;
      }
      case 'table': {
        const m = new Map([['_type', v.type]]);
        for (const [name, x] of v.fields) m.set(name, toPlain(x));
        return m;
      }
      case 'vector':
        return new Map([['len', BigInt(v.len)], ['start', BigInt(v.start)], ['items', v.items.map(toPlain)]]);
      case 'scalars': return v.items.map(toPlain);
      case 'error': return new Map([['error', v.message]]);
      default: throw new Error(`unknown value kind ${v.k}`);
    }
  }

  // ─── Annotations ────────────────────────────────────────

  class Annotator {
    constructor({ nodePaths = new Map(), locationDictionary = null, zstd = null } = {}) {
      this.nodePaths = nodePaths;
      this.locationDictionary = locationDictionary;
      this.zstd = zstd;
    }

    objectId(owner, field, bytes) {
      const id = encodeId(bytes);
      if (bytes.length === 8) {
        const path = this.nodePaths.get(id);
        return { k: 'node', id, path: path === undefined ? null : path };
      }
      let dirs = [];
      if ((['Snapshot', 'TransactionLog', 'SnapshotInfo'].includes(owner) && field === 'id') ||
          (owner === 'Snapshot' && field === 'parent_id') ||
          field === 'new_snap_id' || field === 'previous_snap_id') dirs = ['snapshots', 'transactions'];
      else if (owner === 'SnapshotInfo' && field === 'pruned_ancestor_tx_logs') dirs = ['transactions'];
      else if ((['Manifest', 'ManifestFileInfo', 'ManifestFileInfoV2'].includes(owner) && field === 'id') ||
               (owner === 'ManifestRef' && field === 'object_id')) dirs = ['manifests'];
      else if (owner === 'ChunkRef' && field === 'chunk_id') dirs = ['chunks'];
      return { k: 'id', id, links: dirs.map((d) => `${d}/${id}`) };
    }

    scalar(owner, field, value) {
      if (value.k !== 'int' || value.unsigned !== true) return value;
      const raw = toBig(value.v);
      let iso = null;
      if ((['Snapshot', 'SnapshotInfo'].includes(owner) && field === 'flushed_at') ||
          (owner === 'RepoStatus' && field === 'set_at') ||
          (owner === 'Update' && field === 'updated_at')) {
        iso = rfc3339(floorDiv(BigInt.asIntN(64, raw), 1000000n));
      } else if (owner === 'ChunkRef' && field === 'checksum_last_modified') {
        iso = rfc3339(BigInt.asIntN(64, raw));
      }
      return iso === null ? value : { k: 'time', raw: value.v, iso };
    }

    string(owner, field, s) {
      if ((owner === 'Update' && field === 'backup_path') || (owner === 'Repo' && field === 'repo_before_updates')) {
        return { k: 'id', id: s, links: [`overwritten/${s}`] };
      }
      return V.str(s);
    }

    bytes(owner, field, data) {
      let decoded;
      if ((owner === 'MetadataItem' && field === 'value') || (owner === 'Repo' && field === 'config')) decoded = flexbufferJson(data);
      else if (owner === 'NodeSnapshot' && field === 'user_data') decoded = utf8Json(data);
      else if (owner === 'ChunkRef' && field === 'compressed_location') decoded = this.decompressLocation(data);
      return { k: 'bytes', len: data.length, data, decoded };
    }

    decompressLocation(data) {
      if (this.locationDictionary === null) return undefined;
      if (!this.zstd) return { text: '<decompression failed: zstd is not loaded>' };
      try {
        const out = this.zstd.decompressWithDict(data, this.locationDictionary, MAX_LOCATION_LEN);
        return { text: sanitize(utf8Lossy.decode(out)) };
      } catch (e) {
        return { text: `<decompression failed: ${e && e.message ? e.message : e}>` };
      }
    }
  }

  function flexbufferJson(data) {
    if (!data.length) return undefined;
    try { return { json: sanitizeJson(readFlexbuffer(data)) }; } catch (e) {
      if (e instanceof DecodeError || e instanceof RangeError) return undefined;
      throw e;
    }
  }

  function utf8Json(data) {
    if (!data.length) return undefined;
    const text = strictUtf8(data);
    if (text === null) return undefined;
    try { return { json: sanitizeJson(parseJson(text)) }; } catch (e) {
      if (!(e instanceof DecodeError)) throw e;
    }
    return { text: sanitize(text) };
  }

  // ─── Header & opening ───────────────────────────────────

  function parseHeader(bytes) {
    if (bytes.length < HEADER_LEN) return null;
    for (let i = 0; i < MAGIC.length; i++) if (bytes[i] !== MAGIC[i]) return null;
    const implementation = sanitize(utf8Lossy.decode(bytes.subarray(12, 36)).replace(RUST_WS, ''));
    const fileType = bytes[37], compression = bytes[38];
    return {
      implementation,
      spec_version: bytes[36],
      file_type: fileType,
      file_type_name: FILE_TYPE_NAMES[fileType] || 'unknown',
      compression,
      compression_name: compression === 0 ? 'none' : compression === 1 ? 'zstd' : 'unknown',
    };
  }

  function kindFromPath(path) {
    const dir = path.split('/')[0];
    return { repo: 'RepoInfo', overwritten: 'RepoInfo', snapshots: 'Snapshot', manifests: 'Manifest',
      transactions: 'TransactionLog', chunks: 'Chunk' }[dir] || null;
  }

  // ─── Schema & walker ────────────────────────────────────

  const SCALAR_SIZE = { Bool: 1, Byte: 1, UByte: 1, UType: 1, Short: 2, UShort: 2, Int: 4, UInt: 4, Float: 4, Long: 8, ULong: 8, Double: 8 };

  function prepareSchema(raw) {
    const norm = (t = {}) => ({
      base_type: t.base_type || 'None',
      element: t.element || 'None',
      index: t.index === undefined ? -1 : t.index,
      fixed_length: t.fixed_length || 0,
    });
    const objects = raw.objects.map((o) => {
      const fields = (o.fields || []).map((f) => ({ name: f.name, type: norm(f.type), id: f.id || 0, offset: f.offset || 0 }));
      const byId = fields.slice().sort((a, b) => a.id - b.id);
      return { name: o.name, short: o.name.split('.').pop(), is_struct: !!o.is_struct, bytesize: o.bytesize || 0, fields, byId,
        byName: new Map(fields.map((f) => [f.name, f])) };
    });
    const enums = raw.enums.map((e) => ({ name: e.name,
      values: (e.values || []).map((v) => ({ name: v.name, value: v.value || 0, union_type: v.union_type ? norm(v.union_type) : null })) }));
    const roots = {};
    for (const [kind, name] of Object.entries(ROOT_OBJECT)) roots[kind] = objects.findIndex((o) => o.short === name);
    return { objects, enums, roots };
  }

  // A selector such as `arrays/0/refs/10..20`.
  function parseSelector(s) {
    return s.split('/').filter((p) => p !== '').map((p) => {
      const r = p.indexOf('..');
      if (r >= 0) {
        const a = p.slice(0, r), b = p.slice(r + 2);
        if ((a && !/^\d+$/.test(a)) || (b && !/^\d+$/.test(b))) fail(`invalid range ${p}`);
        const start = a ? Number(a) : 0, end = b ? Number(b) : Infinity;
        if (end < start) fail(`empty range ${p}`);
        return { range: [start, end] };
      }
      if (/^\d+$/.test(p)) return { index: Number(p) };
      return { field: p };
    });
  }

  class Walker {
    constructor(buf, schema, annotator, { maxItems = 0, lenient = false } = {}) {
      this.buf = buf;
      this.dv = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
      this.schema = schema;
      this.annotator = annotator;
      this.maxItems = maxItems;
      this.lenient = lenient;
    }

    rootTable(kind) {
      const idx = this.schema.roots[kind];
      if (idx === undefined || idx < 0) fail('schema has no root table');
      return [this.schema.objects[idx], this.u32(0)];
    }

    walkRoot(kind, sel) {
      const [obj, loc] = this.rootTable(kind);
      return this.table(obj, loc, sel, 0);
    }

    fieldPos(tableLoc, field) {
      const vtable = tableLoc - this.i32(tableLoc);
      if (vtable < 0) fail('vtable offset out of range');
      const vtableLen = this.u16(vtable);
      const slot = field.offset;
      if (slot + 2 > vtableLen) return null;
      const off = this.u16(vtable + slot);
      return off === 0 ? null : tableLoc + off;
    }

    bytesField(obj, tableLoc, name) {
      const field = obj.byName.get(name);
      if (!field) return null;
      const pos = this.fieldPos(tableLoc, field);
      if (pos === null) return null;
      const vec = this.follow(pos);
      return this.slice(vec + 4, this.u32(vec));
    }

    guard(fn) {
      if (!this.lenient) return fn();
      try { return fn(); } catch (e) {
        if (e instanceof DecodeError || e instanceof RangeError) return { k: 'error', message: e.message };
        throw e;
      }
    }

    table(obj, loc, sel, depth) {
      if (depth > MAX_DEPTH) fail(`nesting deeper than ${MAX_DEPTH} tables`);
      const typeName = obj.short;
      if (sel.length) {
        const [first, ...rest] = sel;
        if (first.field === undefined) fail(`${typeName} is a table; select a field name, not ${segText(first)}`);
        const field = obj.byName.get(first.field);
        if (!field) fail(`${typeName} has no field '${first.field}' (fields: ${fieldNames(obj).join(', ')})`);
        const pos = this.fieldPos(loc, field);
        if (pos === null) fail(`field '${first.field}' is not set in this ${typeName}`);
        return this.fieldValue(obj, loc, field, pos, rest, depth);
      }
      const fields = [];
      for (const field of obj.byId) {
        if (field.type.base_type === 'UType') continue;
        const entry = this.guard(() => {
          const pos = this.fieldPos(loc, field);
          return pos === null ? null : this.fieldValue(obj, loc, field, pos, [], depth);
        });
        if (entry !== null) fields.push([field.name, entry]);
      }
      return { k: 'table', type: typeName, fields };
    }

    fieldValue(obj, tableLoc, field, pos, sel, depth) {
      const ty = field.type;
      const owner = obj.short;
      const fname = field.name;
      const leaf = (v) => {
        if (sel.length) fail(`'${fname}' is a scalar; cannot select ${segText(sel[0])} inside it`);
        return v;
      };
      switch (ty.base_type) {
        case 'String':
          return leaf(this.annotator.string(owner, fname, this.stringAt(this.follow(pos))));
        case 'Obj': {
          const child = this.object(ty.index);
          return child.is_struct ? this.structure(child, pos, owner, fname, sel) : this.table(child, this.follow(pos), sel, depth + 1);
        }
        case 'Union': {
          const tagField = obj.byName.get(`${fname}_type`);
          if (!tagField) fail(`union '${fname}' has no _type field`);
          const tagPos = this.fieldPos(tableLoc, tagField);
          const tag = tagPos === null ? 0 : this.u8(tagPos);
          if (tag === 0) fail(`union '${fname}' has a value but its type is NONE`);
          const enumDef = this.schema.enums[ty.index];
          const variant = enumDef && enumDef.values.find((v) => v.value === tag);
          if (!variant || !variant.union_type) fail(`union '${fname}' has unknown type tag ${tag}`);
          return this.table(this.object(variant.union_type.index), this.follow(pos), sel, depth + 1);
        }
        case 'Vector':
          return this.vector(ty, this.follow(pos), owner, fname, sel, depth);
        default:
          return leaf(this.scalarField(ty, pos, owner, fname));
      }
    }

    vector(ty, loc, owner, fname, sel, depth) {
      const len = this.u32(loc);
      const data = loc + 4;
      const elem = ty.element;
      if (elem === 'UByte' || elem === 'Byte') {
        if (sel.length) fail(`'${fname}' is a byte vector; cannot select ${segText(sel[0])} inside it`);
        return this.annotator.bytes(owner, fname, this.slice(data, len));
      }
      let structObj = null;
      if (elem === 'Obj') {
        const o = this.object(ty.index);
        if (o.is_struct) structObj = o;
      }
      let stride;
      if (structObj) stride = structObj.bytesize;
      else if (elem === 'Obj' || elem === 'String') stride = 4;
      else {
        stride = SCALAR_SIZE[elem];
        if (!stride) fail(`unsupported vector element in '${fname}'`);
      }
      const element = (i, rest) => {
        const p = data + i * stride;
        if (structObj) return this.structure(structObj, p, owner, fname, rest);
        if (elem === 'Obj') return this.table(this.object(ty.index), this.follow(p), rest, depth + 1);
        if (elem === 'String') return V.str(this.stringAt(this.follow(p)));
        return this.scalar(elem, p);
      };
      const isScalar = !structObj && elem !== 'Obj' && elem !== 'String';
      let start, end;
      const first = sel[0];
      if (first && first.index !== undefined) {
        if (first.index >= len) fail(`index ${first.index} out of range for '${fname}' (len ${len})`);
        return element(first.index, sel.slice(1));
      } else if (first && first.range) {
        if (sel.length > 1) fail('a range must be the last selector step');
        start = Math.min(first.range[0], len);
        end = Math.min(first.range[1], len);
      } else if (first) {
        fail(`'${fname}' is a vector; select an index, not '${first.field}'`);
      } else if (isScalar || this.maxItems === 0) {
        start = 0; end = len;
      } else {
        start = 0; end = Math.min(len, this.maxItems);
      }
      if (isScalar) {
        const items = [];
        for (let i = start; i < end; i++) items.push(element(i, []));
        return { k: 'scalars', items };
      }
      const items = [];
      for (let i = start; i < end; i++) items.push(this.lenient ? this.guard(() => element(i, [])) : element(i, []));
      return { k: 'vector', len, start, items, window: (a, b) => {
        const out = [];
        for (let i = a; i < Math.min(b, len); i++) out.push(this.guard(() => element(i, [])));
        return out;
      } };
    }

    structure(obj, loc, owner, fname, sel) {
      const typeName = obj.short;
      if (typeName === 'ObjectId12' || typeName === 'ObjectId8') {
        if (sel.length) fail(`'${fname}' is an ID; cannot select ${segText(sel[0])} inside it`);
        return this.annotator.objectId(owner, fname, this.slice(loc, obj.bytesize));
      }
      if (sel.length && sel[0].field !== undefined) {
        const field = obj.byId.find((f) => f.name === sel[0].field);
        if (!field) fail(`${typeName} has no field '${sel[0].field}'`);
        if (sel.length > 1) fail(`cannot select inside struct field '${sel[0].field}'`);
        return this.structField(typeName, field, loc + field.offset);
      }
      if (sel.length) fail(`${typeName} is a struct; select a field name, not ${segText(sel[0])}`);
      return { k: 'table', type: typeName, fields: obj.byId.map((f) => [f.name, this.structField(typeName, f, loc + f.offset)]) };
    }

    structField(owner, field, pos) {
      const ty = field.type;
      if (ty.base_type === 'Obj') return this.structure(this.object(ty.index), pos, owner, field.name, []);
      if (ty.base_type === 'Array') {
        const size = SCALAR_SIZE[ty.element];
        if (!size) fail('unsupported array element');
        const items = [];
        for (let i = 0; i < ty.fixed_length; i++) items.push(this.scalar(ty.element, pos + i * size));
        return { k: 'scalars', items };
      }
      return this.scalarField(ty, pos, owner, field.name);
    }

    scalarField(ty, pos, owner, fname) {
      const v = this.scalar(ty.base_type, pos);
      if (ty.index >= 0) {
        if (v.k !== 'int') return v;
        const value = BigInt.asIntN(64, toBig(v.v));
        const enumDef = this.schema.enums[ty.index];
        const e = enumDef && enumDef.values.find((x) => BigInt(x.value) === value);
        return { k: 'enum', name: e ? e.name : null, value: fromBig(value) };
      }
      return this.annotator.scalar(owner, fname, v);
    }

    scalar(bt, pos) {
      const u = (v) => ({ k: 'int', v, unsigned: true });
      const s = (v) => ({ k: 'int', v });
      switch (bt) {
        case 'Bool': return V.bool(this.u8(pos) !== 0);
        case 'UType': case 'UByte': return u(this.u8(pos));
        case 'Byte': this.check(pos, 1); return s(this.dv.getInt8(pos));
        case 'Short': this.check(pos, 2); return s(this.dv.getInt16(pos, true));
        case 'UShort': return u(this.u16(pos));
        case 'Int': return s(this.i32(pos));
        case 'UInt': return u(this.u32(pos));
        case 'Long': this.check(pos, 8); return s(fromBig(this.dv.getBigInt64(pos, true)));
        case 'ULong': this.check(pos, 8); return u(fromBig(this.dv.getBigUint64(pos, true)));
        case 'Float': this.check(pos, 4); return V.float(this.dv.getFloat32(pos, true));
        case 'Double': this.check(pos, 8); return V.float(this.dv.getFloat64(pos, true));
        default: return fail(`unsupported scalar type ${bt}`);
      }
    }

    object(index) {
      if (!(index >= 0 && index < this.schema.objects.length)) fail(`schema object index ${index} out of range`);
      return this.schema.objects[index];
    }

    follow(pos) { return pos + this.u32(pos); }

    stringAt(loc) {
      const len = this.u32(loc);
      return sanitize(utf8Lossy.decode(this.slice(loc + 4, len)));
    }

    check(pos, len) {
      if (!(pos >= 0 && len >= 0 && pos + len <= this.buf.length)) {
        fail(`read of ${len} bytes at offset ${pos} is past the end of the buffer`);
      }
    }

    slice(pos, len) { this.check(pos, len); return this.buf.subarray(pos, pos + len); }
    u8(pos) { this.check(pos, 1); return this.buf[pos]; }
    u16(pos) { this.check(pos, 2); return this.dv.getUint16(pos, true); }
    u32(pos) { this.check(pos, 4); return this.dv.getUint32(pos, true); }
    i32(pos) { this.check(pos, 4); return this.dv.getInt32(pos, true); }
  }

  function segText(s) {
    if (s.field !== undefined) return `Field("${s.field}")`;
    if (s.index !== undefined) return `Index(${s.index})`;
    return `Range(${s.range[0]}, ${s.range[1]})`;
  }

  const fieldNames = (obj) => obj.byId.filter((f) => f.type.base_type !== 'UType').map((f) => f.name);

  // ─── Opened files ───────────────────────────────────────

  class OpenedFile {
    constructor(ctx, path, kind, storedBytes, header, body) {
      this.ctx = ctx;
      this.path = path;
      this.kind = kind;
      this.storedBytes = storedBytes;
      this.header = header;
      this.body = body;
      this.locationDictionary = undefined;
    }

    get hasSchema() { return this.header !== null && ROOT_OBJECT[this.kind] !== undefined; }

    dictionary() {
      if (this.locationDictionary === undefined) {
        this.locationDictionary = null;
        if (this.kind === 'Manifest' && this.hasSchema) {
          const w = new Walker(this.body, this.ctx.schema, new Annotator());
          const [root, loc] = w.rootTable(this.kind);
          this.locationDictionary = w.bytesField(root, loc, 'location_dictionary');
        }
      }
      return this.locationDictionary;
    }

    // Decode the file (or the part named by `selector`). Mirrors
    // `OpenedFile::walk`; `lenient` turns field-level errors into error values
    // instead of failing the whole decode.
    walk({ selector = null, maxItems = 0, nodePaths = new Map(), lenient = false } = {}) {
      const sel = selector && selector.replace(/^\/+|\/+$/g, '') !== '' ? selector : null;
      const segs = sel ? parseSelector(sel) : [];
      const annotator = new Annotator({ nodePaths, zstd: this.ctx.zstd });
      let value;
      if (!this.hasSchema) {
        value = annotator.bytes('', '', this.body);
      } else {
        annotator.locationDictionary = this.dictionary();
        value = new Walker(this.body, this.ctx.schema, annotator, { maxItems, lenient }).walkRoot(this.kind, segs);
      }
      return { path: this.path, kind: this.kind, stored_bytes: this.storedBytes, header: this.header,
        decoded_bytes: this.header ? this.body.length : null, selector: sel, value };
    }

    select(selector) { return this.walk({ selector }).value; }

    vecLen(selector) {
      const v = this.select(`${selector}/0..0`);
      if (v.k === 'vector') return v.len;
      fail(v.k === 'scalars' ? `'${selector}' is not a vector of tables` : `'${selector}' is not a vector`);
    }

    // `(array index, ref index)` of the ref for `coords` in node `nodeId`'s
    // array manifest, or null. Same probes as `find_chunk_ref`.
    findChunkRef(nodeId, coords) {
      const array = binarySearch(this.vecLen('arrays'), (i) => {
        const v = this.select(`arrays/${i}/node_id`);
        if (v.k !== 'node') fail('unexpected node_id value');
        return v.id < nodeId ? -1 : v.id > nodeId ? 1 : 0;
      });
      if (array === null) return null;
      const refs = `arrays/${array}/refs`;
      const found = binarySearch(this.vecLen(refs), (j) => {
        const v = this.select(`${refs}/${j}/index`);
        if (v.k !== 'scalars') fail('unexpected chunk index value');
        const index = v.items.map((x) => {
          if (x.k !== 'int' || !x.unsigned) fail('unexpected chunk coordinate');
          return Number(BigInt.asUintN(32, toBig(x.v)));
        });
        return compareCoords(index, coords);
      });
      return found === null ? null : [array, found];
    }
  }

  function compareCoords(a, b) {
    const n = Math.min(a.length, b.length);
    for (let i = 0; i < n; i++) if (a[i] !== b[i]) return a[i] < b[i] ? -1 : 1;
    return a.length - b.length < 0 ? -1 : a.length > b.length ? 1 : 0;
  }

  function binarySearch(len, cmp) {
    let lo = 0, hi = len;
    while (lo < hi) {
      const mid = lo + Math.floor((hi - lo) / 2);
      const c = cmp(mid);
      if (c === 0) return mid;
      if (c < 0) lo = mid + 1; else hi = mid;
    }
    return null;
  }

  // ─── Repository context ─────────────────────────────────
  //
  // Everything that needs more than one file: parents, branch tips, node
  // paths, chunk-ref lookups, and chunk-change classification. `fetchBytes`
  // is `async (repoRelativePath) => Uint8Array`.

  class RepoContext {
    constructor(ctx, fetchBytes) {
      this.ctx = ctx;
      this.fetchBytes = fetchBytes;
      this.files = new Map();
      this.repoInfoPromise = null;
      this.nodeManifestCache = new Map();
      this.nodePathCache = new Map();
    }

    async open(path) {
      if (!checkKey(path)) fail(`invalid object path '${path}'`);
      if (!this.files.has(path)) {
        const p = this.fetchBytes(path).then((bytes) => this.ctx.open(path, bytes));
        this.files.set(path, p);
        p.catch(() => this.files.delete(path));
      }
      return this.files.get(path);
    }

    // Snapshot list from the repo info file: [{id, parentIndex, message}],
    // plus branches and tags as name → snapshot id.
    repoInfo() {
      if (!this.repoInfoPromise) {
        this.repoInfoPromise = this.open('repo').then((file) => {
          const v = file.walk().value;
          const field = (t, name) => { const f = t.fields.find(([n]) => n === name); return f ? f[1] : undefined; };
          const snapshots = (field(v, 'snapshots') || { items: [] }).items.map((s) => {
            const po = field(s, 'parent_offset');
            const msg = field(s, 'message');
            const at = field(s, 'flushed_at');
            return { id: field(s, 'id').id, parentIndex: po ? Number(po.v) : 0, message: msg ? msg.v : '',
              flushedAt: at && at.k === 'time' ? at.iso : null };
          });
          const refs = (name) => new Map((field(v, name) || { items: [] }).items.map((r) => {
            const idx = field(r, 'snapshot_index');
            const snap = snapshots[idx ? Number(idx.v) : 0];
            return [field(r, 'name').v, snap ? snap.id : null];
          }));
          return { snapshots, branches: refs('branches'), tags: refs('tags') };
        });
        this.repoInfoPromise.catch(() => { this.repoInfoPromise = null; });
      }
      return this.repoInfoPromise;
    }

    async parentOf(snapshotId) {
      const info = await this.repoInfo();
      const s = info.snapshots.find((x) => x.id === snapshotId);
      if (!s) fail(`snapshot ${snapshotId} is not in the repo info file`);
      if (s.parentIndex < 0) return null;
      const p = info.snapshots[s.parentIndex];
      if (!p) fail(`snapshot ${snapshotId} has parent index ${s.parentIndex}, past the end of the snapshot list`);
      return p.id;
    }

    async branchTip(name) {
      const info = await this.repoInfo();
      return info.branches.get(name) || null;
    }

    async snapshotNodes(snapshotId) {
      const file = await this.open(`snapshots/${snapshotId}`);
      const v = file.walk().value;
      const nodes = v.fields.find(([n]) => n === 'nodes');
      return nodes ? nodes[1].items : [];
    }

    // Node ID → path for one snapshot.
    async nodePaths(snapshotId) {
      if (!this.nodePathCache.has(snapshotId)) {
        const p = this.snapshotNodes(snapshotId).then((nodes) => {
          const m = new Map();
          for (const node of nodes) {
            const id = node.fields.find(([n]) => n === 'id')[1].id;
            const path = node.fields.find(([n]) => n === 'path')[1].v;
            if (!m.has(id)) m.set(id, path);
          }
          return m;
        });
        this.nodePathCache.set(snapshotId, p);
        p.catch(() => this.nodePathCache.delete(snapshotId));
      }
      return this.nodePathCache.get(snapshotId);
    }

    // Node paths for labeling a file: a transaction log uses its own
    // snapshot and that snapshot's parent; a manifest uses `context` or the
    // `main` branch tip. Mirrors `node_context`.
    async nodeContext(file, context = null) {
      const paths = new Map();
      const add = (m) => { for (const [k, v] of m) if (!paths.has(k)) paths.set(k, v); };
      if (file.kind === 'TransactionLog') {
        // Labels are best-effort: any failure leaves the log unlabeled, and a
        // missing parent (V1 repos have no repo info file) is skipped.
        const id = file.path.split('/').pop();
        try {
          const own = await this.nodePaths(id);
          let parent = null;
          try { parent = await this.parentOf(id); } catch (_) { parent = null; }
          const parentPaths = parent ? await this.nodePaths(parent) : new Map();
          add(own);
          add(parentPaths);
        } catch (_) {
          paths.clear();
        }
      } else if (file.kind === 'Manifest') {
        let snap = context;
        if (!snap) { try { snap = await this.branchTip('main'); } catch (_) { snap = null; } }
        if (snap) add(await this.nodePaths(snap));
      }
      return paths;
    }

    async nodeManifests(snapshotId) {
      if (!this.nodeManifestCache.has(snapshotId)) {
        const p = this.snapshotNodes(snapshotId).then((nodes) => {
          const m = new Map();
          for (const node of nodes) {
            const get = (t, n) => { const f = t.fields.find(([x]) => x === n); return f ? f[1] : undefined; };
            const data = get(node, 'node_data');
            if (!data || data.type !== 'ArrayNodeData') continue;
            const manifests = (get(data, 'manifests') || { items: [] }).items.map((ref) => ({
              id: get(ref, 'object_id').id,
              extents: get(ref, 'extents').items.map((r) => [Number(get(r, 'from').v), Number(get(r, 'to').v)]),
            }));
            m.set(get(node, 'id').id, manifests);
          }
          return m;
        });
        this.nodeManifestCache.set(snapshotId, p);
        p.catch(() => this.nodeManifestCache.delete(snapshotId));
      }
      return this.nodeManifestCache.get(snapshotId);
    }

    // Where the ref for `coords` of node `nodeId` lives in `snapshotId`:
    // {manifest, at, chunk_ref}, or null when no manifest holds one.
    async locate(snapshotId, nodeId, coords) {
      const ms = (await this.nodeManifests(snapshotId)).get(nodeId);
      if (!ms) return null;
      // Same check as icechunk's `ManifestExtents::contains` (a zip, so extra
      // coordinates are ignored).
      const candidates = ms.filter((m) => m.extents.every((r, i) => i >= coords.length || (coords[i] >= r[0] && coords[i] < r[1])));
      for (const m of candidates) {
        const file = await this.open(`manifests/${m.id}`);
        const hit = file.findChunkRef(nodeId, coords);
        if (hit) {
          const at = `arrays/${hit[0]}/refs/${hit[1]}`;
          return { manifest: m.id, at, chunk_ref: file.select(at) };
        }
      }
      return null;
    }

    // Port of `fetch_chunk_ref` for a snapshot ID.
    async chunkRef(snapshotId, arrayPath, coords) {
      const nodes = await this.snapshotNodes(snapshotId);
      const node = nodes.find((n) => n.fields.find(([x]) => x === 'path')[1].v === arrayPath);
      if (!node) fail(`no node at '${arrayPath}' in snapshot ${snapshotId}`);
      const data = node.fields.find(([x]) => x === 'node_data');
      if (!data || data[1].type !== 'ArrayNodeData') fail(`'${arrayPath}' is a group, not an array`);
      const nodeId = node.fields.find(([x]) => x === 'id')[1].id;
      return { snapshot: snapshotId, path: arrayPath, node_id: nodeId, coords: coords.slice(),
        location: await this.locate(snapshotId, nodeId, coords) };
    }

    // One coordinate's change between `parent` (null for the first commit)
    // and `snapshotId`: added, deleted, overwritten, rewritten (same ref
    // written again), or absent (in the log but in neither manifest).
    async classify(parent, snapshotId, nodeId, coords) {
      const before = parent ? await this.locate(parent, nodeId, coords) : null;
      const after = await this.locate(snapshotId, nodeId, coords);
      let kind;
      if (!before && after) kind = 'added';
      else if (before && !after) kind = 'deleted';
      else if (!before && !after) kind = 'absent';
      else kind = stringify(toPlain(before.chunk_ref)) === stringify(toPlain(after.chunk_ref)) ? 'rewritten' : 'overwritten';
      return { coords, kind, before, after };
    }

    // Port of `fetch_chunk_changes`: classify every coordinate the commit's
    // transaction log lists by looking it up before and after. The log
    // lists the coordinate without the kind of change, and a manifest has no
    // entry for a deleted chunk.
    async chunkChanges(snapshotId, { pathFilter = null, limit = 0, onProgress = null } = {}) {
      const parent = await this.parentOf(snapshotId);
      const snapFile = await this.open(`snapshots/${snapshotId}`);
      const snapValue = snapFile.walk({ maxItems: 1 }).value;
      const msg = snapValue.fields.find(([n]) => n === 'message');
      const tx = (await this.open(`transactions/${snapshotId}`)).walk().value;
      const paths = new Map(await this.nodePaths(snapshotId));
      if (parent) for (const [k, v] of await this.nodePaths(parent)) if (!paths.has(k)) paths.set(k, v);

      const updated = tx.fields.find(([n]) => n === 'updated_chunks');
      const arrays = [];
      let remaining = limit === 0 ? Infinity : limit;
      let truncated = false;
      let done = 0;
      for (const entry of updated ? updated[1].items : []) {
        const nodeId = entry.fields.find(([n]) => n === 'node_id')[1].id;
        const path = paths.has(nodeId) ? paths.get(nodeId) : null;
        if (pathFilter !== null && path !== pathFilter) continue;
        const coordsList = entry.fields.find(([n]) => n === 'chunks')[1].items.map((c) =>
          c.fields.find(([n]) => n === 'coords')[1].items.map((x) => Number(x.v)));
        const changes = [];
        for (const coords of coordsList) {
          if (remaining === 0) { truncated = true; break; }
          remaining--;
          changes.push(await this.classify(parent, snapshotId, nodeId, coords));
          if (onProgress) onProgress(++done);
        }
        arrays.push({ path, node_id: nodeId, listed: coordsList.length, changes });
      }
      return { snapshot: snapshotId, parent, message: msg ? msg[1].v : '', arrays, truncated };
    }
  }

  function changesToPlain(c) {
    const loc = (l) => (l ? new Map([['manifest', l.manifest], ['at', l.at], ['chunk_ref', toPlain(l.chunk_ref)]]) : null);
    return new Map([
      ['snapshot', c.snapshot], ['parent', c.parent], ['message', c.message],
      ['arrays', c.arrays.map((a) => new Map([
        ['path', a.path], ['node_id', a.node_id], ['listed', BigInt(a.listed)],
        ['changes', a.changes.map((ch) => new Map([
          ['coords', ch.coords.map(BigInt)], ['kind', ch.kind], ['before', loc(ch.before)], ['after', loc(ch.after)],
        ]))],
      ]))],
      ['truncated', c.truncated],
    ]);
  }

  // ─── Entry point ────────────────────────────────────────

  function create({ schema, zstd = null }) {
    const ctx = { schema: prepareSchema(schema), zstd };
    ctx.open = (path, bytes) => {
      const header = parseHeader(bytes);
      const kind = (header && KIND_FROM_HEADER[header.file_type]) || kindFromPath(path);
      if (!kind) fail(`cannot tell what kind of file '${path}' is`);
      let body = bytes;
      if (header && ROOT_OBJECT[kind]) {
        const raw = bytes.subarray(HEADER_LEN);
        if (header.compression === 0) body = raw;
        else if (header.compression === 1) {
          if (!zstd) fail('zstd decompressor is not loaded');
          try { body = zstd.decompress(raw, MAX_DECOMPRESSED); } catch (e) {
            fail(`zstd decompression failed: ${e && e.message ? e.message : e}`);
          }
        } else fail(`unknown compression code ${header.compression}`);
      }
      return new OpenedFile(ctx, path, kind, bytes.length, header, body);
    };
    return {
      open: ctx.open,
      repo: (fetchBytes) => new RepoContext(ctx, fetchBytes),
    };
  }

  return {
    create, zstdFromLib, toPlain, changesToPlain, stringify, parseJson, sanitize, encodeId, decodeId, isObjectId,
    checkKey, parseHeader, readFlexbuffer, hex, DecodeError, HEADER_LEN, PREVIEW_BYTES,
  };
});
