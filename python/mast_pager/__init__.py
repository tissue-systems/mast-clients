"""Send to a Mast channel from Python.

    from mast_pager import Channel

    ch = Channel("https://mast.tissue.dev/mk_...")
    sent = ch.page(title="db-01", body="replication stopped", key="db-01")
    if ch.wait(sent.id, timeout=300).acknowledged:
        ...

Everything lives in client.py, which imports nothing outside the standard
library so it can also be dropped into a project as a single file.
"""

from .client import (
    Channel,
    MastError,
    Poller,
    RateLimited,
    Rejected,
    Sent,
    Status,
    Unreachable,
    UnknownChannel,
    UnknownMessage,
    is_message_id,
    parse_channel_url,
)

__version__ = "0.1.0"

__all__ = [
    "Channel",
    "Poller",
    "Sent",
    "Status",
    "MastError",
    "Unreachable",
    "UnknownChannel",
    "UnknownMessage",
    "RateLimited",
    "Rejected",
    "is_message_id",
    "parse_channel_url",
]
