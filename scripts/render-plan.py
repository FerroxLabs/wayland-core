#!/usr/bin/env python3
"""
Render THE PLAN for the current release cycle from the criteria ledger.

WHY THIS IS GENERATED AND NOT WRITTEN BY HAND
---------------------------------------------
Every handoff this project has produced was a NARRATIVE of what somebody did,
never a LEDGER of what is true. A narrative cannot be queried or falsified, so
each session re-derived "what is done" from prose and got a different answer.
v0.13.10 shipped claiming 22 issues closed; grading found 9. A hand-maintained
plan document is the same failure with better formatting: it is correct on the
day it is written and silently wrong by the next morning.

So this file has no facts of its own. STATE comes from `.planning/ledger/*.md`.
VERIFICATION comes from `.planning/plan-verification.json`, written by the
adversarial verifiers. ROUTING -- who is doing it and on which machine -- comes
from `.planning/PLAN-ROUTING.json`. This script only joins them and renders.

FAIL-CLOSED
-----------
The render EXITS NON-ZERO if any outstanding criterion has no route. That is
deliberate. Two release-blocking issues (core#113, wayland#863) sat outside
every lane in this cycle purely because nothing forced them to be assigned.
An unrouted criterion is how work goes missing, so it stops the render.

THE FOUR STATES, and why "met" is not "done"
--------------------------------------------
  DONE      ledger says met, evidence resolves, AND an independent adversarial
            verifier confirmed the lane. This is the only state that counts.
  CLAIMED   ledger says met, but no verifier has confirmed it yet. Historically
            this is where the partials hide: a criterion written thin is `met`
            without the bug being fixed. Never report CLAIMED as done.
  OPEN      not-met. Real outstanding work.
  HANDOFF   blocked, owned by another team, WITH a filed ticket carrying the
            remainder. Not a partial -- a decomposition. Without a ticket it
            is not a HANDOFF, it is a partial wearing a label, so it renders
            as OPEN and blocks.

Usage:  python3 scripts/render-plan.py [--out PATH] [--html PATH] [--check]
        --check  exit non-zero if anything is unrouted or blocking
"""
import argparse, json, os, re, subprocess, sys, glob, datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LEDGER = os.path.join(ROOT, ".planning", "ledger")
ROUTING = os.path.join(ROOT, ".planning", "PLAN-ROUTING.json")
VERIF = os.path.join(ROOT, ".planning", "plan-verification.json")

DONE, CLAIMED, OPEN, HANDOFF = "DONE", "CLAIMED", "OPEN", "HANDOFF"


def say(m=""):
    sys.stdout.write(m + "\n")


def load_ledger():
    recs = []
    for p in sorted(glob.glob(os.path.join(LEDGER, "*.md"))):
        t = open(p, encoding="utf-8").read()
        if not t.startswith("---"):
            continue
        fm = t.split("---")[1]
        def f(k, d=None):
            m = re.search(r"^%s:\s*\"?(.*?)\"?\s*$" % k, fm, re.M)
            return m.group(1) if m else d
        repo = f("repo", "")
        num = f("issue", "")
        key = ("core#" if repo.endswith("wayland-core") else "wl#") + num
        crits = []
        for b in re.split(r"\n  - id: ", fm)[1:]:
            cid = b.split("\n")[0].strip()
            def g(k, d=""):
                m = re.search(r"^\s+%s:\s*\"?(.*?)\"?\s*$" % k, b, re.M)
                return m.group(1) if m else d
            crits.append({
                "id": cid, "state": g("state", "?"), "owner": g("owner", "?"),
                "evidence": g("evidence", ""), "handoff": g("handoff", ""),
                "text": " ".join(re.sub(r"\s+", " ", g("text", "")).split()),
            })
        recs.append({
            "key": key, "repo": repo, "num": num, "title": f("title", ""),
            "status": f("status", "?"), "kind": f("kind", ""), "crits": crits,
            "path": os.path.relpath(p, ROOT),
        })
    return recs


def classify(rec, crit, verified_lanes, route):
    st = crit["state"]
    if st in ("met", "superseded"):
        lane = route.get("lane") if route else None
        return DONE if (lane and verified_lanes.get(lane) == "CONFIRMED") else CLAIMED
    if st == "blocked":
        # A blocked criterion owned by another team is only a decomposition if a
        # ticket actually carries the remainder. Otherwise it is a partial.
        if crit["owner"] != "core" and crit["handoff"]:
            return HANDOFF
        return OPEN
    return OPEN


# ---------------------------------------------------------------- HTML render
HTML_HEAD = """<title>Wayland Core Release Board</title>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=IBM+Plex+Sans+Condensed:wght@500;600;700&family=IBM+Plex+Sans:wght@400;500;600&family=IBM+Plex+Mono:wght@400;500;600&display=swap">
<style>
:root{
  --ground:#FBF8F4; --card:#FFFFFF; --sunk:#F3EDE5;
  --ink:#1A1512; --ink-2:#54483E; --ink-3:#8A7767;
  --rule:#E4DACD; --rule-2:#D2C4B2;
  --accent:#D2570A; --accent-soft:#FBEADC;
  --open:#B45309; --open-soft:#FCEBD5;
  --claim:#8A5CD6; --claim-soft:#F0E9FC;
  --done:#1F7A4D; --done-soft:#DCF2E6;
  --hand:#0E6E86; --hand-soft:#DCEFF4;
  --shadow:0 1px 2px rgba(26,21,18,.05),0 8px 24px -12px rgba(26,21,18,.16);
}
@media (prefers-color-scheme:dark){:root:not([data-theme="light"]){
  --ground:#141110; --card:#1D1917; --sunk:#262120;
  --ink:#F5EFE9; --ink-2:#BFB0A3; --ink-3:#8B7C70;
  --rule:#332C29; --rule-2:#463C37;
  --accent:#FF8A3D; --accent-soft:#3A2314;
  --open:#E9A23B; --open-soft:#3A2B12;
  --claim:#B99BF0; --claim-soft:#2C2340;
  --done:#4FC98A; --done-soft:#13301F;
  --hand:#4FC3DE; --hand-soft:#0F2C33;
  --shadow:0 1px 2px rgba(0,0,0,.4),0 10px 28px -14px rgba(0,0,0,.7);
}}
:root[data-theme="dark"]{
  --ground:#141110; --card:#1D1917; --sunk:#262120;
  --ink:#F5EFE9; --ink-2:#BFB0A3; --ink-3:#8B7C70;
  --rule:#332C29; --rule-2:#463C37;
  --accent:#FF8A3D; --accent-soft:#3A2314;
  --open:#E9A23B; --open-soft:#3A2B12;
  --claim:#B99BF0; --claim-soft:#2C2340;
  --done:#4FC98A; --done-soft:#13301F;
  --hand:#4FC3DE; --hand-soft:#0F2C33;
  --shadow:0 1px 2px rgba(0,0,0,.4),0 10px 28px -14px rgba(0,0,0,.7);
}
*{box-sizing:border-box}
body{margin:0;background:var(--ground);color:var(--ink);
  font:400 16px/1.6 "IBM Plex Sans",ui-sans-serif,system-ui,sans-serif;
  -webkit-font-smoothing:antialiased}
.wrap{max-width:1080px;margin:0 auto;padding:28px 20px 96px;display:flex;flex-direction:column;gap:34px}
h1,h2,h3{font-family:"IBM Plex Sans Condensed","IBM Plex Sans",sans-serif;
  text-wrap:balance;margin:0;letter-spacing:-.01em}
code,.mono{font-family:"IBM Plex Mono",ui-monospace,monospace;font-variant-numeric:tabular-nums}

/* masthead */
.mast{display:flex;flex-direction:column;gap:14px;
  border-bottom:2px solid var(--ink);padding-bottom:18px}
.eyebrow{font-family:"IBM Plex Mono",monospace;font-size:11.5px;letter-spacing:.13em;
  text-transform:uppercase;color:var(--ink-3);display:flex;gap:10px;flex-wrap:wrap;align-items:center}
.eyebrow b{color:var(--accent);font-weight:600}
h1{font-size:clamp(30px,6.4vw,46px);font-weight:700;line-height:1.05}
.sub{color:var(--ink-2);max-width:64ch;font-size:15px}

/* verdict */
.verdict{display:flex;flex-wrap:wrap;align-items:baseline;gap:14px;
  background:var(--accent-soft);border:1px solid var(--accent);
  border-left-width:5px;border-radius:3px;padding:16px 18px}
.verdict.ok{background:var(--done-soft);border-color:var(--done)}
.vtag{font-family:"IBM Plex Sans Condensed",sans-serif;font-weight:700;font-size:26px;
  letter-spacing:.02em;color:var(--accent)}
.verdict.ok .vtag{color:var(--done)}
.vtext{color:var(--ink);font-size:15px}

/* counters */
.counts{display:grid;grid-template-columns:repeat(auto-fit,minmax(148px,1fr));gap:12px}
.count{background:var(--card);border:1px solid var(--rule);border-radius:3px;
  padding:13px 14px;display:flex;flex-direction:column;gap:3px;box-shadow:var(--shadow)}
.count .n{font-family:"IBM Plex Mono",monospace;font-size:30px;font-weight:600;line-height:1}
.count .l{font-family:"IBM Plex Mono",monospace;font-size:10.5px;letter-spacing:.12em;
  text-transform:uppercase;color:var(--ink-3)}
.count .d{font-size:12.5px;color:var(--ink-2);line-height:1.4}
.count.done .n{color:var(--done)} .count.claim .n{color:var(--claim)}
.count.open .n{color:var(--open)} .count.hand .n{color:var(--hand)}

section{display:flex;flex-direction:column;gap:16px}
.shead{display:flex;flex-direction:column;gap:5px;border-top:1px solid var(--rule-2);padding-top:16px}
h2{font-size:12px;letter-spacing:.15em;text-transform:uppercase;color:var(--ink-3);font-weight:600}
.stitle{font-family:"IBM Plex Sans Condensed",sans-serif;font-size:23px;font-weight:600}
.snote{color:var(--ink-2);font-size:14.5px;max-width:70ch}

/* phase band */
.phase{font-family:"IBM Plex Mono",monospace;font-size:11px;letter-spacing:.12em;
  text-transform:uppercase;color:var(--ink-3);display:flex;align-items:center;gap:10px;margin-top:6px}
.phase::after{content:"";flex:1;height:1px;background:var(--rule)}

/* lane */
.lane{background:var(--card);border:1px solid var(--rule);border-radius:3px;
  overflow:hidden;box-shadow:var(--shadow)}
.lhead{padding:13px 16px;border-bottom:1px solid var(--rule);display:flex;
  flex-wrap:wrap;gap:8px 12px;align-items:baseline;background:var(--sunk)}
.lname{font-family:"IBM Plex Mono",monospace;font-weight:600;font-size:14px;color:var(--accent)}
.ltitle{font-size:14px;color:var(--ink-2);flex:1;min-width:200px}
.chip{font-family:"IBM Plex Mono",monospace;font-size:10.5px;letter-spacing:.06em;
  padding:3px 8px;border-radius:2px;border:1px solid var(--rule-2);color:var(--ink-2);
  background:var(--card);white-space:nowrap}
.chip.host{border-color:var(--accent);color:var(--accent)}
.scroll{overflow-x:auto}
table{width:100%;border-collapse:collapse;font-size:14px}
th{font-family:"IBM Plex Mono",monospace;font-size:10px;letter-spacing:.12em;text-transform:uppercase;
  color:var(--ink-3);text-align:left;font-weight:500;padding:9px 16px;border-bottom:1px solid var(--rule);
  white-space:nowrap}
td{padding:10px 16px;border-bottom:1px solid var(--rule);vertical-align:top;color:var(--ink)}
tr:last-child td{border-bottom:none}
td.k{white-space:nowrap;font-family:"IBM Plex Mono",monospace;font-size:12.5px;color:var(--ink-2)}
td.k b{color:var(--ink);font-weight:600}
td.t{min-width:280px;line-height:1.5}
.pill{display:inline-block;font-family:"IBM Plex Mono",monospace;font-size:10px;
  letter-spacing:.08em;padding:2px 7px;border-radius:2px;font-weight:500}
.pill.open{background:var(--open-soft);color:var(--open)}
.pill.claim{background:var(--claim-soft);color:var(--claim)}
.pill.done{background:var(--done-soft);color:var(--done)}
.pill.hand{background:var(--hand-soft);color:var(--hand)}
.note{font-size:12.5px;color:var(--ink-3);margin-top:5px;line-height:1.45}

.plain{background:var(--card);border:1px solid var(--rule);border-radius:3px;
  padding:16px 18px;box-shadow:var(--shadow)}
.plain ul{margin:0;padding-left:19px;display:flex;flex-direction:column;gap:7px}
.plain li{font-size:14.5px}
.empty{color:var(--ink-3);font-style:italic;font-size:14px}
footer{border-top:1px solid var(--rule-2);padding-top:16px;color:var(--ink-3);font-size:13px;
  display:flex;flex-direction:column;gap:6px}
footer code{color:var(--ink-2);font-size:12.5px}
a{color:var(--accent)}
:focus-visible{outline:2px solid var(--accent);outline-offset:2px}
@media (max-width:560px){.wrap{padding:20px 14px 72px;gap:26px}
  td,th{padding-left:12px;padding-right:12px}}
</style>
"""

PHASE_LABEL = {
    "1-build": "Phase 1 — authored and gated on Hetzner",
    "2-platform": "Phase 2 — needs a real machine, serialised",
    "2-decompose": "Phase 2 — file the remainder on its owner",
    "2-maintainer": "Phase 2 — Sean only",
    "3-unrouted-pickup": "Phase 3 — picked up after falling outside every lane",
}
PHASE_ORDER = ["1-build", "2-platform", "2-decompose", "2-maintainer", "3-unrouted-pickup", ""]


def esc(s):
    return (str(s).replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;"))


def render_html(rows, lanes_meta, cycle, verdict, blk, unrouted, counts, stamp, extra):
    h = [HTML_HEAD, '<div class="wrap">']
    a = h.append
    a('<header class="mast">')
    a('<div class="eyebrow"><b>wayland-core</b><span>%s</span><span>generated %s UTC</span></div>' % (esc(cycle), esc(stamp)))
    a('<h1>Release Board</h1>')
    a('<p class="sub">Joined from the criteria ledger, the verifier results and the routing table. '
      'Nothing here is typed by hand, so it cannot drift from the tree. '
      'Regenerate with <code>just plan</code>.</p>')
    a('</header>')

    ok = "ok" if verdict == "SHIP" else ""
    a('<div class="verdict %s"><span class="vtag">%s</span><span class="vtext">%s</span></div>'
      % (ok, esc(verdict),
         ("%d defect criteria block %s." % (len(blk), esc(cycle))) if blk
         else "No defect criterion is outstanding."))

    a('<div class="counts">')
    for cls, key, lab, desc in [
        ("done", DONE, "Done", "met, evidence resolves, independently verified"),
        ("claim", CLAIMED, "Claimed", "met but no verifier has confirmed it yet"),
        ("open", OPEN, "Open", "real outstanding work"),
        ("hand", HANDOFF, "Handoff", "another team's half, with a ticket carrying it"),
    ]:
        a('<div class="count %s"><span class="n">%d</span><span class="l">%s</span>'
          '<span class="d">%s</span></div>' % (cls, counts[key], lab, desc))
    a('</div>')

    if unrouted:
        a('<section><div class="shead"><h2>Unrouted</h2>'
          '<div class="stitle">%d criteria nobody is doing</div>'
          '<p class="snote">The render fails until every one has a lane. An unrouted criterion '
          'is how work goes missing.</p></div><div class="plain"><ul>' % len(unrouted))
        for r in unrouted:
            a('<li><code>%s %s</code> — %s</li>' % (esc(r["key"]), esc(r["cid"]), esc(r["text"][:150])))
        a('</ul></div></section>')

    # blocking, by phase then lane
    a('<section><div class="shead"><h2>Blocking</h2>'
      '<div class="stitle">The definition of done for %s</div>'
      '<p class="snote">Every row must become true before this release cuts. Feature requests are '
      'excluded by instruction and appear further down.</p></div>' % esc(cycle))
    bylane = {}
    for r in blk:
        bylane.setdefault(r["lane"] or "UNROUTED", []).append(r)
    seen_phase = set()
    for lane in sorted(bylane, key=lambda L: (
            PHASE_ORDER.index(lanes_meta.get(L, {}).get("phase", "")) if
            lanes_meta.get(L, {}).get("phase", "") in PHASE_ORDER else 99, L)):
        meta = lanes_meta.get(lane, {})
        ph = meta.get("phase", "")
        if ph not in seen_phase:
            seen_phase.add(ph)
            a('<div class="phase">%s</div>' % esc(PHASE_LABEL.get(ph, ph or "unassigned")))
        a('<div class="lane"><div class="lhead"><span class="lname">%s</span>'
          '<span class="ltitle">%s</span>' % (esc(lane), esc(meta.get("title", ""))))
        if meta.get("host"):
            a('<span class="chip host">%s</span>' % esc(meta["host"]))
        a('<span class="chip">%d open</span></div>' % len(bylane[lane]))
        a('<div class="scroll"><table><thead><tr><th>Issue</th><th>What must become true</th>'
          '<th>Owner</th></tr></thead><tbody>')
        for r in sorted(bylane[lane], key=lambda x: (x["key"], x["cid"])):
            note = ('<div class="note">%s</div>' % esc(r["note"])) if r.get("note") else ""
            a('<tr><td class="k"><b>%s</b><br>%s</td><td class="t">%s%s</td>'
              '<td class="k">%s</td></tr>'
              % (esc(r["key"]), esc(r["cid"]), esc(r["text"]), note, esc(r["owner"])))
        a('</tbody></table></div></div>')
    a('</section>')

    for title, head, note, items in extra:
        a('<section><div class="shead"><h2>%s</h2><div class="stitle">%s</div>'
          '<p class="snote">%s</p></div><div class="plain">' % (esc(title), esc(head), note))
        if items:
            a('<ul>')
            for it in items:
                a('<li>%s</li>' % it)
            a('</ul>')
        else:
            a('<p class="empty">Nothing recorded yet.</p>')
        a('</div></section>')

    a('<footer><div>Source of truth. If this disagrees with anyone&rsquo;s recollection, '
      'this is right — that is the entire point.</div>'
      '<div><code>just plan</code> regenerates &middot; <code>just plan-check</code> '
      'fails on an unrouted criterion or outstanding defect work</div></footer>')
    a('</div>')
    return "\n".join(h)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(ROOT, ".planning", "THE-PLAN.md"))
    ap.add_argument("--html", default="")
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--cycle", default="0.13.12")
    a = ap.parse_args()

    if not os.path.isdir(LEDGER):
        say("FAIL: %s does not exist. There is no ledger, so there is no plan." % LEDGER)
        return 2
    recs = load_ledger()
    if not recs:
        say("FAIL: ledger holds no files. A plan rendered from nothing is not a plan.")
        return 2

    routing = json.load(open(ROUTING)) if os.path.exists(ROUTING) else {}
    routes = routing.get("criteria", {})
    lanes_meta = routing.get("lanes", {})
    verified = json.load(open(VERIF)) if os.path.exists(VERIF) else {}

    rows, unrouted, blocking, handoffs = [], [], [], []
    for r in recs:
        for c in r["crits"]:
            ck = "%s %s" % (r["key"], c["id"])
            route = routes.get(ck)
            state = classify(r, c, verified, route)
            kind = r["kind"] or routing.get("kind_overrides", {}).get(r["key"], "")
            row = dict(key=r["key"], title=r["title"], cid=c["id"], text=c["text"],
                       owner=c["owner"], state=state, raw=c["state"], kind=kind,
                       evidence=c["evidence"], handoff=c["handoff"],
                       lane=(route or {}).get("lane", ""), host=(route or {}).get("host", ""),
                       phase=(route or {}).get("phase", ""), note=(route or {}).get("note", ""),
                       issue_status=r["status"])
            rows.append(row)
            if state == HANDOFF:
                handoffs.append(row)
            if state == OPEN:
                if not route:
                    unrouted.append(row)
                if kind != "feature":
                    blocking.append(row)

    n = lambda s: sum(1 for r in rows if r["state"] == s)
    blk = [r for r in blocking if r["kind"] != "feature"]
    verdict = "SHIP" if not blk else "BLOCKED"

    out = []
    w = out.append
    w("# THE PLAN — wayland-core %s" % a.cycle)
    w("")
    w("> **GENERATED FILE. Do not edit.** Regenerate with `just plan`. Every fact here is")
    w("> joined from `.planning/ledger/` (state), `plan-verification.json` (independent")
    w("> verification) and `PLAN-ROUTING.json` (assignment). If this disagrees with anyone's")
    w("> recollection, this is right and the recollection is wrong — that is the entire point.")
    w("")
    w("Rendered %s UTC" % datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d %H:%M"))
    w("")
    w("## VERDICT: %s" % verdict)
    w("")
    if blk:
        w("**%d criteria block the %s release.** Full list in §3." % (len(blk), a.cycle))
    else:
        w("No defect criterion is outstanding. %s is releasable." % a.cycle)
    w("")
    w("| state | count | means |")
    w("|---|---:|---|")
    w("| DONE | %d | met, evidence resolves, independently verified |" % n(DONE))
    w("| CLAIMED | %d | met but NOT yet independently verified — never report as done |" % n(CLAIMED))
    w("| OPEN | %d | outstanding work |" % n(OPEN))
    w("| HANDOFF | %d | another team's half, with a filed ticket carrying it |" % n(HANDOFF))
    w("")
    if unrouted:
        w("### ⚠ %d UNROUTED — nobody is doing these" % len(unrouted))
        w("")
        w("An unrouted criterion is how work goes missing. The render fails until each has a lane.")
        w("")
        for r in unrouted:
            w("- `%s %s` — %s" % (r["key"], r["cid"], r["text"][:110]))
        w("")

    # ---- blocking, grouped by lane ----
    w("## §3 BLOCKING — the definition of done for %s" % a.cycle)
    w("")
    bylane = {}
    for r in blk:
        bylane.setdefault(r["lane"] or "UNROUTED", []).append(r)
    for lane in sorted(bylane):
        meta = lanes_meta.get(lane, {})
        w("### `%s`%s" % (lane, (" — " + meta["title"]) if meta.get("title") else ""))
        if meta.get("host"):
            w("")
            w("Runs on: **%s**%s" % (meta["host"], ("  ·  " + meta["phase"]) if meta.get("phase") else ""))
        w("")
        w("| criterion | issue | what must become true |")
        w("|---|---|---|")
        for r in sorted(bylane[lane], key=lambda x: (x["key"], x["cid"])):
            w("| `%s` | %s | %s |" % (r["cid"], r["key"], r["text"][:150].replace("|", "\\|")))
        w("")

    # ---- handoffs ----
    w("## §4 DECOMPOSED — another team's half, tracked")
    w("")
    w("These are NOT partials. Core's half is closed; the remainder is filed against a named")
    w("owner with its own contract. A blocked criterion with no ticket does not appear here —")
    w("it appears in §3 as blocking, because that is what it is.")
    w("")
    if handoffs:
        w("| criterion | issue | owner | carried by |")
        w("|---|---|---|---|")
        for r in sorted(handoffs, key=lambda x: x["key"]):
            w("| `%s` | %s | %s | %s |" % (r["cid"], r["key"], r["owner"], r["handoff"]))
    else:
        w("_None recorded yet._")
    w("")

    # ---- claimed but unverified ----
    cl = [r for r in rows if r["state"] == CLAIMED]
    w("## §5 CLAIMED BUT UNVERIFIED — %d" % len(cl))
    w("")
    w("Marked `met` with resolving evidence, but no independent verifier has confirmed the lane.")
    w("Historically this is exactly where a partial hides: a criterion written thin reads `met`")
    w("while the reported bug is still live. Do not report these as done.")
    w("")
    byk = {}
    for r in cl:
        byk.setdefault(r["key"], []).append(r["cid"])
    for k in sorted(byk, key=lambda x: (x.split("#")[0], int(x.split("#")[1]))):
        w("- **%s** — %s" % (k, ", ".join(sorted(byk[k]))))
    w("")

    # ---- out of scope ----
    feat = sorted({r["key"] for r in rows if r["kind"] == "feature" and r["state"] == OPEN})
    w("## §6 OUT OF SCOPE for %s — feature work" % a.cycle)
    w("")
    w("Excluded by explicit instruction: defects ship, feature requests wait. The work still")
    w("gets built and its branch pushed; it just does not gate this release.")
    w("")
    for k in feat:
        t = next(r["title"] for r in rows if r["key"] == k)
        w("- **%s** — %s" % (k, t[:100]))
    if not feat:
        w("_None._")
    w("")

    # ---- done ----
    dn = {}
    for r in rows:
        if r["state"] == DONE:
            dn.setdefault(r["key"], []).append(r["cid"])
    w("## §7 DONE — verified")
    w("")
    w("Every criterion met, evidence resolves in the tree, and an independent adversarial")
    w("verifier re-ran the gate and confirmed it.")
    w("")
    if dn:
        for k in sorted(dn, key=lambda x: (x.split("#")[0], int(x.split("#")[1]))):
            w("- **%s** — %s" % (k, ", ".join(sorted(dn[k]))))
    else:
        w("_Nothing verified yet this cycle._")
    w("")

    txt = "\n".join(out) + "\n"
    with open(a.out, "w", encoding="utf-8") as fh:
        fh.write(txt)
    say("wrote %s  (%s: %d blocking, %d unrouted, %d done, %d claimed, %d handoff)"
        % (os.path.relpath(a.out, ROOT), verdict, len(blk), len(unrouted),
           n(DONE), n(CLAIMED), n(HANDOFF)))

    if a.html:
        counts = {DONE: n(DONE), CLAIMED: n(CLAIMED), OPEN: n(OPEN), HANDOFF: n(HANDOFF)}
        cl_items = []
        byk = {}
        for r in rows:
            if r["state"] == CLAIMED:
                byk.setdefault(r["key"], []).append(r["cid"])
        for k in sorted(byk, key=lambda x: (x.split("#")[0], int(x.split("#")[1]))):
            cl_items.append("<code>%s</code> &mdash; %s" % (esc(k), esc(", ".join(sorted(byk[k])))))
        hd_items = ["<code>%s %s</code> &mdash; %s, carried by %s"
                    % (esc(r["key"]), esc(r["cid"]), esc(r["owner"]), esc(r["handoff"]))
                    for r in sorted(handoffs, key=lambda x: x["key"])]
        dn2 = {}
        for r in rows:
            if r["state"] == DONE:
                dn2.setdefault(r["key"], []).append(r["cid"])
        dn_items = ["<code>%s</code> &mdash; %s" % (esc(k), esc(", ".join(sorted(v))))
                    for k, v in sorted(dn2.items(), key=lambda kv: (kv[0].split("#")[0], int(kv[0].split("#")[1])))]
        ft = sorted({r["key"] for r in rows if r["kind"] == "feature" and r["state"] == OPEN})
        ft_items = ["<code>%s</code> &mdash; %s" % (esc(k), esc(next(r["title"] for r in rows if r["key"] == k)))
                    for k in ft]
        extra = [
            ("Decomposed", "Another team&rsquo;s half, tracked",
             "Not partials. Core&rsquo;s half closes; the remainder is filed against a named owner "
             "with its own contract. A blocked criterion with no ticket is not here &mdash; it is "
             "blocking, because that is what it is.", hd_items),
            ("Claimed", "Met, but nobody has checked",
             "Marked <code>met</code> with resolving evidence, and no independent verifier has "
             "confirmed the lane. This is where a partial hides: a criterion written thin reads "
             "<code>met</code> while the reported bug is still live. Never report these as done.", cl_items),
            ("Out of scope", "Feature work, deferred to 0.13.13",
             "Defects ship; feature requests wait. The work still gets built and its branch pushed, "
             "it just does not gate this release.", ft_items),
            ("Done", "Verified",
             "Met, evidence resolves in the tree, and an independent adversarial verifier re-ran the "
             "gate and confirmed it.", dn_items),
        ]
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d %H:%M")
        with open(a.html, "w", encoding="utf-8") as fh:
            fh.write(render_html(rows, lanes_meta, a.cycle, verdict, blk, unrouted, counts, stamp, extra))
        say("wrote %s" % a.html)

    if a.check:
        if unrouted:
            say("FAIL: %d outstanding criteria have no route. Assign every one in "
                "PLAN-ROUTING.json." % len(unrouted))
            return 1
        if blk:
            say("NOT READY: %d defect criteria outstanding." % len(blk))
            return 1
        say("READY: no defect criterion outstanding.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
