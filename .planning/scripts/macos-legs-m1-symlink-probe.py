#!/usr/bin/env python3
"""Root-cause probe for the macOS media-intake refusal found by the 27-C1 live drive.

The live drive's KNOWN-POSITIVE arm — a real WAV under `$TMPDIR` — came back
`is_error=true: Cannot open audio path component ...: Not a directory (os error 20)`
on macOS, where the same arm on `hetzner-dsm` reached the network.

`media_intake::open_once` walks the path from `/` opening every intermediate
component with `O_DIRECTORY | O_NOFOLLOW`. This reproduces that exact syscall
sequence outside the product, on three path shapes, so the cause is a measured
syscall result and not an inference from reading the source.

  python3 .planning/scripts/macos-legs-m1-symlink-probe.py
"""

import ctypes
import ctypes.util
import errno
import os
import pathlib
import sys
import tempfile

libc = ctypes.CDLL(ctypes.util.find_library("c"), use_errno=True)
libc.openat.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int]
libc.openat.restype = ctypes.c_int

O_RDONLY = os.O_RDONLY
O_NOFOLLOW = os.O_NOFOLLOW
O_DIRECTORY = os.O_DIRECTORY
O_CLOEXEC = os.O_CLOEXEC
O_NONBLOCK = os.O_NONBLOCK


def walk(path):
    """Replay `open_once`'s component walk. Returns (ok, failing_component, errno)."""
    p = pathlib.PurePosixPath(path)
    assert p.is_absolute()
    names = [c for c in p.parts[1:]]
    parent = os.open("/", O_RDONLY | O_DIRECTORY)
    try:
        for i, name in enumerate(names):
            leaf = i + 1 == len(names)
            flags = (
                O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC
                if leaf
                else O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC
            )
            ctypes.set_errno(0)
            fd = libc.openat(parent, name.encode(), flags)
            if fd < 0:
                e = ctypes.get_errno()
                return False, "/".join(names[: i + 1]), e
            os.close(parent)
            parent = fd
        return True, None, 0
    finally:
        try:
            os.close(parent)
        except OSError:
            pass


def report(label, path):
    ok, comp, e = walk(path)
    name = errno.errorcode.get(e, str(e))
    msg = os.strerror(e) if e else "-"
    print(
        f"  {label:<28} path={path}\n"
        f"  {'':<28} walk_ok={ok} failed_at={comp} errno={name} ({msg})"
    )
    return ok


def main():
    print("### macOS media-intake component-walk probe")
    print(f"uname: {os.uname().sysname} {os.uname().release} {os.uname().machine}")
    print(f"TMPDIR={os.environ.get('TMPDIR')}")
    print()

    print("0. Which top-level directories are symlinks on this host?")
    for d in ("/tmp", "/var", "/etc", "/Users", "/private"):
        try:
            is_link = os.path.islink(d)
            tgt = os.readlink(d) if is_link else "-"
        except OSError as exc:
            is_link, tgt = f"<{exc}>", "-"
        print(f"  {d:<10} islink={is_link} -> {tgt}")
    print()

    print("1. A real file under the platform's own TMPDIR (what tempfile/mktemp -d give you)")
    tmpd = pathlib.Path(tempfile.mkdtemp(prefix="wl-m1-probe-"))
    f_tmp = tmpd / "good.wav"
    f_tmp.write_bytes(b"RIFF$\x08\x00\x00WAVEfmt " + b"\x00" * 16)
    a = report("TMPDIR (as handed out)", str(f_tmp))

    print()
    print("2. The SAME file, via its fully resolved path (no symlinked component)")
    b = report("TMPDIR (realpath)", os.path.realpath(str(f_tmp)))

    print()
    print("3. A file under $HOME, whose components are all real directories")
    home_dir = pathlib.Path(os.path.expanduser("~")) / ".wl-m1-probe"
    home_dir.mkdir(exist_ok=True)
    f_home = home_dir / "good.wav"
    f_home.write_bytes(b"RIFF$\x08\x00\x00WAVEfmt " + b"\x00" * 16)
    c = report("HOME", str(f_home))

    print()
    print("4. Literal /tmp (the Linux-native shape this walk was written against)")
    ltmp = pathlib.Path("/tmp") / f"wl-m1-probe-{os.getpid()}.wav"
    ltmp.write_bytes(b"RIFF$\x08\x00\x00WAVEfmt " + b"\x00" * 16)
    d = report("/tmp literal", str(ltmp))

    # cleanup
    for p in (f_tmp, f_home, ltmp):
        try:
            p.unlink()
        except OSError:
            pass
    try:
        tmpd.rmdir()
        home_dir.rmdir()
    except OSError:
        pass

    print()
    print(
        f"PROBE-SUMMARY: tmpdir_walk_ok={a} tmpdir_realpath_walk_ok={b} "
        f"home_walk_ok={c} slash_tmp_walk_ok={d}"
    )
    print(
        "INTERPRETATION: the walk succeeds only when NO component is a symlink. "
        "On macOS `/tmp` and `/var` are OS-provided symlinks into `/private`, and "
        "`$TMPDIR` is always under `/var/folders/...`, so every path the platform's "
        "own temp APIs hand out is refused by an O_NOFOLLOW|O_DIRECTORY walk. "
        "Linux has no such top-level symlinks, which is why this never appeared there."
    )
    # Both directions: if arm 2 or 3 also failed, the diagnosis "symlinked
    # component" would be wrong and this probe must say so rather than assert it.
    if not (b and c):
        print(
            "PROBE-WARNING: a no-symlink path ALSO failed — the symlinked-component "
            "diagnosis is NOT supported by this probe."
        )
        return 1
    if a:
        print(
            "PROBE-WARNING: the TMPDIR path SUCCEEDED — the live-drive failure has a "
            "different cause and this probe does not explain it."
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
