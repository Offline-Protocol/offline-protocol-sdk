#!/usr/bin/env node
'use strict';

/**
 * Unit tests for `src/parseProtocolEvent.ts` (no native runtime).
 */

const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');

const PACKAGE_DIR = path.resolve(__dirname, '..');

function compileSdk() {
  const tsc = path.join(PACKAGE_DIR, 'node_modules', 'typescript', 'bin', 'tsc');
  if (!fs.existsSync(tsc)) {
    throw new Error(`TypeScript not found at ${tsc} — run \`npm ci\` in ${PACKAGE_DIR} first.`);
  }
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'op-rn-parse-ev-'));
  execFileSync(
    process.execPath,
    [tsc, '--outDir', outDir, '--declaration', 'false', '--declarationMap', 'false'],
    { cwd: PACKAGE_DIR, stdio: 'inherit' }
  );
  return outDir;
}

const outDir = compileSdk();
const {
  PROTOCOL_EVENT_TYPES,
  PROTOCOL_EVENT_TYPE_SET,
  parseProtocolEvent,
  tryParseProtocolEvent,
  protocolEventTypeOf,
  isProtocolEventType,
  narrowProtocolEvent,
} = require(path.join(outDir, 'parseProtocolEvent.js'));

let passed = 0;
function test(name, fn) {
  try {
    fn();
    passed += 1;
  } catch (error) {
    console.error(`FAIL: ${name}`);
    throw error;
  }
}

test('catalog matches set size', () => {
  assert.equal(PROTOCOL_EVENT_TYPES.length, PROTOCOL_EVENT_TYPE_SET.size);
});

test('parse known message_received', () => {
  const json = JSON.stringify({ type: 'message_received', message_id: 'm1', sender: 'alice' });
  const result = parseProtocolEvent(json);
  assert.equal(result.ok, true);
  assert.equal(result.event.type, 'message_received');
});

test('reject invalid json', () => {
  const result = parseProtocolEvent('{not json');
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'invalid_json');
});

test('reject missing type', () => {
  const result = parseProtocolEvent(JSON.stringify({ foo: 1 }));
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'missing_type');
});

test('reject unknown type by default', () => {
  const result = parseProtocolEvent(JSON.stringify({ type: 'future_event_tag' }));
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'unknown_type');
});

test('accept unknown type when requireKnownType is false', () => {
  const payload = { type: 'future_event_tag', detail: 1 };
  const result = parseProtocolEvent(JSON.stringify(payload), { requireKnownType: false });
  assert.equal(result.ok, true);
  assert.deepEqual(result.event, payload);
});

test('tryParseProtocolEvent returns null on failure', () => {
  assert.equal(tryParseProtocolEvent('[]'), null);
});

test('protocolEventTypeOf and isProtocolEventType', () => {
  assert.equal(protocolEventTypeOf({ type: 'diagnostic' }), 'diagnostic');
  assert.equal(protocolEventTypeOf([]), null);
  assert.equal(isProtocolEventType('message_sent'), true);
  assert.equal(isProtocolEventType('not_a_real_type'), false);
});

test('narrowProtocolEvent', () => {
  const event = { type: 'message_sent', message_id: 'x', recipient: 'y' };
  const parsed = parseProtocolEvent(JSON.stringify(event));
  assert.equal(parsed.ok, true);
  const narrowed = narrowProtocolEvent(parsed.event, 'message_sent');
  assert.ok(narrowed);
  assert.equal(narrowProtocolEvent(parsed.event, 'message_received'), null);
});

const indexSrc = fs.readFileSync(path.join(PACKAGE_DIR, 'src', 'index.ts'), 'utf8');
test('helpers are exported from package entry', () => {
  assert.match(indexSrc, /parseProtocolEvent/);
  assert.match(indexSrc, /PROTOCOL_EVENT_TYPES/);
});

console.log(`parse-protocol-event.test.js: ${passed} passed`);
