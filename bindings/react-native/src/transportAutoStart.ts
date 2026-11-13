/**
 * Which configured transports `start()` connects on its own.
 *
 * `start()` and `forScenario` both decide this, and they must agree: a preset
 * that accepted a config `start()` would not connect builds an app that is
 * silently offline on the transport it was chosen for. Both read these
 * predicates, so neither can drift from the other.
 */

import type { InternetTransportConfig, NostrTransportConfig } from './types';

/** Internet config `start()` connects: enabled and naming a server. */
export function internetAutoConnects(
  internet: InternetTransportConfig | undefined
): internet is InternetTransportConfig & { serverAddress: string } {
  return Boolean(internet?.enabled && internet.serverAddress);
}

/** Nostr config `start()` enables: enabled and naming at least one relay. */
export function nostrAutoConnects(
  nostr: NostrTransportConfig | undefined
): nostr is NostrTransportConfig & { relayUrls: string[] } {
  return Boolean(nostr?.enabled && nostr.relayUrls?.length);
}
