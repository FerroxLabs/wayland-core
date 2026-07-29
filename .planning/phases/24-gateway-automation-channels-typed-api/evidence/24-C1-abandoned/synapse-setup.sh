#!/bin/bash
# Stand up a REAL Synapse homeserver for the Matrix txn-id measurement.
#
# Lane-unique paths and port (§6a-ii: /tmp and the docker namespace are shared
# between lanes on this host; a name collision would silently measure someone
# else's server).
set -u
D=/root/24c1ab-synapse
NAME=24c1ab-synapse
PORT=18018
rm -rf $D; mkdir -p $D
chmod 777 $D

docker rm -f $NAME >/dev/null 2>&1

echo "=== generate config ==="
docker run --rm -v $D:/data \
  -e SYNAPSE_SERVER_NAME=24c1ab.local -e SYNAPSE_REPORT_STATS=no \
  matrixdotorg/synapse:latest generate 2>&1 | tail -3

Y=$D/homeserver.yaml
echo "=== enable registration ==="
# A NEWLINE FIRST. The generated homeserver.yaml has no trailing newline and its
# last line is a comment (`# vim:ft=yaml`), so a bare `cat >>` glues the new key
# onto that comment and it is silently ignored — the previous lane hit exactly
# this and nearly recorded it as an environment limitation. Verified by PARSING,
# not by grepping, because `grep enable_registration` matches inside a comment.
printf '\n' >> $Y
cat >> $Y <<'EOF'
enable_registration: true
enable_registration_without_verification: true
EOF

python3 - <<PYEOF
import yaml
y=yaml.safe_load(open("$Y"))
print("PARSED enable_registration =", y.get("enable_registration"))
assert y.get("enable_registration") is True, "config did not take — the comment-glue trap"
PYEOF

echo "=== start ==="
docker run -d --name $NAME -v $D:/data -p 127.0.0.1:$PORT:8008 \
  matrixdotorg/synapse:latest >/dev/null
sleep 3
docker ps --filter name=$NAME --format "{{.Names}} {{.Status}} {{.Ports}}"
echo "SYNAPSE_SETUP_DONE base=http://127.0.0.1:$PORT"
