// Vite serves this file as the module worker entry. It re-exports the binding
// worker so imports resolve to ../pkg and ../src beside the binding root.
import "../src/worker.js";
