#!/usr/bin/env python3
"""Does a `transport` failure leave billable work on the provider side?

The engine's unserved class re-sends `connection` / `transport` / `http_503` /
`http_529` for up to 900 s (~35 sends). The doc justified that with "nothing
was generated and nothing was billed". `connection` and 503/529 are safe by
construction. `transport` is the open one: the socket WAS established, the
request WAS dispatched, and the peer destroyed it before a response head
arrived. Did the provider already do the work?

The corpus cannot answer this. Its fault proxy returns before ever relaying
upstream (`b2-provider-failure/provider_proxy.py`: `if fault_active(): ...
return`), so a faulted request never reaches the provider — confirmed
numerically in advp-WA, where the proxy ledger says relay=4 fault=10 and the
recorder captured exactly 4 requests.

So model the real shape instead: a load balancer that accepts the request,
forwards it, and then destroys the CLIENT leg. We keep the UPSTREAM leg open
so we can observe what the provider did, which is precisely the observation
the client loses.

  client (this script) -> cutter -> provider
                            |
                            +-- client leg RST after forwarding
                            +-- upstream leg read to completion, usage logged

Prints ONLY token counts and status. Never a credential, never a body.
"""
import http.client
import json
import os
import re
import socket
import ssl
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

TOML = os.environ.get("JOBCORPUS_PROVIDER_TOML", "/dev/shm/b2cred/provider.toml")


def cred():
    """Read base_url / api_key / model without ever printing the key."""
    txt = open(TOML).read()

    def grab(key):
        m = re.search(r'^\s*%s\s*=\s*"([^"]*)"' % key, txt, re.M)
        return m.group(1) if m else None

    return grab("base_url"), grab("api_key"), grab("model")


BASE, KEY, MODEL = cred()
assert BASE and KEY and MODEL, "provider fragment incomplete"
UP = urlparse(BASE)
OBSERVED = {}


def upstream_call(body):
    """Do the real call and report what the provider produced."""
    conn = (http.client.HTTPSConnection(UP.hostname, UP.port or 443,
                                        context=ssl.create_default_context())
            if UP.scheme == "https" else
            http.client.HTTPConnection(UP.hostname, UP.port or 80))
    path = (UP.path.rstrip("/") or "") + "/chat/completions"
    t0 = time.time()
    conn.request("POST", path, body=body, headers={
        "Content-Type": "application/json",
        "Authorization": "Bearer " + KEY,
    })
    resp = conn.getresponse()
    raw = resp.read()
    OBSERVED["status"] = resp.status
    OBSERVED["upstream_seconds"] = round(time.time() - t0, 2)
    try:
        OBSERVED["usage"] = json.loads(raw).get("usage")
    except Exception:
        OBSERVED["usage"] = None
    conn.close()


class Cutter(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n)
        # Forward upstream FIRST, so the provider genuinely receives the work.
        t = threading.Thread(target=upstream_call, args=(body,))
        t.start()
        # Give the request time to land on the provider, then destroy the
        # client leg with a RST - no response head ever reaches the client.
        time.sleep(1.5)
        OBSERVED["client_reset_at"] = time.time()
        try:
            self.connection.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER,
                                       b"\x01\x00\x00\x00\x00\x00\x00\x00")
            self.connection.close()
        except Exception:
            pass
        self.close_connection = True
        t.join(300)


def main():
    srv = ThreadingHTTPServer(("127.0.0.1", 0), Cutter)
    port = srv.server_address[1]
    threading.Thread(target=srv.serve_forever, daemon=True).start()

    payload = json.dumps({
        "model": MODEL,
        "max_tokens": 200,
        "messages": [{"role": "user", "content":
                      "Count from 1 to 40, one number per line."}],
    }).encode()

    # The client leg: exactly what the product's socket would experience.
    client_err = None
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=120)
    try:
        c.request("POST", "/v1/chat/completions", body=payload,
                  headers={"Content-Type": "application/json"})
        r = c.getresponse()
        r.read()
        client_err = "NO ERROR - client got status %d" % r.status
    except Exception as e:
        client_err = "%s: %s" % (type(e).__name__, e)

    for _ in range(300):
        if "usage" in OBSERVED:
            break
        time.sleep(1)

    print("client leg      :", client_err)
    print("upstream status :", OBSERVED.get("status"))
    print("upstream seconds:", OBSERVED.get("upstream_seconds"))
    print("provider usage  :", OBSERVED.get("usage"))
    u = OBSERVED.get("usage") or {}
    done = (u.get("completion_tokens") or 0) > 0
    print()
    print("VERDICT:", "BILLABLE WORK COMPLETED for a request the client "
          "experienced as a transport failure" if done else
          "no completion tokens produced")
    return 0 if "usage" in OBSERVED else 1


if __name__ == "__main__":
    sys.exit(main())
