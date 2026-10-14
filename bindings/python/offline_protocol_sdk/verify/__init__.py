"""``offline-protocol-verify``: drive a running service and read the proof.

Every networking property the engine has, it reports as an event on the
local API: ``message_deferred`` while the recipient is away,
``message_relayed`` on a device that carries a frame for someone else,
``message_received`` with its ``hop_count`` and ``transport`` on the far
side, and ``message_delivered`` back on the sender, which is the
recipient's own acknowledgement and names the carrier it arrived on. This
package sends through the local API and waits for those events, so a
scenario passes or fails on what the engine says rather than on what an
observer reads in a log.

It is a client of the local API like any other (``docs/spec/local-api.md``
holds for it unchanged), runs on the device whose service it talks to, and
needs only the ``websockets`` package the SDK depends on. Guide:
``docs/local-api.md``.
"""

from .client import VerifyClient

__all__ = ["VerifyClient"]
