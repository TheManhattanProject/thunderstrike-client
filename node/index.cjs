// Loads the prebuilt native addon for this platform.
//
// The published package carries one `.node` per platform and nothing
// else — no Rust, no build step, no compiler on the installing
// machine. `npm install` unpacks the right binary and this file finds
// it.

const { platform, arch } = process;
const local = `./thunderstrike.${platform}-${arch}.node`;

// Two shapes are supported:
//   * everything in one package (what `npm run build` produces here)
//   * one small package per platform, pulled in by optionalDependencies
//     (what a published release uses, so an install downloads ~1 MiB
//     rather than every platform's binary)
const candidates = [local, `thunderstrike-${platform}-${arch}`];

let native;
const tried = [];
for (const candidate of candidates) {
  try {
    native = require(candidate);
    break;
  } catch (err) {
    tried.push(`  ${candidate}: ${err.message.split('\n')[0]}`);
  }
}

if (!native) {
  throw new Error(
    `thunderstrike: no prebuilt binary for ${platform}-${arch}.\n` +
      `Tried:\n${tried.join('\n')}\n` +
      `thunderstrike-${platform}-${arch} is an optional dependency: reinstall ` +
      `without --omit=optional (or --no-optional) if this platform is supported.`
  );
}

// Numeric codes, mirroring thunderstrike_client.h. Positive: the status the
// server sent. Negative: a condition the client detected.
const codes = Object.freeze({
  OK: 0,
  NOT_FOUND: 1,
  WRONG_SHARD: 2,
  CAS_CONFLICT: 3,
  SERVER_OOM: 4,
  NOT_AUTHED: 5,
  UNKNOWN_OP: 6,
  MALFORMED: 7,
  UNSUPPORTED_VERSION: 9,
  RESPONSE_TOO_LARGE: 11,
  STORAGE_EXHAUSTED: 12,
  BUSY: 13,
  SERVER_ERROR: 14,
  UNKNOWN_STATUS: 64,
  ERR_INVALID_ARG: -1,
  ERR_CONFIG: -2,
  ERR_CONNECT: -3,
  ERR_IO: -4,
  ERR_TIMEOUT: -5,
  ERR_DISCONNECTED: -6,
  ERR_PROTOCOL: -7,
  ERR_KEY_TOO_LONG: -8,
  ERR_CLOSED: -9,
  ERR_TLS: -13,
  ERR_VALUE_TOO_LARGE: -14,
});

const RETRY_NEVER = 0;
const RETRY_IF_IDEMPOTENT = 1;
const RETRY_ALWAYS = 2;

// The native layer carries the numeric code in the message as
// "(code N)". Lift it onto the error, with what a caller needs to
// decide on a retry, so nobody parses text.
const CODE_RE = /\(code (-?\d+)\)/;
function decorate(err) {
  if (err && typeof err.message === 'string') {
    const m = CODE_RE.exec(err.message);
    if (m) {
      const code = Number(m[1]);
      err.thunderstrikeCode = code;
      err.retryClass = native.retryClass(code);
      err.busy = code === codes.BUSY;
    }
  }
  return err;
}

const proto = native.ThunderstrikeClient.prototype;
for (const name of Object.getOwnPropertyNames(proto)) {
  const fn = proto[name];
  if (name === 'constructor' || typeof fn !== 'function') continue;
  proto[name] = function (...args) {
    let r;
    try {
      r = fn.apply(this, args);
    } catch (e) {
      throw decorate(e);
    }
    return r && typeof r.then === 'function' ? r.catch((e) => Promise.reject(decorate(e))) : r;
  };
}
const connect = native.ThunderstrikeClient.connect;
native.ThunderstrikeClient.connect = (options) =>
  connect.call(native.ThunderstrikeClient, options).catch((e) => Promise.reject(decorate(e)));

module.exports = {
  ...native,
  ThunderstrikeClient: native.ThunderstrikeClient,
  codes,
  RETRY_NEVER,
  RETRY_IF_IDEMPOTENT,
  RETRY_ALWAYS,
};
