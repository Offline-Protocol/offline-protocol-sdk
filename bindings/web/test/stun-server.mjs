// A minimal STUN server for the browser tests (RFC 8489 Binding only): it
// answers every Binding request with a XOR-MAPPED-ADDRESS naming the
// address it was told to report, not the one the request came from, so a
// page on one machine gathers a server-reflexive candidate distinct from its
// host candidates, as it would behind a real network's NAT. Node's standard
// library only.
//
// Usage: node test/stun-server.mjs <port> <reported-ipv4>

import { createSocket } from "node:dgram";

const port = Number(process.argv[2] ?? 8767);
const reported = (process.argv[3] ?? "203.0.113.50").split(".").map(Number);
const MAGIC = 0x2112a442;

const socket = createSocket("udp4");
socket.on("message", (request, from) => {
  if (request.length < 20 || request.readUInt16BE(0) !== 0x0001) return;
  if (request.readUInt32BE(4) !== MAGIC) return;
  const transaction = request.subarray(8, 20);
  const attribute = Buffer.alloc(12);
  attribute.writeUInt16BE(0x0020, 0); // XOR-MAPPED-ADDRESS
  attribute.writeUInt16BE(8, 2);
  attribute.writeUInt8(0x01, 5); // IPv4
  attribute.writeUInt16BE(from.port ^ (MAGIC >>> 16), 6);
  const address = Buffer.from(reported);
  attribute.writeUInt32BE((address.readUInt32BE(0) ^ MAGIC) >>> 0, 8);
  const header = Buffer.alloc(20);
  header.writeUInt16BE(0x0101, 0); // Binding success response
  header.writeUInt16BE(attribute.length, 2);
  header.writeUInt32BE(MAGIC, 4);
  transaction.copy(header, 8);
  socket.send(Buffer.concat([header, attribute]), from.port, from.address);
});
socket.bind(port, "127.0.0.1", () => console.log(`stun test server on ${port}`));
