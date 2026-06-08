---
name: with-artifacts
description: agentskills.io skill that declares an artifact
when-to-use: when reproducing the artifact contract
artifacts:
  - path: out/report.md
    template: "Hello ${args.name}"
---

Body.
