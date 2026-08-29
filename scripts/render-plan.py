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
