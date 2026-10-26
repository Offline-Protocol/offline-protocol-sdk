// Loads the lobby with no network once it has been opened online (W15).
//
// One build serves the page and the worker: index.js on the page and
// worker.js speak a private message protocol, and the glue loads only the
// module it was built with. So every file is cached under one build id, a
// cache is complete or absent, and a new build never takes over a running
// page: a worker the page restarts after a trap (W5) would load the new
// build under the old page.
//
// A classic worker, so the browser compares the imported script byte for
// byte on every update check: a new build id is a new version.
importScripts("../../bindings/web/pkg/build-id.js");

const CACHE = `web-lobby-${self.BUILD_ID}`;
const SDK = "../../bindings/web/";
// Every file the lobby loads. `the_lobby_caches_every_sdk_file` holds this
// list to bindings/web/src and to the files beside this one: a file missing
// here fails only offline.
const ASSETS = [
  "./",
  "./engine-worker.js",
  "./manifest.webmanifest",
  "./icon.svg",
  "./icon-192.png",
  "./icon-512.png",
  ...["bluetooth", "index", "nearby", "nostr-relays", "nostr-signal", "store",
      "tab-carrier", "webrtc", "worker"].map((file) => `${SDK}src/${file}.js`),
  `${SDK}pkg/offline_protocol_web.js`,
  `${SDK}pkg/offline_protocol_web_bg.wasm`,
];

self.addEventListener("install", (event) => {
  // addAll fails whole if one file fails. "reload" skips the HTTP cache,
  // which could hand back the last build's glue beside this build's module.
  event.waitUntil(caches.open(CACHE).then((cache) =>
    cache.addAll(ASSETS.map((url) => new Request(url, { cache: "reload" })))));
});

self.addEventListener("activate", (event) => {
  event.waitUntil(caches.keys().then((keys) => Promise.all(keys
    .filter((key) => key.startsWith("web-lobby-") && key !== CACHE)
    .map((key) => caches.delete(key)))));
});

// The page's update button. Every page stops its node and reloads once this
// build has taken over (controllerchange), the one that asked included.
self.addEventListener("message", (event) => {
  if (event.data === "activate") self.skipWaiting();
});

self.addEventListener("fetch", (event) => {
  const { request } = event;
  if (request.method !== "GET" || new URL(request.url).origin !== location.origin) return;
  // Only this build's cache answers; anything it lacks goes to the network.
  // ignoreSearch on a navigation, so ?profile=b loads offline too.
  event.respondWith(caches.open(CACHE).then(async (cache) =>
    (await cache.match(request, { ignoreSearch: request.mode === "navigate" })) ?? fetch(request)));
});
