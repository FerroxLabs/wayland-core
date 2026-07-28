### preflight-clean
$ launchctl list wayland-core-gateway-f24j
residual_query_status=113
Could not find service "wayland-core-gateway-f24j" in domain for port

gateway_state="uninstalled"

### binary-identity
$ /tmp/f24-run/macos/wayland-core --build-info
wayland-core 0.12.25 (source eba6e9d7b75d46954ae376cecfdcc7ea4d994b14)
sha256=997d55408ba53814f9156929c9a68c3748c3de27ec2f790f1bc4d8f0783a8664
path=/tmp/f24-run/macos/wayland-core

### profile-setup
$ write /tmp/f24-run/macos-home/credentials.toml + channels/f24jsink.toml (synthetic credentials)
home=/tmp/f24-run/macos-home
profile=f24j
credentials_mode=0600
canary_bytes=49
seeded_bot_token=[REDACTED]

### sink-start
$ /Users/seandonahoe/.nvm/versions/node/v22.23.1/bin/node scripts/f24-sink.mjs --journal /tmp/f24-run/macos-arrivals.jsonl
SINK_READY url=http://127.0.0.1:49344 journal=/tmp/f24-run/macos-arrivals.jsonl
sink_pid=44885
health={"ok":true,"arrivals":0}

### gateway-install
$ /tmp/f24-run/macos/wayland-core gateway install --profile f24j
wrote unit: /Users/seandonahoe/Library/LaunchAgents/wayland-core-gateway-f24j.plist
gateway installed (launchd): wayland-core-gateway-f24j
  home:    /private/tmp/f24-run/macos-home
  binary:  /tmp/f24-run/macos/wayland-core
$ launchctl list wayland-core-gateway-f24j
registration_status=0
{
	"StandardOutPath" = "/private/tmp/f24-run/macos-home/gateway.log";
	"LimitLoadToSessionType" = "Aqua";
	"StandardErrorPath" = "/private/tmp/f24-run/macos-home/gateway.log";
	"Label" = "wayland-core-gateway-f24j";
	"OnDemand" = false;
	"LastExitStatus" = 0;
	"PID" = 44903;
	"Program" = "/tmp/f24-run/macos/wayland-core";
	"ProgramArguments" = (
		"/tmp/f24-run/macos/wayland-core";
		"gateway";
		"run";
		"--profile";
		"f24j";
	);
};

### gateway-start
$ /tmp/f24-run/macos/wayland-core gateway start --profile f24j
gateway start requested (launchd)

### status-running
$ /tmp/f24-run/macos/wayland-core gateway status --profile f24j --json
{
  "state": "running",
  "pid": 44903,
  "uptime_secs": 1,
  "profile": "f24j",
  "turns_in_flight": 0,
  "deliveries_pending": 0,
  "binary_path": "/tmp/f24-run/macos/wayland-core",
  "binary_version": "0.12.25"
}
liveness_probe=kill -0 44903 -> alive

### automation-add
$ /tmp/f24-run/macos/wayland-core cron add --trigger every:15 --channel f24jsink --text f24j-heartbeat ; /tmp/f24-run/macos/wayland-core cron add --trigger "cron:0 9 * * *" --channel f24jsink --text f24j-daily
next[0]: 2026-07-28T14:36:48.731+00:00
next[1]: 2026-07-28T14:37:48.731+00:00
next[2]: 2026-07-28T14:38:48.731+00:00
added 0b80c04c-1c2d-4d76-9004-340c1230982f
next[0]: 2026-07-29T09:00:00+00:00
next[1]: 2026-07-30T09:00:00+00:00
next[2]: 2026-07-31T09:00:00+00:00
added c60f1de8-1bda-49db-a578-dcf9c34270d1
$ /tmp/f24-run/macos/wayland-core cron list
on  0b80c04c-1c2d-4d76-9004-340c1230982f  [interval  ] @every 15s                    channel  f24jsink :: f24j-heartbeat  last_fired=never
on  c60f1de8-1bda-49db-a578-dcf9c34270d1  [cron      ] 0 9 * * *                     channel  f24jsink :: f24j-daily  last_fired=never

### deliveries-submit
$ /tmp/f24-run/macos/wayland-core cron add --trigger every:15 --channel f24jsink --text f24j-delivery-NN x12
submitted=12
f24j-delivery-01: added f4b31226-38bd-47e1-a11e-5f5c8281218c
f24j-delivery-02: added 6077738e-5658-4950-a84b-f4b689250674
f24j-delivery-03: added c0d43fe1-ba7c-4756-8041-e3e7b7a5f022
f24j-delivery-04: added 6f76d247-1b01-4c0d-aef9-2dfbd458ad69
f24j-delivery-05: added 1878c155-d1fc-48f2-90c4-1c681717a67e
f24j-delivery-06: added ca934827-8eba-4f1d-82c6-0c705cb36c5d
f24j-delivery-07: added 412d4abc-7db5-4063-aa68-873fa3416c10
f24j-delivery-08: added 8c503a3a-2807-4cd2-af2e-7be213888d78
f24j-delivery-09: added aa29fdf1-3aaf-4ab6-8705-8e3171c1b0ca
f24j-delivery-10: added d2d58469-d9a9-4e0c-8e2b-7b73f0e3d7d8
f24j-delivery-11: added 712d3c9f-eea7-4433-9cbb-0db9ab566ee5
f24j-delivery-12: added 6e0c0c01-c96b-4397-b22d-16c8a551923e

### arrival-before-kill
$ read arrivals journal /tmp/f24-run/macos-arrivals.jsonl (owned by the independent sink)
arrivals_total=13
unique_expected_bodies=12
first_arrival={"seq":1,"ts":"1.000000","endpoint":"chat.postMessage","conversation_id":"f24jsink","text":"f24j-heartbeat","auth_fingerprint":"sha256:ce107af0b499","answered":true,"idempotency_key":"cron:0b80c04c-1c2d-4d76-9004-340c1230982f:1785249408730","suppressed":false,"at":"2026-07-28T14:36:49.711Z"}

### hard-kill
$ kill -9 44903
killed_pid=44903
kill_status=0

liveness_after_kill=kill -0 44903 -> gone

### platform-recover
$ /tmp/f24-run/macos/wayland-core gateway status --profile f24j --json (polled; NO manual start)
killed_pid=44903
recovered_pid=46344
{
  "state": "running",
  "pid": 46344,
  "uptime_secs": 1,
  "profile": "f24j",
  "turns_in_flight": 0,
  "deliveries_pending": 0,
  "binary_path": "/tmp/f24-run/macos/wayland-core",
  "binary_version": "0.12.25"
}
$ launchctl list wayland-core-gateway-f24j
status=0
{
	"StandardOutPath" = "/private/tmp/f24-run/macos-home/gateway.log";
	"LimitLoadToSessionType" = "Aqua";
	"StandardErrorPath" = "/private/tmp/f24-run/macos-home/gateway.log";
	"Label" = "wayland-core-gateway-f24j";
	"OnDemand" = false;
	"LastExitStatus" = 9;
	"PID" = 46344;
	"Program" = "/tmp/f24-run/macos/wayland-core";
	"ProgramArguments" = (
		"/tmp/f24-run/macos/wayland-core";
		"gateway";
		"run";
		"--profile";
		"f24j";
	);
};

### delivery-reconcile
$ tally the independent sink's journal /tmp/f24-run/macos-arrivals.jsonl
arrival_source=independent-sink
submitted=12
arrived=12
unique=12
duplicates=0
losses=0
journal_lines_total=13

### upgrade-in-place
$ /tmp/f24-run/macos/wayland-core gateway uninstall --profile f24j ; /tmp/f24-run/macos-upgraded-core gateway install --profile f24j ; /tmp/f24-run/macos/wayland-core gateway start --profile f24j
upgrade_target=/tmp/f24-run/macos-upgraded-core
stop_status=0
gateway stop requested (launchd)
removed unit: /Users/seandonahoe/Library/LaunchAgents/wayland-core-gateway-f24j.plist
gateway uninstalled (launchd): wayland-core-gateway-f24j
wrote unit: /Users/seandonahoe/Library/LaunchAgents/wayland-core-gateway-f24j.plist
gateway installed (launchd): wayland-core-gateway-f24j
  home:    /private/tmp/f24-run/macos-home
  binary:  /tmp/f24-run/macos-upgraded-core
gateway start requested (launchd)
observed_binary_path=/tmp/f24-run/macos-upgraded-core
observed_pid=46555
{
  "state": "running",
  "pid": 46555,
  "uptime_secs": 2,
  "profile": "f24j",
  "turns_in_flight": 0,
  "deliveries_pending": 0,
  "binary_path": "/tmp/f24-run/macos-upgraded-core",
  "binary_version": "0.12.25"
}

### rollback
$ /tmp/f24-run/macos/wayland-core gateway uninstall --profile f24j ; /tmp/f24-run/macos/wayland-core gateway install --profile f24j ; /tmp/f24-run/macos/wayland-core gateway start --profile f24j
rollback_target=/tmp/f24-run/macos/wayland-core
stop_status=0
gateway stop requested (launchd)
removed unit: /Users/seandonahoe/Library/LaunchAgents/wayland-core-gateway-f24j.plist
gateway uninstalled (launchd): wayland-core-gateway-f24j
wrote unit: /Users/seandonahoe/Library/LaunchAgents/wayland-core-gateway-f24j.plist
gateway installed (launchd): wayland-core-gateway-f24j
  home:    /private/tmp/f24-run/macos-home
  binary:  /tmp/f24-run/macos/wayland-core
gateway start requested (launchd)
observed_binary_path=/tmp/f24-run/macos/wayland-core
observed_pid=46682
{
  "state": "running",
  "pid": 46682,
  "uptime_secs": 2,
  "profile": "f24j",
  "turns_in_flight": 0,
  "deliveries_pending": 0,
  "binary_path": "/tmp/f24-run/macos/wayland-core",
  "binary_version": "0.12.25"
}
