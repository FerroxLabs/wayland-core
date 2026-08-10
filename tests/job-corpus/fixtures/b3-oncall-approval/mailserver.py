#!/usr/bin/env python3
"""B-3 fixture: a hermetic mail host that is the INDEPENDENT OBSERVER.

Two servers in one process, standard library only, no credentials, no network:

  * SMTP  -- accepts mail from the program under test and from the fixture's
             stand-in humans. Every accepted message is written to an
             append-only transcript with its full envelope and body.
  * IMAP4 -- serves those messages back. Every command line is written to the
             same transcript, so "the message was actually fetched" is a fact
             observed by this process, not a claim made by the program.

The transcript is therefore an independent record of both halves of the
conversation: what left the machine, and what came back in.

Accounts: any LOGIN succeeds; the mailbox is the local part of the username.
The fixture uses agent@fixture.local (the program) and oncall@fixture.local
(the human). Passwords are accepted and never written to the transcript.

Usage:
  python3 mailserver.py --smtp-port 2525 --imap-port 2143 \
      --transcript $EVID/mail-transcript.jsonl
"""

from __future__ import annotations

import argparse
import email
import email.utils
import json
import os
import re
import socket
import socketserver
import threading
import time

LOCK = threading.Lock()
CFG = {}
SEQ = {"n": 0, "uid": 1}
MAILBOXES = {}  # local part -> list of message dicts
NEW_MAIL = threading.Condition()


def record(entry):
    with LOCK:
        SEQ["n"] += 1
        entry["seq"] = SEQ["n"]
        entry["ts"] = time.time()
        entry["iso"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(entry["ts"]))
        with open(CFG["transcript"], "a", encoding="utf-8") as fh:
            fh.write(json.dumps(entry, sort_keys=True) + "\n")
            fh.flush()
            os.fsync(fh.fileno())


def local_part(addr):
    addr = email.utils.parseaddr(addr)[1] or addr
    return addr.split("@")[0].strip().lower().strip("<>")


def deliver(sender, recipients, raw):
    msg = email.message_from_string(raw)
    body_parts = []
    if msg.is_multipart():
        for part in msg.walk():
            if part.get_content_maintype() == "text":
                try:
                    body_parts.append(part.get_payload(decode=True).decode("utf-8", "replace"))
                except Exception:
                    pass
    else:
        body_parts.append(msg.get_payload())
    body = "\n".join(body_parts)

    with LOCK:
        for rcpt in recipients:
            box = local_part(rcpt)
            uid = SEQ["uid"]
            SEQ["uid"] += 1
            MAILBOXES.setdefault(box, []).append({
                "uid": uid,
                "flags": set(),
                "raw": raw,
                "internaldate": time.time(),
            })
    record({
        "kind": "delivered",
        "from": sender,
        "to": list(recipients),
        "mailboxes": [local_part(r) for r in recipients],
        "subject": msg.get("Subject", ""),
        "message_id": msg.get("Message-ID", ""),
        "in_reply_to": msg.get("In-Reply-To", ""),
        "body": body,
        "bytes": len(raw),
    })
    with NEW_MAIL:
        NEW_MAIL.notify_all()


# --------------------------------------------------------------------------
# SMTP
# --------------------------------------------------------------------------

class SmtpHandler(socketserver.StreamRequestHandler):
    timeout = 300

    def send(self, line):
        self.wfile.write((line + "\r\n").encode())
        self.wfile.flush()

    def handle(self):
        self.send("220 fixture.local ESMTP jobcorpus")
        sender, rcpts = None, []
        while True:
            try:
                raw = self.rfile.readline()
            except Exception:
                return
            if not raw:
                return
            line = raw.decode("utf-8", "replace").strip()
            up = line.upper()
            if up.startswith("EHLO"):
                self.send("250-fixture.local")
                self.send("250-AUTH PLAIN LOGIN")
                self.send("250-8BITMIME")
                self.send("250 SIZE 20971520")
            elif up.startswith("HELO"):
                self.send("250 fixture.local")
            elif up.startswith("STARTTLS"):
                self.send("454 TLS not available on this fixture")
            elif up.startswith("AUTH LOGIN"):
                self.send("334 VXNlcm5hbWU6")
                self.rfile.readline()
                self.send("334 UGFzc3dvcmQ6")
                self.rfile.readline()
                self.send("235 2.7.0 Authentication successful")
            elif up.startswith("AUTH PLAIN"):
                if len(line.split()) == 2:
                    self.send("334 ")
                    self.rfile.readline()
                self.send("235 2.7.0 Authentication successful")
            elif up.startswith("MAIL FROM"):
                sender = line.split(":", 1)[1].strip() if ":" in line else ""
                rcpts = []
                self.send("250 2.1.0 Ok")
            elif up.startswith("RCPT TO"):
                rcpts.append(line.split(":", 1)[1].strip() if ":" in line else "")
                self.send("250 2.1.5 Ok")
            elif up == "DATA":
                self.send("354 End data with <CR><LF>.<CR><LF>")
                chunks = []
                while True:
                    dl = self.rfile.readline()
                    if not dl:
                        break
                    text = dl.decode("utf-8", "replace")
                    if text.rstrip("\r\n") == ".":
                        break
                    if text.startswith(".."):
                        text = text[1:]
                    chunks.append(text)
                deliver(sender or "", rcpts, "".join(chunks))
                self.send("250 2.0.0 Ok: queued")
            elif up == "RSET":
                sender, rcpts = None, []
                self.send("250 2.0.0 Ok")
            elif up == "NOOP":
                self.send("250 2.0.0 Ok")
            elif up == "QUIT":
                self.send("221 2.0.0 Bye")
                return
            else:
                self.send("502 5.5.2 Not implemented")


# --------------------------------------------------------------------------
# IMAP4rev1 (the subset a mail client actually uses)
# --------------------------------------------------------------------------

LOGIN_RE = re.compile(r"^(\S+)\s+LOGIN\s+(\S+)\s+(.+)$", re.I)


class ImapHandler(socketserver.StreamRequestHandler):
    timeout = 600

    def send(self, line):
        self.wfile.write((line + "\r\n").encode())
        self.wfile.flush()

    def send_bytes(self, blob):
        self.wfile.write(blob)
        self.wfile.flush()

    # -- helpers ----------------------------------------------------------
    def messages(self):
        with LOCK:
            return list(MAILBOXES.get(self.box, []))

    def resolve(self, spec, by_uid):
        msgs = self.messages()
        if not msgs:
            return []
        chosen = []
        for part in spec.split(","):
            part = part.strip()
            if not part:
                continue
            if ":" in part:
                lo, hi = part.split(":", 1)
            else:
                lo = hi = part
            if by_uid:
                top = max(m["uid"] for m in msgs)
                lo_n = top if lo == "*" else int(lo)
                hi_n = top if hi == "*" else int(hi)
                lo_n, hi_n = min(lo_n, hi_n), max(lo_n, hi_n)
                chosen += [(i + 1, m) for i, m in enumerate(msgs)
                           if lo_n <= m["uid"] <= hi_n]
            else:
                top = len(msgs)
                lo_n = top if lo == "*" else int(lo)
                hi_n = top if hi == "*" else int(hi)
                lo_n, hi_n = min(lo_n, hi_n), max(lo_n, hi_n)
                chosen += [(i + 1, m) for i, m in enumerate(msgs)
                           if lo_n <= i + 1 <= hi_n]
        seen, out = set(), []
        for n, m in chosen:
            if m["uid"] not in seen:
                seen.add(m["uid"])
                out.append((n, m))
        return out

    @staticmethod
    def envelope(msg):
        parsed = email.message_from_string(msg["raw"])

        def addr(field):
            vals = email.utils.getaddresses([parsed.get(field, "")])
            if not vals:
                return "NIL"
            items = []
            for name, mail in vals:
                if not mail:
                    continue
                lp, _, dom = mail.partition("@")
                items.append('(%s NIL "%s" "%s")'
                             % ('"%s"' % name if name else "NIL", lp, dom or "fixture.local"))
            return "(" + "".join(items) + ")" if items else "NIL"

        def q(field):
            val = parsed.get(field)
            return '"%s"' % val.replace('"', "'") if val else "NIL"

        return "(%s %s %s %s %s %s NIL NIL %s %s)" % (
            q("Date"), q("Subject"), addr("From"), addr("From"), addr("From"),
            addr("To"), q("In-Reply-To"), q("Message-ID"))

    def fetch_one(self, seqno, msg, items_raw):
        raw = msg["raw"]
        blob = raw.encode("utf-8", "replace")
        parsed = email.message_from_string(raw)
        header_text = "".join("%s: %s\r\n" % (k, v) for k, v in parsed.items()) + "\r\n"
        body_text = parsed.get_payload() if not parsed.is_multipart() else raw.split("\n\n", 1)[-1]

        items = items_raw.strip()
        if items.startswith("(") and items.endswith(")"):
            items = items[1:-1]
        wanted = re.findall(r"BODY\.PEEK\[[^\]]*\]|BODY\[[^\]]*\]|[A-Z0-9\.]+", items.upper())
        pieces = []
        literals = []
        mark_seen = False

        for item in wanted:
            if item == "UID":
                pieces.append("UID %d" % msg["uid"])
            elif item == "FLAGS":
                pieces.append("FLAGS (%s)" % " ".join(sorted(msg["flags"])))
            elif item == "RFC822.SIZE":
                pieces.append("RFC822.SIZE %d" % len(blob))
            elif item == "INTERNALDATE":
                pieces.append('INTERNALDATE "%s"'
                              % email.utils.formatdate(msg["internaldate"], localtime=False))
            elif item == "ENVELOPE":
                pieces.append("ENVELOPE %s" % self.envelope(msg))
            elif item == "BODYSTRUCTURE":
                pieces.append('BODYSTRUCTURE ("TEXT" "PLAIN" ("CHARSET" "UTF-8") NIL NIL '
                              '"7BIT" %d %d)' % (len(blob), raw.count("\n") + 1))
            elif item in ("RFC822", "RFC822.HEADER", "RFC822.TEXT") or item.startswith("BODY"):
                if item == "RFC822.HEADER" or "HEADER" in item:
                    payload = header_text.encode("utf-8", "replace")
                    label = "RFC822.HEADER" if item.startswith("RFC822") else "BODY[HEADER]"
                elif item == "RFC822.TEXT" or "TEXT]" in item:
                    payload = body_text.encode("utf-8", "replace")
                    label = "RFC822.TEXT" if item.startswith("RFC822") else "BODY[TEXT]"
                else:
                    payload = blob
                    label = "RFC822" if item == "RFC822" else "BODY[]"
                if not item.startswith("BODY.PEEK"):
                    mark_seen = True
                literals.append((label, payload))
            else:
                continue

        if mark_seen:
            with LOCK:
                msg["flags"].add("\\Seen")

        head = "* %d FETCH (" % seqno
        parts = list(pieces)
        self.send_bytes(head.encode())
        first = True
        for text in parts:
            self.send_bytes(((" " if not first else "") + text).encode())
            first = False
        for label, payload in literals:
            self.send_bytes(((" " if not first else "") + "%s {%d}\r\n"
                             % (label, len(payload))).encode())
            self.send_bytes(payload)
            first = False
        self.send_bytes(b")\r\n")

    # -- main loop --------------------------------------------------------
    def handle(self):
        self.box = "inbox"
        self.selected = False
        self.send("* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN LOGIN UIDPLUS IDLE] fixture ready")
        while True:
            try:
                raw = self.rfile.readline()
            except (socket.timeout, OSError):
                return
            if not raw:
                return
            line = raw.decode("utf-8", "replace").rstrip("\r\n")
            if not line.strip():
                continue
            safe = re.sub(r"(?i)(LOGIN\s+\S+\s+).*", r"\1<redacted>", line)
            record({"kind": "imap_command", "line": safe, "mailbox": self.box})
            try:
                self.dispatch(line)
            except Exception as exc:  # never take the fixture down mid-run
                tag = line.split(" ", 1)[0]
                record({"kind": "imap_error", "line": safe, "error": repr(exc)})
                self.send("%s BAD %s" % (tag, type(exc).__name__))

    def dispatch(self, line):
        parts = line.split(" ", 2)
        tag = parts[0]
        cmd = parts[1].upper() if len(parts) > 1 else ""
        rest = parts[2] if len(parts) > 2 else ""

        if cmd == "CAPABILITY":
            self.send("* CAPABILITY IMAP4rev1 AUTH=PLAIN LOGIN UIDPLUS IDLE")
            self.send("%s OK CAPABILITY completed" % tag)
        elif cmd == "LOGIN":
            m = LOGIN_RE.match(line)
            user = m.group(2).strip('"') if m else "inbox"
            self.box = local_part(user)
            MAILBOXES.setdefault(self.box, [])
            record({"kind": "imap_login", "mailbox": self.box})
            self.send("%s OK LOGIN completed" % tag)
        elif cmd == "AUTHENTICATE":
            self.send("+ ")
            self.rfile.readline()
            self.send("%s OK AUTHENTICATE completed" % tag)
        elif cmd in ("LIST", "LSUB", "XLIST"):
            self.send('* %s (\\HasNoChildren) "/" "INBOX"' % ("LIST" if cmd != "LSUB" else "LSUB"))
            self.send("%s OK %s completed" % (tag, cmd))
        elif cmd == "STATUS":
            msgs = self.messages()
            unseen = sum(1 for m in msgs if "\\Seen" not in m["flags"])
            self.send('* STATUS "INBOX" (MESSAGES %d UNSEEN %d UIDNEXT %d UIDVALIDITY 1)'
                      % (len(msgs), unseen, SEQ["uid"]))
            self.send("%s OK STATUS completed" % tag)
        elif cmd in ("SELECT", "EXAMINE"):
            msgs = self.messages()
            self.selected = True
            self.send("* FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft)")
            self.send("* OK [PERMANENTFLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft \\*)] ok")
            self.send("* %d EXISTS" % len(msgs))
            self.send("* 0 RECENT")
            self.send("* OK [UIDVALIDITY 1] UIDs valid")
            self.send("* OK [UIDNEXT %d] Predicted next UID" % SEQ["uid"])
            self.send("%s OK [READ-WRITE] %s completed" % (tag, cmd))
        elif cmd == "SEARCH" or (cmd == "UID" and rest.upper().startswith("SEARCH")):
            by_uid = cmd == "UID"
            crit = (rest[len("SEARCH"):] if by_uid else rest).strip().upper() or "ALL"
            msgs = self.messages()
            hits = []
            for i, m in enumerate(msgs, 1):
                if "UNSEEN" in crit and "\\Seen" in m["flags"]:
                    continue
                if "SEEN" in crit and "UNSEEN" not in crit and "\\Seen" not in m["flags"]:
                    continue
                hits.append(m["uid"] if by_uid else i)
            self.send("* SEARCH" + ("".join(" %d" % h for h in hits)))
            self.send("%s OK SEARCH completed" % tag)
        elif cmd == "FETCH" or (cmd == "UID" and rest.upper().startswith("FETCH")):
            by_uid = cmd == "UID"
            body = rest[len("FETCH"):].strip() if by_uid else rest
            spec, _, items = body.partition(" ")
            for seqno, msg in self.resolve(spec, by_uid):
                if by_uid and "UID" not in items.upper():
                    items = items.rstrip()
                    items = ("(" + items.strip("()") + " UID)") if items else "(UID)"
                self.fetch_one(seqno, msg, items)
            self.send("%s OK FETCH completed" % tag)
        elif cmd == "STORE" or (cmd == "UID" and rest.upper().startswith("STORE")):
            by_uid = cmd == "UID"
            body = rest[len("STORE"):].strip() if by_uid else rest
            spec, _, action = body.partition(" ")
            flags = re.findall(r"\\\w+", action)
            for seqno, msg in self.resolve(spec, by_uid):
                with LOCK:
                    if action.strip().startswith("-"):
                        for f in flags:
                            msg["flags"].discard(f)
                    else:
                        msg["flags"].update(flags)
                self.send("* %d FETCH (FLAGS (%s)%s)"
                          % (seqno, " ".join(sorted(msg["flags"])),
                             " UID %d" % msg["uid"] if by_uid else ""))
            self.send("%s OK STORE completed" % tag)
        elif cmd == "IDLE":
            self.send("+ idling")
            before = len(self.messages())
            deadline = time.time() + 60
            while time.time() < deadline:
                with NEW_MAIL:
                    NEW_MAIL.wait(1.0)
                if len(self.messages()) != before:
                    self.send("* %d EXISTS" % len(self.messages()))
                    break
            self.rfile.readline()  # DONE
            self.send("%s OK IDLE terminated" % tag)
        elif cmd in ("NOOP", "CHECK", "SUBSCRIBE", "UNSUBSCRIBE", "CLOSE", "EXPUNGE"):
            self.send("%s OK %s completed" % (tag, cmd))
        elif cmd == "LOGOUT":
            self.send("* BYE fixture signing off")
            self.send("%s OK LOGOUT completed" % tag)
            raise SystemExit(0)
        else:
            self.send("%s BAD unsupported command %s" % (tag, cmd))


class Threaded(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--smtp-port", type=int, default=2525)
    ap.add_argument("--imap-port", type=int, default=2143)
    ap.add_argument("--transcript", required=True)
    ap.add_argument("--bind", default="127.0.0.1")
    args = ap.parse_args()
    CFG["transcript"] = os.path.abspath(args.transcript)
    os.makedirs(os.path.dirname(CFG["transcript"]) or ".", exist_ok=True)
    open(CFG["transcript"], "a", encoding="utf-8").close()

    smtp = Threaded((args.bind, args.smtp_port), SmtpHandler)
    imap = Threaded((args.bind, args.imap_port), ImapHandler)
    threading.Thread(target=smtp.serve_forever, daemon=True).start()
    print("smtp %s:%d  imap %s:%d  transcript %s"
          % (args.bind, args.smtp_port, args.bind, args.imap_port, CFG["transcript"]),
          flush=True)
    try:
        imap.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
