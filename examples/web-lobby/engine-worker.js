// The engine's worker, started from inside the service worker's scope. A
// dedicated worker is matched to a service worker by its own URL, so one
// started from ../../bindings/web/src/worker.js would be fetched from the
// network and fail offline. Its imports and its module are requests of this
// worker, which the scope covers.
import "../../bindings/web/src/worker.js";
