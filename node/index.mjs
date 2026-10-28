// ESM wrapper.
//
// `index.cjs` assigns `module.exports` at runtime, which Node's
// CommonJS lexer cannot see into. Without this file `import {
// ThunderstrikeClient } from ...` fails with "Named export not found", so
// the named exports are re-stated here explicitly.

import native from './index.cjs';

export const ThunderstrikeClient = native.ThunderstrikeClient;
export const shardOf = native.shardOf;
export const version = native.version;
export const retryClass = native.retryClass;
export const describe = native.describe;
export const codes = native.codes;
export const RETRY_NEVER = native.RETRY_NEVER;
export const RETRY_IF_IDEMPOTENT = native.RETRY_IF_IDEMPOTENT;
export const RETRY_ALWAYS = native.RETRY_ALWAYS;

export default native;
