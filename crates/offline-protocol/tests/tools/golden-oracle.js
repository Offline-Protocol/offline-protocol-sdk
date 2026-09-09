#!/usr/bin/env node
/**
 * Replays `tests/fixtures/pipe-golden-input-v1.json` through the frozen
 * TypeScript client (`@offline-protocol/mesh-analytics`, the oracle the Rust
 * pipe is a port of) and writes the batches it produced to
 * `tests/fixtures/pipe-golden-expected-v1.json`.
 *
 * The oracle is driven with an injected clock, a capturing `fetch`, no
 * storage and no React Native: exactly the pure pipeline. `batch_id` and
 * `session_id` are replaced with ordinals so the file is stable across runs;
 * the Rust test applies the same normalization before comparing.
 *
 * Usage:
 *   cd <mesh-analytics checkout> && npm ci && npm run build
 *   MESH_ANALYTICS_DIST=<checkout>/dist node crates/offline-protocol/tests/tools/golden-oracle.js
 *
 * The oracle never changes (the package is frozen), so this only needs to
 * run again when the scenario in `src/telemetry/pipe/tests/golden.rs` does.
 */
'use strict';

const fs = require('node:fs');
const path = require('node:path');

const dist = process.env.MESH_ANALYTICS_DIST;
if (!dist) {
  console.error('set MESH_ANALYTICS_DIST to the built dist/ of the mesh-analytics checkout');
  process.exit(2);
}
const { Pipeline } = require(path.join(dist, 'listener.js'));
const { HttpFlusher } = require(path.join(dist, 'flusher.js'));

const fixtures = path.resolve(__dirname, '..', 'fixtures');
const input = JSON.parse(fs.readFileSync(path.join(fixtures, 'pipe-golden-input-v1.json'), 'utf8'));

let now = input.start_ms;
const bodies = [];
const fetchImpl = async (_url, init) => {
  bodies.push(JSON.parse(init.body));
  return { status: 202 };
};

const config = { ...input.config };
const pipeline = new Pipeline(config, {
  now: () => now,
  appState: null,
  flusherFactory: (source) =>
    new HttpFlusher({
      config: source.config,
      drain: source.drain,
      now: source.now,
      store: null,
      fetchImpl,
      platform: input.platform,
    }),
});

const settle = () => new Promise((resolve) => setImmediate(resolve));

(async () => {
  for (const step of input.steps) {
    now = step.t_ms;
    switch (step.kind) {
      case 'record':
        pipeline.handle(step.record);
        break;
      case 'app_state':
        // The lifecycle bridge is what the React Native AppState listener
        // called into; driving it directly keeps its edge debounce.
        pipeline.bridge.handleChange(step.state);
        await settle();
        break;
      case 'flush':
        await pipeline.requestFlush();
        break;
      case 'end_session':
        pipeline.emitSessionSummary();
        await pipeline.requestFlush();
        pipeline.rotateSession();
        break;
      default:
        throw new Error(`unknown step kind ${step.kind}`);
    }
  }

  const sessions = new Map();
  const batches = bodies.map((batch, i) => {
    if (!sessions.has(batch.session_id)) {
      sessions.set(batch.session_id, `session-${sessions.size + 1}`);
    }
    return { ...batch, batch_id: `batch-${i + 1}`, session_id: sessions.get(batch.session_id) };
  });
  const out = path.join(fixtures, 'pipe-golden-expected-v1.json');
  fs.writeFileSync(out, JSON.stringify({ batches }, null, 2) + '\n');
  console.log(`wrote ${batches.length} batches to ${out}`);
})().catch((err) => {
  console.error(err);
  process.exit(1);
});
