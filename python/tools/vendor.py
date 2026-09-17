"""Write the single-file copy of the client.

Upstream projects will not take a dependency: Apprise wants a self-contained
plugin module and Uptime Kuma wants an inline fetch, so the same code also
ships as one file somebody can paste into a tree. It is a copy of client.py
with a banner on it, which only works as long as client.py keeps importing
nothing of its own. Run this after changing the client; test_vendor.py fails if
the two drift.
"""

import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SOURCE = ROOT / "mast_pager" / "client.py"
TARGET = ROOT / "vendor" / "mast.py"

BANNER = '''# Vendored copy of the mast-pager client.
#
# Generated from mast_pager/client.py by tools/vendor.py. Edit it there, not
# here. Standard library only, so it can sit in a tree that does not want the
# package: https://github.com/tissue-systems/mast-clients

'''


def render():
    return BANNER + SOURCE.read_text()


def main(argv):
    wanted = render()
    if "--check" in argv:
        found = TARGET.read_text() if TARGET.exists() else ""
        if found != wanted:
            sys.stderr.write("vendor/mast.py is stale: run python tools/vendor.py\n")
            return 1
        return 0
    TARGET.parent.mkdir(exist_ok=True)
    TARGET.write_text(wanted)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
