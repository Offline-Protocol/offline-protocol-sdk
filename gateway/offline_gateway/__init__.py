"""The reference gateway daemon for the Offline Protocol SDK.

It implements the device-facing half of ``docs/spec/gateway-contract.md``
(attach, submit, verdict, deliver, presence, capabilities) and, optionally, a
Reticulum backbone between gateways, which is how a LoRa radio reaches the
SDK. See ``gateway/README.md``.
"""

from .backbone import Backbone, BackboneFrame, ForwardResult, InMemoryBackbone, InMemoryNetwork
from .config import ConfigError, GatewayConfig, ReticulumConfig
from .gateway import Gateway

__all__ = [
    "Backbone",
    "BackboneFrame",
    "ConfigError",
    "ForwardResult",
    "Gateway",
    "GatewayConfig",
    "InMemoryBackbone",
    "InMemoryNetwork",
    "ReticulumConfig",
]

__version__ = "0.1.0"
