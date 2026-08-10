#!/usr/bin/env python3
"""Build the A-10 audio fixture: a planning call with one decision in it.

    python3 gen_a10_audio.py --out <dir>      (macOS: needs `say` and ffmpeg)

Each line is synthesised on its own and the clips are joined in order, so the
onset of every utterance is arithmetic rather than an estimate. That is the
only reason "at what timestamp" can be graded exactly.

The call contains a decoy. Early on, someone proposes a batch size of five
hundred and is overruled; the number that was actually agreed comes later. An
answer of five hundred means the recording was searched for a number rather
than listened to, and the key treats that as a failure rather than a near miss.

The first line carries a control phrase that appears nowhere else. If a
transcriber cannot recover the control phrase, the recording is worse than any
real recording and the sub-case is UNPROVEN rather than failed.
"""

import argparse
import json
import os
import shutil
import subprocess
import tempfile

CONTROL_PHRASE = "harbour lantern nine"

# Two speakers, strictly alternating. Only voices that are actually installed
# are used: `say` falls back silently to the default voice for a name it does
# not have, which would quietly put two speakers in one voice.
SCRIPT = [
    ("Daniel", "Right. Quick planning sync for the migration. "
               "Control phrase for this recording is harbour lantern nine."),
    ("Samantha", "Before we start, are we still blocked on the staging refresh?"),
    ("Daniel", "Partly. Storage is the constraint. We will come back to it."),
    ("Samantha", "On the job runner. I still think we should cap the batch size "
                 "at five hundred rows per job."),
    ("Daniel", "Five hundred is the number that broke us in February. "
               "The database team were very clear about that."),
    ("Samantha", "That is fair. So what number do you want instead?"),
    ("Daniel", "Give me a moment. The cutover date and the batch size interact."),
    ("Samantha", "Take your time."),
    ("Daniel", "Here is the decision. We are moving the cutover to the fourteenth "
               "of March, and we are capping the batch size at two hundred and "
               "fifty rows per job."),
    ("Samantha", "Two hundred and fifty. I will update the runbook this afternoon."),
    ("Daniel", "Good. Back to storage. How much headroom do we actually have?"),
    ("Samantha", "About forty percent, but it drops when the nightly export runs."),
]

# Which line carries the answer, and which one is the decoy.
ANSWER_INDEX = 8
DECOY_INDEX = 3


def duration_of(path):
    out = subprocess.run(
        ["ffprobe", "-v", "error", "-show_entries", "format=duration",
         "-of", "default=nw=1:nk=1", path],
        stdout=subprocess.PIPE, check=True,
    )
    return float(out.stdout.decode().strip())


def build(out_dir):
    if shutil.which("say") is None:
        raise SystemExit("this fixture is generated on macOS, which has `say`")
    if shutil.which("ffmpeg") is None:
        raise SystemExit("ffmpeg is required")

    installed = subprocess.run(["say", "-v", "?"], stdout=subprocess.PIPE, check=True)
    names = {line.split()[0] for line in installed.stdout.decode().splitlines() if line.split()}
    for voice, _text in SCRIPT:
        if voice not in names:
            raise SystemExit(
                "voice %r is not installed; `say` would fall back silently and put "
                "two speakers in one voice" % voice
            )

    tmp = tempfile.mkdtemp(prefix="a10-audio-")
    onsets = []
    clock = 0.0
    gap = 0.35
    try:
        parts = []
        for index, (voice, text) in enumerate(SCRIPT):
            aiff = os.path.join(tmp, "s%02d.aiff" % index)
            wav = os.path.join(tmp, "s%02d.wav" % index)
            subprocess.run(["say", "-v", voice, "-o", aiff, text], check=True)
            subprocess.run(
                ["ffmpeg", "-y", "-loglevel", "error", "-i", aiff,
                 "-af", "adelay=%d|%d" % (int(gap * 1000), int(gap * 1000)),
                 "-ar", "22050", "-ac", "1", wav],
                check=True,
            )
            length = duration_of(wav)
            onsets.append({
                "index": index,
                "voice": voice,
                "text": text,
                "starts_at_s": round(clock + gap, 3),
                "ends_at_s": round(clock + length, 3),
            })
            clock += length
            parts.append(wav)

        listing = os.path.join(tmp, "parts.txt")
        with open(listing, "w", encoding="utf-8") as fh:
            for part in parts:
                fh.write("file '%s'\n" % part)
        mp3 = os.path.join(out_dir, "migration-planning-call.mp3")
        subprocess.run(
            ["ffmpeg", "-y", "-loglevel", "error", "-f", "concat", "-safe", "0",
             "-i", listing, "-c:a", "libmp3lame", "-b:a", "48k", "-ar", "22050",
             "-ac", "1", mp3],
            check=True,
        )
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    return mp3, onsets


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    mp3, onsets = build(args.out)

    answer = onsets[ANSWER_INDEX]
    decoy = onsets[DECOY_INDEX]
    print("wrote %s (%d bytes)" % (mp3, os.path.getsize(mp3)))
    print("total length %.2fs" % onsets[-1]["ends_at_s"])
    print("control phrase %r in the first line" % CONTROL_PHRASE)
    print("DECOY  (five hundred, overruled) %.2f - %.2f s"
          % (decoy["starts_at_s"], decoy["ends_at_s"]))
    print("ANSWER (two hundred and fifty)   %.2f - %.2f s"
          % (answer["starts_at_s"], answer["ends_at_s"]))
    print(json.dumps(onsets, indent=2))


if __name__ == "__main__":
    main()
