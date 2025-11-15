# Mesh Visualizer

Plain HTML/CSS/JS sketch that mirrors the mesh controller logic from the SDK so
you can see how cluster membership, leader election, and connection limits
behave as nodes join or leave.

## Usage

1. Open `examples/mesh-visualizer/index.html` in any modern browser (no build
   step required).
2. Choose an initial node count and press **Apply**.
3. Use the <kbd>↑</kbd> key to add nodes and <kbd>↓</kbd> to remove nodes. The
   canvas updates instantly to show the new cluster state.

## What you are seeing

- **Leader, member, and bridge roles** are colored to match their state within
  `MeshController`.
- **Edges** respect the `maxConnectionsPerDevice` budget, preferring the leader
  until it is saturated.
- **Rejected nodes** accumulate along the bottom edge whenever the controller
  refuses them because of cluster or connection limits.
- **Remote clusters** appear automatically once the local cluster cannot accept
  more members. Leaders are linked with dashed red bridge edges so you can see
  how clusters fan out.

Metrics jitter every ~1.5s which can trigger a new leader election, replicating
the scoring logic (`rssi`, `battery`, `signalQuality`, `stability`, `hopCount`)
from the Swift/Kotlin implementations.

