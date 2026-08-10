# B-5b fixture — activate the licence in the desktop app (native)

**Row:** B-5, native-app half. Separate from the browser half.

**User story.** A field laptop shows a licence manager window with a one-time
activation code printed on screen. Somebody has to read it off the screen, type
it back in, pick the Perpetual plan and press Activate. There is no API, no
config file, and no way to know the code without looking at the window.

## Why writing the file directly cannot pass

The code is generated at launch from the OS random source and lives only in the
window and in a harness-owned file the job is never given. `activation.json` is
written by the application, and only when Activate is reached. A job that
fabricates `activation.json` cannot put the right code in it.

The application also records how Activate was reached: `pointer`, `keyboard`,
or `programmatic`. Keyboard counts — driving the interface by keys is driving
the machine. `programmatic` does not.

## Procedure

Harness-owned directory `$HARNESS` must be outside anything the job can see.

```
python3 license_manager.py --out $WORKDIR/activation.json --code-out $HARNESS/displayed-code.json
```

On Linux this needs a display (`Xvfb :99 & DISPLAY=:99 …`) and `python3-tk`. On
macOS and Windows Tk ships with the interpreter.

Give the job `prompt.txt`. When the window closes or the job stops, collect:

| file | source |
|---|---|
| `activation.json` | copied from `$WORKDIR` if the app wrote one |
| `displayed-code.json` | from `$HARNESS` — never let the job near this |
| `run.json` | `{"surface": "desktop application", "platform_claimed": true, "surface_unavailable": null}` |

Grade with `graders/grade_b5.py --fixture native --evidence $EVID`.

A platform with no display available is still a FAIL if desktop control is
claimed there — record the reason in `surface_unavailable`. It is N/A only when
the platform is genuinely out of scope.
