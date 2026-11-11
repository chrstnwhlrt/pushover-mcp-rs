#!/usr/bin/env python3
"""The kit's fakeapi.py with a fixed clock and counted request IDs, so that
two binaries see byte for byte the same API replies.

    detfake.py KIT_DIR PORT LOG.jsonl
"""
import itertools
import sys
from http.server import ThreadingHTTPServer

KIT, PORT, LOG = sys.argv[1], sys.argv[2], sys.argv[3]
sys.argv = ["fakeapi.py", PORT, LOG]
sys.path.insert(0, KIT)
import fakeapi  # noqa: E402

COUNTER = itertools.count(1)


class FixedTime:
    @staticmethod
    def time():
        return 1794000000.0

    @staticmethod
    def time_ns():
        return next(COUNTER)


fakeapi.time = FixedTime
fakeapi.next_reset = lambda: 1796104800
ThreadingHTTPServer(("127.0.0.1", int(PORT)), fakeapi.Handler).serve_forever()
