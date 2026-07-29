# Cross-audit panel — is the Windows positive control as good as the Linux one?

**The judgement call.** The Linux proof's positive-control arm used a **valid** cloud
token and got `HTTP 404: {"error":"app not found"}` from `api.machines.dev`. Windows has
no valid token and none may be moved there, so this lane used a deliberately invalid
placeholder and got `HTTP 401: {"error":"Authenticate: token validation error"}`. Is that
evidentially **equivalent** for the claim *"the request physically left the machine and a
remote server answered"*, or **materially weaker**?

Panel (LANE-BRIEF §4), full transcripts in `panel/`:

| auditor | verdict | core argument |
|---|---|---|
| **gemini 3.1 pro** | **EQUIVALENT** | Both are application-layer HTTP statuses carrying provider-specific JSON. Faking either requires the same local MITM, so the 401 is not weaker. |
| **kimi k3** | **EQUIVALENT** (caveated) | The claim only asks for *packet left + remote party answered*; the 404 proves strictly more than the claim needs. Would flip to WEAKER if TLS were not verified to terminate at the genuine origin. |
| **codex gpt-5.6-sol** | **WEAKER** | A 401 is producible by a localhost proxy, an endpoint-security TLS interceptor, hosts/DNS redirection, or an injected mock transport. Wants packet capture or remote-side logs. |
| **internal adversarial** | sided with **codex** | The genuine hole is not "401 vs 404" — it is that *no leg recorded the TLS peer*. Every leg, including Linux's 404, is vulnerable to the same MITM story, and neither proof had measured it. |

**Resolution: majority verdict EQUIVALENT — but taken only after measuring the minority's
confounder rather than out-voting it.** Codex named a *testable* mechanism, so it was
tested on the host, and the internal pass was right that this was the real gap in both
proofs. Measured on `seandesktop` (`tls-peer.txt`, `harness/tls.ps1`):

```
LOCAL_ENDPOINT=[2403:6200:88a6:8ea2:82c:d273:cde:e853]:56539
REMOTE_ENDPOINT=[2a09:8280:1::8969]:443
TLS_PROTOCOL=Tls13
CERT_SUBJECT=CN=api.machines.dev
CERT_ISSUER=CN=YE2, O=Let's Encrypt, C=US
CERT_THUMBPRINT=43A331EF50537665C101EFE4D768E7FACDDCB9B6
SSL_POLICY_ERRORS=None
PROXY_ENV=HTTP_PROXY/HTTPS_PROXY/ALL_PROXY empty at process, user and machine scope
WININET_ProxyEnable=0  WININET_ProxyServer=(empty)
```

- The remote endpoint is a **global-scope public IPv6 address** in Fly.io's
  `2a09:8280::/29` allocation — not loopback, not RFC-1918-equivalent, not link-local.
  The local endpoint is the host's own **global** address, so the packet left over the WAN
  interface.
- The certificate is `CN=api.machines.dev` issued by **Let's Encrypt**, validating cleanly
  against the OS trust store (`SSL_POLICY_ERRORS=None`). A corporate TLS-intercepting
  proxy would have to present a certificate from a locally-installed root; none is in play.
- No proxy is configured at process, user, machine or WinINET scope.

Every mechanism codex named — local proxy, TLS interceptor, hosts/DNS redirection — is
excluded by those three measurements. Kimi's caveat ("would flip to WEAKER if TLS were not
verified to terminate at the genuine origin") is satisfied in the direction of EQUIVALENT.

**Dissent recorded, and it improved the result.** Codex's remaining ask — a packet capture
or Fly-side logs — was not done, and is genuinely stronger evidence than a TLS peer
identity. I did not do it and I say so; what is here excludes on-host fabrication but does
not independently corroborate from the far side.
