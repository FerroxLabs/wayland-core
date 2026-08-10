#!/usr/bin/env python3
"""Positive control for the B-3 mail host.

Starts the fixture mail host, then drives it with Python's own smtplib and
imaplib — clients written by somebody else — to prove the server really speaks
the protocols before anyone blames the product for not talking to it.

  python3 mail_smoke.py        # exits 0 on success, prints what it checked
"""

from __future__ import annotations

import imaplib
import os
import smtplib
import socket
import subprocess
import sys
import tempfile
import time
from email.message import EmailMessage

HERE = os.path.dirname(os.path.abspath(__file__))


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def main():
    smtp_port, imap_port = free_port(), free_port()
    tmp = tempfile.mkdtemp(prefix="b3-smoke-")
    transcript = os.path.join(tmp, "mail-transcript.jsonl")
    proc = subprocess.Popen(
        [sys.executable, os.path.join(HERE, "mailserver.py"),
         "--smtp-port", str(smtp_port), "--imap-port", str(imap_port),
         "--transcript", transcript],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    checks = []
    try:
        for _ in range(50):
            try:
                socket.create_connection(("127.0.0.1", imap_port), timeout=0.5).close()
                break
            except OSError:
                time.sleep(0.1)

        msg = EmailMessage()
        msg["From"] = "agent@fixture.local"
        msg["To"] = "oncall@fixture.local"
        msg["Subject"] = "moneykit major upgrade - which one"
        msg["Message-ID"] = "<q1@fixture.local>"
        msg.set_content("2.0.0 keeps LegacyRateShim. 2.1.0 removes it. Which?")
        with smtplib.SMTP("127.0.0.1", smtp_port, timeout=20) as s:
            s.login("agent@fixture.local", "hunter2")
            s.send_message(msg)
        checks.append(("smtp accepts an authenticated send", True))

        subprocess.run(
            [sys.executable, os.path.join(HERE, "reply_bot.py"),
             "--smtp-port", str(smtp_port), "--persona", "oncall",
             "--transcript", transcript], check=True, capture_output=True)

        box = imaplib.IMAP4("127.0.0.1", imap_port)
        box.login("agent@fixture.local", "hunter2")
        typ, _ = box.select("INBOX")
        checks.append(("imap SELECT INBOX", typ == "OK"))
        typ, data = box.search(None, "ALL")
        uids = data[0].split()
        checks.append(("imap SEARCH finds the reply", typ == "OK" and len(uids) == 1))
        typ, data = box.fetch(uids[0], "(RFC822)")
        raw = data[0][1].decode()
        checks.append(("imap FETCH returns the reply body", "Take 2.0.0" in raw))
        checks.append(("reply threads onto the question", "<q1@fixture.local>" in raw))
        typ, data = box.uid("SEARCH", None, "ALL")
        checks.append(("imap UID SEARCH", typ == "OK" and bool(data[0].split())))
        uid = data[0].split()[0]
        typ, data = box.uid("FETCH", uid, "(BODY.PEEK[] FLAGS)")
        checks.append(("imap UID FETCH BODY.PEEK", typ == "OK" and b"Take 2.0.0" in data[0][1]))
        box.uid("STORE", uid, "+FLAGS", "(\\Seen)")
        typ, data = box.search(None, "UNSEEN")
        checks.append(("\\Seen is honoured", data[0].split() == []))
        box.logout()

        with open(transcript, "r", encoding="utf-8") as fh:
            body = fh.read()
        checks.append(("transcript records the outbound question",
                       "moneykit major upgrade" in body))
        checks.append(("transcript records the fetch", "imap_command" in body))
        checks.append(("transcript hides the password", "hunter2" not in body))
    finally:
        proc.terminate()

    bad = [name for name, ok in checks if not ok]
    for name, ok in checks:
        print("  [%s] %s" % ("ok" if ok else "XX", name))
    print("mail host smoke: %d/%d" % (len(checks) - len(bad), len(checks)))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
