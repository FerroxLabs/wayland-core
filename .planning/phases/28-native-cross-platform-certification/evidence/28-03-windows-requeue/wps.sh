#!/bin/bash
# wps.sh -- run a PowerShell script on seandesktop with NO shell-quoting hazard.
#
# The script is read from stdin (or $1 as a file), encoded UTF-16LE base64, and handed to
# powershell -EncodedCommand. Nothing is interpreted by bash, by ssh's remote shell, or by
# PowerShell's own string parser on the way in.
#
# EXIT STATUS: carried through explicitly. `$LASTEXITCODE`/`$?` is folded into `exit` on the
# remote side and ssh propagates it. NEVER pipe the output of this script into anything --
# a pipeline reports the LAST command's status, which is the documented trap that has already
# reported a rc=100 run as rc=0 twice on this program. Redirect to a file and read the file.
set -o pipefail
SRC="${1:-/dev/stdin}"
B64=$(python3 -c "
import sys
s = open(sys.argv[1],'rb').read().decode('utf-8')
# wrap so the remote always terminates with an explicit numeric status
s = s + \"\nexit \$(if (\$LASTEXITCODE -ne \$null) { \$LASTEXITCODE } else { 0 })\n\"
sys.stdout.write(__import__('base64').b64encode(s.encode('utf-16-le')).decode())
" "$SRC")
ssh -o BatchMode=yes -o ConnectTimeout=30 -o ServerAliveInterval=15 SeanD@seandesktop \
  "powershell -NoProfile -NonInteractive -EncodedCommand $B64"
