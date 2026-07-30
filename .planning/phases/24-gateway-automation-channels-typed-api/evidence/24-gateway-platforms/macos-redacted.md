### preflight-clean
$ launchctl list wayland-core-gateway-f24j
residual_query_status=113
Could not find service "wayland-core-gateway-f24j" in domain for port

gateway_state="uninstalled"

### binary-identity
$ /tmp/lane-gateway-platforms-art/mac/wayland-core --build-info
wayland-core 0.12.25 (source 7c61079842346988f6d19d399b7d92b672dec680)
sha256=11e14834540ab11998d04d52a6c31bb3d0ba7f7f1345a2755d832eda22a33a44
path=/tmp/lane-gateway-platforms-art/mac/wayland-core

### profile-setup
$ write /tmp/lane-gateway-platforms-run/macos/macos-home/credentials.toml + channels/f24jsink.toml (synthetic credentials)
home=/tmp/lane-gateway-platforms-run/macos/macos-home
profile=f24j
credentials_mode=0600
canary_bytes=49
seeded_bot_token=[REDACTED]

### sink-start
$ /Users/seandonahoe/.nvm/versions/node/v22.23.1/bin/node scripts/f24-sink.mjs --journal /tmp/lane-gateway-platforms-run/macos/macos-arrivals.jsonl
SINK_READY url=http://127.0.0.1:60372 journal=/tmp/lane-gateway-platforms-run/macos/macos-arrivals.jsonl
sink_pid=50831
health={"ok":true,"arrivals":0}

### gateway-install
$ /tmp/lane-gateway-platforms-art/mac/wayland-core gateway install --profile f24j
wrote unit: /Users/seandonahoe/Library/LaunchAgents/wayland-core-gateway-f24j.plist
gateway installed (launchd): wayland-core-gateway-f24j
  home:    /private/tmp/lane-gateway-platforms-run/macos/macos-home
  binary:  /tmp/lane-gateway-platforms-art/mac/wayland-core
$ launchctl list wayland-core-gateway-f24j
registration_status=0
{
	"StandardOutPath" = "/private/tmp/lane-gateway-platforms-run/macos/macos-home/gateway.log";
	"LimitLoadToSessionType" = "Aqua";
	"StandardErrorPath" = "/private/tmp/lane-gateway-platforms-run/macos/macos-home/gateway.log";
	"Label" = "wayland-core-gateway-f24j";
	"OnDemand" = false;
	"LastExitStatus" = 0;
	"PID" = 50849;
	"Program" = "/tmp/lane-gateway-platforms-art/mac/wayland-core";
	"ProgramArguments" = (
		"/tmp/lane-gateway-platforms-art/mac/wayland-core";
		"gateway";
		"run";
		"--profile";
		"f24j";
	);
};

### gateway-start
$ /tmp/lane-gateway-platforms-art/mac/wayland-core gateway start --profile f24j
gateway start requested (launchd)

### status-running
$ /tmp/lane-gateway-platforms-art/mac/wayland-core gateway status --profile f24j --json
{
  "state": "running",
  "pid": 50849,
  "uptime_secs": 0,
  "profile": "f24j",
  "turns_in_flight": 0,
  "deliveries_pending": 0,
  "binary_path": "/tmp/lane-gateway-platforms-art/mac/wayland-core",
  "binary_version": "0.12.25"
}
liveness_probe=kill -0 50849 -> alive

### automation-add
$ /tmp/lane-gateway-platforms-art/mac/wayland-core cron add --trigger every:15 --channel f24jsink --text f24j-heartbeat ; /tmp/lane-gateway-platforms-art/mac/wayland-core cron add --trigger "cron:0 9 * * *" --channel f24jsink --text f24j-daily
next[0]: 2026-07-30T01:55:22.429255+00:00
next[1]: 2026-07-30T01:56:22.429255+00:00
next[2]: 2026-07-30T01:57:22.429255+00:00
added 1c599c57-b402-477b-b56a-99df8e8e9e0e
next[0]: 2026-07-30T09:00:00+00:00
next[1]: 2026-07-31T09:00:00+00:00
next[2]: 2026-08-01T09:00:00+00:00
added 25bb1c9f-563d-440b-9e3d-194bae988d57
$ /tmp/lane-gateway-platforms-art/mac/wayland-core cron list
on  1c599c57-b402-477b-b56a-99df8e8e9e0e  [interval  ] @every 15s                    channel  f24jsink :: f24j-heartbeat  last_fired=never
on  25bb1c9f-563d-440b-9e3d-194bae988d57  [cron      ] 0 9 * * *                     channel  f24jsink :: f24j-daily  last_fired=never

### deliveries-submit
$ /tmp/lane-gateway-platforms-art/mac/wayland-core cron add --trigger every:15 --channel <per-adapter> --text f24j-delivery-NN x12 across 3 adapters
submitted=12
submitted_by_adapter=slack=4 whatsapp=4 sms=4
f24j-delivery-01 -> slack: added 340076e7-a112-4d10-88ce-649b8621fb88
f24j-delivery-02 -> whatsapp: added 60901099-7f39-4c89-8f03-25b0a21a3fe5
f24j-delivery-03 -> sms: added 8028c056-5539-4da0-a24b-263bb2e2f247
f24j-delivery-04 -> slack: added cc3d7f0c-e204-4df7-94ee-35051ae8e40e
f24j-delivery-05 -> whatsapp: added 5ef0bff5-eda3-42b6-8270-aec4c7d448e4
f24j-delivery-06 -> sms: added 91f6edbe-e17c-4bea-a4ca-34000d18c3ff
f24j-delivery-07 -> slack: added b70a8592-9d8a-419a-919b-274741b58bc1
f24j-delivery-08 -> whatsapp: added 313a3664-13e2-4083-a576-4765723857dc
f24j-delivery-09 -> sms: added 7e056b46-9607-4e78-9045-7737c0beda3e
f24j-delivery-10 -> slack: added 43dfdb51-2d6c-4a84-97c7-7040764e068d
f24j-delivery-11 -> whatsapp: added 5d359599-02eb-4f63-977f-a3e094533384
f24j-delivery-12 -> sms: added bcecd5df-62c5-45b4-98bd-0ee74e0847b6

### arrival-before-kill
$ read arrivals journal /tmp/lane-gateway-platforms-run/macos/macos-arrivals.jsonl (owned by the independent sink)
arrivals_total=13
unique_expected_bodies=12
first_arrival={"seq":1,"ts":"1.000000","endpoint":"chat.postMessage","conversation_id":"f24jsink","text":"f24j-heartbeat","auth_fingerprint":"sha256:8dae03abb765","answered":true,"idempotency_key":"cron:1c599c57-b402-477b-b56a-99df8e8e9e0e:1785376522429","suppressed":false,"at":"2026-07-30T01:55:23.430Z"}

### hard-kill
$ kill -9 50849
killed_pid=50849
kill_status=0

liveness_after_kill=kill -0 50849 -> gone

### platform-recover
$ /tmp/lane-gateway-platforms-art/mac/wayland-core gateway status --profile f24j --json (polled; NO manual start)
killed_pid=50849
recovered_pid=59271
{
  "state": "running",
  "pid": 59271,
  "uptime_secs": 1,
  "profile": "f24j",
  "turns_in_flight": 0,
  "deliveries_pending": 0,
  "binary_path": "/tmp/lane-gateway-platforms-art/mac/wayland-core",
  "binary_version": "0.12.25"
}
$ launchctl list wayland-core-gateway-f24j
status=0
{
	"StandardOutPath" = "/private/tmp/lane-gateway-platforms-run/macos/macos-home/gateway.log";
	"LimitLoadToSessionType" = "Aqua";
	"StandardErrorPath" = "/private/tmp/lane-gateway-platforms-run/macos/macos-home/gateway.log";
	"Label" = "wayland-core-gateway-f24j";
	"OnDemand" = false;
	"LastExitStatus" = 9;
	"PID" = 59271;
	"Program" = "/tmp/lane-gateway-platforms-art/mac/wayland-core";
	"ProgramArguments" = (
		"/tmp/lane-gateway-platforms-art/mac/wayland-core";
		"gateway";
		"run";
		"--profile";
		"f24j";
	);
};

### delivery-reconcile
$ tally the independent sink's journal /tmp/lane-gateway-platforms-run/macos/macos-arrivals.jsonl, per observed endpoint
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
$ /tmp/lane-gateway-platforms-art/mac/wayland-core gateway uninstall --profile f24j ; /tmp/lane-gateway-platforms-run/macos/macos-upgraded-core gateway install --profile f24j ; /tmp/lane-gateway-platforms-art/mac/wayland-core gateway start --profile f24j
upgrade_target=/tmp/lane-gateway-platforms-run/macos/macos-upgraded-core
stop_status=0
gateway stop requested (launchd)
removed unit: /Users/seandonahoe/Library/LaunchAgents/wayland-core-gateway-f24j.plist
gateway uninstalled (launchd): wayland-core-gateway-f24j
wrote unit: /Users/seandonahoe/Library/LaunchAgents/wayland-core-gateway-f24j.plist
gateway installed (launchd): wayland-core-gateway-f24j
  home:    /private/tmp/lane-gateway-platforms-run/macos/macos-home
  binary:  /tmp/lane-gateway-platforms-run/macos/macos-upgraded-core
gateway start requested (launchd)
observed_binary_path=/tmp/lane-gateway-platforms-run/macos/macos-upgraded-core
observed_pid=59451
{
  "state": "running",
  "pid": 59451,
  "uptime_secs": 1,
  "profile": "f24j",
  "turns_in_flight": 0,
  "deliveries_pending": 0,
  "binary_path": "/tmp/lane-gateway-platforms-run/macos/macos-upgraded-core",
  "binary_version": "0.12.25"
}

### rollback
$ /tmp/lane-gateway-platforms-art/mac/wayland-core gateway uninstall --profile f24j ; /tmp/lane-gateway-platforms-art/mac/wayland-core gateway install --profile f24j ; /tmp/lane-gateway-platforms-art/mac/wayland-core gateway start --profile f24j
rollback_target=/tmp/lane-gateway-platforms-art/mac/wayland-core
stop_status=0
gateway stop requested (launchd)
removed unit: /Users/seandonahoe/Library/LaunchAgents/wayland-core-gateway-f24j.plist
gateway uninstalled (launchd): wayland-core-gateway-f24j
wrote unit: /Users/seandonahoe/Library/LaunchAgents/wayland-core-gateway-f24j.plist
gateway installed (launchd): wayland-core-gateway-f24j
  home:    /private/tmp/lane-gateway-platforms-run/macos/macos-home
  binary:  /tmp/lane-gateway-platforms-art/mac/wayland-core
gateway start requested (launchd)
observed_binary_path=/tmp/lane-gateway-platforms-art/mac/wayland-core
observed_pid=59638
{
  "state": "running",
  "pid": 59638,
  "uptime_secs": 1,
  "profile": "f24j",
  "turns_in_flight": 0,
  "deliveries_pending": 0,
  "binary_path": "/tmp/lane-gateway-platforms-art/mac/wayland-core",
  "binary_version": "0.12.25"
}
