package com.offlineprotocol

/**
 * Who may open a peer stream, framework-free. Mirrors iOS's
 * `PeerStreamDialPolicy` (PeerStreamSession.swift), keep in sync.
 */
internal object PeerStreamDialPolicy {
    /**
     * Whether the listener takes a connection from [host], given the hosts of
     * the inbound streams open now and the count of all open streams.
     *
     * The listener binds every interface, so on a shared network anyone on it
     * can connect, not only group members. Two bounds keep that from costing
     * this device its own dials: inbound streams take at most
     * [PeerStreamSockets.Limits.maxInbound] of the budget, and one remote
     * address at most [PeerStreamSockets.Limits.maxInboundPerHost] of those.
     * Unknown hosts (null) share one bound.
     */
    fun admitsInbound(
        host: String?,
        inboundFrom: Collection<String?>,
        open: Int,
        limits: PeerStreamSockets.Limits,
    ): Boolean =
        open < limits.maxStreams &&
            inboundFrom.size < limits.maxInbound &&
            inboundFrom.count { it == host } < limits.maxInboundPerHost
}
