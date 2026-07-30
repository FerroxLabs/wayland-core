### preflight-clean
$ systemctl --user list-unit-files wayland-core-gateway-f24j.service
residual_query_status=1
UNIT FILE STATE PRESET

0 unit files listed.

gateway_state="uninstalled"

### binary-identity
$ /root/lane-gwplat-core --build-info
wayland-core 0.12.25 (source 7c61079842346988f6d19d399b7d92b672dec680)
sha256=b3190b19ff6012cf6c68dca298753b021d02b4bc2dd6670135a93fc482a62385
path=/root/lane-gwplat-core

### profile-setup
$ write /tmp/lane-gateway-platforms-run/linux/linux-home/credentials.toml + channels/f24jsink.toml (synthetic credentials)
home=/tmp/lane-gateway-platforms-run/linux/linux-home
profile=f24j
credentials_mode=0600
canary_bytes=49
seeded_bot_token=[REDACTED]

### sink-start
$ /usr/local/bin/node scripts/f24-sink.mjs --journal /tmp/lane-gateway-platforms-run/linux/linux-arrivals.jsonl
SINK_READY url=http://127.0.0.1:33063 journal=/tmp/lane-gateway-platforms-run/linux/linux-arrivals.jsonl
sink_pid=443609
health={"ok":true,"arrivals":0}

### gateway-install
$ /root/lane-gwplat-core gateway install --profile f24j
wrote unit: /root/.config/systemd/user/wayland-core-gateway-f24j.service
gateway installed (systemd): wayland-core-gateway-f24j
  home:    /tmp/lane-gateway-platforms-run/linux/linux-home
  binary:  /root/lane-gwplat-core
$ systemctl --user daemon-reload
status=0

$ systemctl --user list-unit-files wayland-core-gateway-f24j.service
registration_status=0
UNIT FILE                         STATE   PRESET
wayland-core-gateway-f24j.service enabled enabled

1 unit files listed.

### gateway-start
$ /root/lane-gwplat-core gateway start --profile f24j
gateway start requested (systemd)

### status-running
$ /root/lane-gwplat-core gateway status --profile f24j --json
{
  "state": "running",
  "pid": 444753,
  "uptime_secs": 0,
  "profile": "f24j",
  "turns_in_flight": 0,
  "deliveries_pending": 0,
  "binary_path": "/root/lane-gwplat-core",
  "binary_version": "0.12.25"
}
liveness_probe=kill -0 444753 -> alive

### automation-add
$ /root/lane-gwplat-core cron add --trigger every:15 --channel f24jsink --text f24j-heartbeat ; /root/lane-gwplat-core cron add --trigger "cron:0 9 * * *" --channel f24jsink --text f24j-daily
next[0]: 2026-07-30T01:59:29.195925563+00:00
next[1]: 2026-07-30T02:00:29.195925563+00:00
next[2]: 2026-07-30T02:01:29.195925563+00:00
added 07cf4642-496a-4682-a639-f580e8f8362d
next[0]: 2026-07-30T09:00:00+00:00
next[1]: 2026-07-31T09:00:00+00:00
next[2]: 2026-08-01T09:00:00+00:00
added 3824e3b8-709f-4671-986f-bfd7650d88f8
$ /root/lane-gwplat-core cron list
on  07cf4642-496a-4682-a639-f580e8f8362d  [interval  ] @every 15s                    channel  f24jsink :: f24j-heartbeat  last_fired=never
on  3824e3b8-709f-4671-986f-bfd7650d88f8  [cron      ] 0 9 * * *                     channel  f24jsink :: f24j-daily  last_fired=never

### deliveries-submit
$ /root/lane-gwplat-core cron add --trigger every:15 --channel <per-adapter> --text f24j-delivery-NN x12 across 3 adapters
submitted=12
submitted_by_adapter=slack=4 whatsapp=4 sms=4
f24j-delivery-01 -> slack: added aa27947d-c147-43f9-891e-796074013c5b
f24j-delivery-02 -> whatsapp: added 8d158a3d-eadc-402c-9575-189104721d5c
f24j-delivery-03 -> sms: added c28315e2-3fc1-4971-8bfb-d87735cb1698
f24j-delivery-04 -> slack: added 86804a2b-1da6-4dd8-bfcb-612096b06267
f24j-delivery-05 -> whatsapp: added 46c3c921-03bc-407b-8b82-97caad2cb102
f24j-delivery-06 -> sms: added 18091d12-d5cf-451c-a561-256e7299ddd9
f24j-delivery-07 -> slack: added fe4552ba-cbec-4d1e-bd96-760aa1434250
f24j-delivery-08 -> whatsapp: added 0b3d5920-4e92-4a41-b914-eedf0d1b2256
f24j-delivery-09 -> sms: added 4dcdafa5-9bb4-4cdb-91dd-ca266460a822
f24j-delivery-10 -> slack: added 5fc986c2-8c36-41d6-bfb6-6aa6caa76313
f24j-delivery-11 -> whatsapp: added c972951e-6cef-4ec1-87e3-627b8fb5dd8d
f24j-delivery-12 -> sms: added 3b834510-7e67-4c72-8529-440019aa309f

### arrival-before-kill
$ read arrivals journal /tmp/lane-gateway-platforms-run/linux/linux-arrivals.jsonl (owned by the independent sink)
arrivals_total=8
unique_expected_bodies=7
first_arrival={"seq":1,"ts":"1.000000","endpoint":"chat.postMessage","conversation_id":"f24jsink","text":"f24j-heartbeat","auth_fingerprint":"sha256:f524f4552ee0","answered":true,"idempotency_key":"cron:07cf4642-496a-4682-a639-f580e8f8362d:1785376769195","suppressed":false,"at":"2026-07-30T01:59:30.092Z"}

### hard-kill
$ kill -9 444753
killed_pid=444753
kill_status=0

liveness_after_kill=kill -0 444753 -> gone

### platform-recover
$ /root/lane-gwplat-core gateway status --profile f24j --json (polled; NO manual start)
killed_pid=444753
recovered_pid=478963
{
  "state": "running",
  "pid": 478963,
  "uptime_secs": 1,
  "profile": "f24j",
  "turns_in_flight": 0,
  "deliveries_pending": 0,
  "binary_path": "/root/lane-gwplat-core",
  "binary_version": "0.12.25"
}
$ systemctl --user show -p NRestarts --value wayland-core-gateway-f24j
status=0
1

### delivery-reconcile
$ tally the independent sink's journal /tmp/lane-gateway-platforms-run/linux/linux-arrivals.jsonl, per observed endpoint
arrival_source=independent-sink
submitted=12
arrived=12
unique=12
duplicates=0
losses=0
journal_lines_total=13
adapters_exercised=3/10
  slack endpoint=chat.postMessage submitted=4 arrived=4 unique=4
  whatsapp endpoint=whatsapp.messages submitted=4 arrived=4 unique=4
  sms endpoint=twilio.messages submitted=4 arrived=4 unique=4

### upgrade-in-place
$ /root/lane-gwplat-core gateway uninstall --profile f24j ; /tmp/lane-gateway-platforms-run/linux/linux-upgraded-core gateway install --profile f24j ; /root/lane-gwplat-core gateway start --profile f24j
upgrade_target=/tmp/lane-gateway-platforms-run/linux/linux-upgraded-core
stop_status=0
gateway stop requested (systemd)
removed unit: /root/.config/systemd/user/wayland-core-gateway-f24j.service
gateway uninstalled (systemd): wayland-core-gateway-f24j
wrote unit: /root/.config/systemd/user/wayland-core-gateway-f24j.service
gateway installed (systemd): wayland-core-gateway-f24j
  home:    /tmp/lane-gateway-platforms-run/linux/linux-home
  binary:  /tmp/lane-gateway-platforms-run/linux/linux-upgraded-core
gateway start requested (systemd)
observed_binary_path=/tmp/lane-gateway-platforms-run/linux/linux-upgraded-core
observed_pid=480409
{
  "state": "running",
  "pid": 480409,
  "uptime_secs": 0,
  "profile": "f24j",
  "turns_in_flight": 0,
  "deliveries_pending": 0,
  "binary_path": "/tmp/lane-gateway-platforms-run/linux/linux-upgraded-core",
  "binary_version": "0.12.25"
}

### rollback
$ /root/lane-gwplat-core gateway uninstall --profile f24j ; /root/lane-gwplat-core gateway install --profile f24j ; /root/lane-gwplat-core gateway start --profile f24j
rollback_target=/root/lane-gwplat-core
stop_status=0
gateway stop requested (systemd)
removed unit: /root/.config/systemd/user/wayland-core-gateway-f24j.service
gateway uninstalled (systemd): wayland-core-gateway-f24j
wrote unit: /root/.config/systemd/user/wayland-core-gateway-f24j.service
gateway installed (systemd): wayland-core-gateway-f24j
  home:    /tmp/lane-gateway-platforms-run/linux/linux-home
  binary:  /root/lane-gwplat-core
gateway start requested (systemd)
observed_binary_path=/root/lane-gwplat-core
observed_pid=481173
{
  "state": "running",
  "pid": 481173,
  "uptime_secs": 2,
  "profile": "f24j",
  "turns_in_flight": 0,
  "deliveries_pending": 0,
  "binary_path": "/root/lane-gwplat-core",
  "binary_version": "0.12.25"
}
