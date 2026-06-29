# Changelog

## [0.12.3](https://github.com/FerroxLabs/wayland-core/compare/v0.12.16...v0.12.3) (2026-06-29)


### Features

* **#255:** active-window kernel — context % vs the post-swap active model ([#74](https://github.com/FerroxLabs/wayland-core/issues/74)) ([7d22c84](https://github.com/FerroxLabs/wayland-core/commit/7d22c847718e48871bde90d666c906de350aecb8))
* **#279:** JSON-stream observability — active-window %, agent-run correlation, structured traces ([#76](https://github.com/FerroxLabs/wayland-core/issues/76)) ([3b9b070](https://github.com/FerroxLabs/wayland-core/commit/3b9b07006f399af3ccd9689166d028d94f2de003))
* **#280:** smart auto-compaction at active-window threshold (default-off, Flux-aware, memory handoff) ([#78](https://github.com/FerroxLabs/wayland-core/issues/78)) ([508d9e8](https://github.com/FerroxLabs/wayland-core/commit/508d9e8e790771f23f82b8577edecfd511624096))
* **#282:** Flux context-routing contract — client side V1 ([#77](https://github.com/FerroxLabs/wayland-core/issues/77)) ([508af81](https://github.com/FerroxLabs/wayland-core/commit/508af81c533b36e0cdedc0e48f55e6f695c70e1d))
* **agent:** add the inbound channel subscriber + TurnDispatcher seam ([b5a077b](https://github.com/FerroxLabs/wayland-core/commit/b5a077b3dbf9d4ea9511fa5822e53c8bf4edffa0))
* **agent:** allow chatgpt.com egress when the chatgpt provider is active ([b3372ac](https://github.com/FerroxLabs/wayland-core/commit/b3372ac8af6b639934b293e0915e21d0c604aebb))
* **agent:** cap plugin MCP connect at 8s so a broken plugin can't dominate boot ([ccdc57e](https://github.com/FerroxLabs/wayland-core/commit/ccdc57e39487a729906936b3ece63965212126fd))
* **agent:** compact Bash tool output into the model transcript (engine-side) ([d24daa0](https://github.com/FerroxLabs/wayland-core/commit/d24daa0c20d0a6f571c7aed2870a536c08384030))
* **agent:** default runaway-loop circuit breaker ([c9483aa](https://github.com/FerroxLabs/wayland-core/commit/c9483aaf8e49b93cba85aa060685f6aa5ca442f5))
* **agent:** default runaway-loop circuit breaker + loop-convergence E2E ([646f0ba](https://github.com/FerroxLabs/wayland-core/commit/646f0bafff8303a99f0d0717ad8ab41c67f6d4d8))
* **agent:** load + namespace agents from installed marketplace plugins (Lane D / G2+G4) ([6c3dc23](https://github.com/FerroxLabs/wayland-core/commit/6c3dc23851ce61460b37e2c053fb11ded90e7fc9))
* **agent:** per-node turn/token budget overrides in workflow IR (rank 50) ([5265df9](https://github.com/FerroxLabs/wayland-core/commit/5265df97cf8d24a1effd5100531f19233bb5c75d))
* **agent:** resolve ${CLAUDE_PLUGIN_ROOT} vars for declarative plugin MCP (Lane D / G3) ([e254335](https://github.com/FerroxLabs/wayland-core/commit/e2543350cb301ea618d048e047d13856e2e94ddc))
* **agent:** wire openai-chatgpt provider with oauth bearer source ([18a50d6](https://github.com/FerroxLabs/wayland-core/commit/18a50d626b45f8bc78ef729f6836732193f9a971))
* **agent:** wire the engine-backed channel dispatcher into bootstrap ([2ba0037](https://github.com/FerroxLabs/wayland-core/commit/2ba003750b8ab53ffd8e85212597dbf6ccf134c4))
* **channels,tui:** surface channel integrations in /doctor + fix F-019 (S10 v1) ([6958c1c](https://github.com/FerroxLabs/wayland-core/commit/6958c1cfbb11e648166af0571c3b42772339584f))
* **channels:** ack reactions + typing keepalive state machine (P7) ([37e32ca](https://github.com/FerroxLabs/wayland-core/commit/37e32caab2d42761d220dde20416e1aba893ee1e))
* **channels:** add the pure inbound dispatch kernel (fail-closed) ([389a40f](https://github.com/FerroxLabs/wayland-core/commit/389a40f1437f7b99305fc3d6af757ba708ed0676))
* **channels:** auth-aware inbound media fetch for all connectors ([e37ae4f](https://github.com/FerroxLabs/wayland-core/commit/e37ae4ff35d19950a8682501f8add5263b3cbdcb))
* **channels:** auth-aware inbound media fetch for all connectors ([a977d34](https://github.com/FerroxLabs/wayland-core/commit/a977d34e6b5902da96f68498de4f8871400d2554))
* **channels:** enrich IncomingMessage with structured inbound facts ([7b08723](https://github.com/FerroxLabs/wayland-core/commit/7b0872389c8b6fd44368a4db573bc0fadbd14613))
* **channels:** fetch + transcribe/describe inbound media before the turn ([8acfbeb](https://github.com/FerroxLabs/wayland-core/commit/8acfbeb0e467c563e9bc8643d6fff525e132f491))
* **channels:** inbound media fetch — transcribe/describe attachments before the turn ([8dedd76](https://github.com/FerroxLabs/wayland-core/commit/8dedd7658935defa4a21683e1292f713d3e017ad))
* **channels:** inbound receive for matrix (/sync) and msteams (Activity) ([33d0c67](https://github.com/FerroxLabs/wayland-core/commit/33d0c6792116e81a6146c3af5142f856ecf220e3))
* **channels:** inbound webhook host (un-mutes slack/whatsapp/sms) ([34234d1](https://github.com/FerroxLabs/wayland-core/commit/34234d1cf847fdeb5e4100a75f7da750f92fbb40))
* **channels:** media (telegram) + per-connector correctness (P5/P6) ([f19d96d](https://github.com/FerroxLabs/wayland-core/commit/f19d96dcb08d5a8da62101143033022581030c09))
* **channels:** outbound message chunking + per-platform caps (HIGH-6) ([f2326fa](https://github.com/FerroxLabs/wayland-core/commit/f2326fa2f6f5ced479ce8735a73e6cd727f6d5d4))
* **channels:** real react/typing for discord, matrix, slack, whatsapp ([4e3cfba](https://github.com/FerroxLabs/wayland-core/commit/4e3cfba6a2c4724f9ece688b93e2decc25b380c8))
* **channels:** real react/typing for discord, matrix, slack, whatsapp ([894572c](https://github.com/FerroxLabs/wayland-core/commit/894572ce8317b92b49f7359bd1babf4a32b4401f))
* **channels:** reconnect supervision — channels survive disconnects ([5568289](https://github.com/FerroxLabs/wayland-core/commit/55682891118436bfc1568440ab2b15e103ca90bf))
* **channels:** scope channel-originated agents to a tool posture ([da66b72](https://github.com/FerroxLabs/wayland-core/commit/da66b728d08ec7374973955fd2e9466a82ec21b5))
* ChatGPT-sub model filtering ([#158](https://github.com/FerroxLabs/wayland-core/issues/158)) + MiniMax cost catalog ([#240](https://github.com/FerroxLabs/wayland-core/issues/240)) ([#68](https://github.com/FerroxLabs/wayland-core/issues/68)) ([f807397](https://github.com/FerroxLabs/wayland-core/commit/f807397dab29b9eea1fe18a9ef0f80e9ead3edfd))
* **cli:** browse catalog cache for the /plugins overlay (Lane F2) ([9ae535e](https://github.com/FerroxLabs/wayland-core/commit/9ae535ea71c235c094aedd897c070da1b9a0757a))
* **cli:** make Config Expert tier ProviderCompat cost fields editable ([18d1557](https://github.com/FerroxLabs/wayland-core/commit/18d1557b4e52b0e52f68120f7f0777aa594e144a))
* **cli:** marketplace resolver + quarantine clone + install pipeline (Lane C) ([0513f29](https://github.com/FerroxLabs/wayland-core/commit/0513f2931c49515f7ad7261604ea778c842a27d8))
* **cli:** plugin marketplace subcommands + name@marketplace install (Lane F1) ([66ab3d0](https://github.com/FerroxLabs/wayland-core/commit/66ab3d005b513e9a04081361ea78b99889d69031))
* **cli:** remove_marketplace_plugin — uninstall teardown for the /plugins overlay ([4523a86](https://github.com/FerroxLabs/wayland-core/commit/4523a867d003365ee53b2dce304c187352d2da01))
* **cli:** unsigned-source trust warning (Lane E3) ([6d66c64](https://github.com/FerroxLabs/wayland-core/commit/6d66c64670b4ac5897a08917cbcbf9d6a8874f31))
* **cli:** wayland auth login/logout/status for chatgpt ([060dc45](https://github.com/FerroxLabs/wayland-core/commit/060dc4533e6df3781a0fefb8021c31500fa5ecd8))
* **compact:** add compaction-floor index primitive (token-opt) ([177052a](https://github.com/FerroxLabs/wayland-core/commit/177052ab919d457b0a7e2394d03f2f550a9d8baf))
* **compat:** add input_optimization route-gate flag (token-opt) ([0c63e7b](https://github.com/FerroxLabs/wayland-core/commit/0c63e7b2c47471d777f4e70476073ccea1610c26))
* **config,tui:** redacted effective-config preview (S9 v1) ([ff30d20](https://github.com/FerroxLabs/wayland-core/commit/ff30d2051303c85cf1019951b59cfccc7cc8287b))
* **config:** chatgpt_defaults compat preset ([8fac871](https://github.com/FerroxLabs/wayland-core/commit/8fac87162af5dd40c9f26c0a7b2196d1590aca55))
* **config:** compact_bash ProviderCompat gate (default on) ([a3cc467](https://github.com/FerroxLabs/wayland-core/commit/a3cc4676c9d02a841b20f1fa3a306783483f9db9))
* **config:** config cockpit — paste-to-connect, editors, /doctor health, /effective, channels, discovery ([8fe5559](https://github.com/FerroxLabs/wayland-core/commit/8fe5559f04131ea02a0ffba23402f5a36a76f6df))
* **config:** connected_providers credential helper ([4cffba9](https://github.com/FerroxLabs/wayland-core/commit/4cffba9030a56ad6d7c4fdedf08bf80a5060414c))
* **config:** openai-chatgpt provider type + parsing ([5709f87](https://github.com/FerroxLabs/wayland-core/commit/5709f87ae5de3e1633b4f6cf6141e9213a70627d))
* **config:** read the Forge local-MCP discovery file (Slice 3) ([1014e21](https://github.com/FerroxLabs/wayland-core/commit/1014e212eab7bf472f4ac38c02fe9939c2116cc4))
* **config:** resolve ~/.wayland profile home for plugin MCP servers ([724106e](https://github.com/FerroxLabs/wayland-core/commit/724106e8663ad37f87eea75ff007995506377de0))
* **discord:** idempotency nonce on outbound sends (HIGH-7) ([4b459d5](https://github.com/FerroxLabs/wayland-core/commit/4b459d52be09e2545da1d7665cafb31b4f130944))
* **discord:** idempotency nonce on outbound sends to dedupe retries ([5c4e3ad](https://github.com/FerroxLabs/wayland-core/commit/5c4e3ad1bcf8bd0639488ae9958ca741dff6eee6))
* **doctor:** list declared MCP servers in --doctor, add --probe-mcp ([100e779](https://github.com/FerroxLabs/wayland-core/commit/100e7792da97c138988155e4ac503f7c90f759d5))
* **evolve:** give ToolCallSchemaReward real selector glue + name the deferral (rank 80) ([e994bd9](https://github.com/FerroxLabs/wayland-core/commit/e994bd9209a87052a55860ced7d96785996cc09a))
* **evolve:** real GatedTraceSink + ExecutionBudget, wired into the binaries (ranks 54, 53) ([dc477ff](https://github.com/FerroxLabs/wayland-core/commit/dc477ff25fed5572e9369094f8db24c47f4a01af))
* FluxRouter capabilities (image/fetch/web_search) + per-model max_tokens + reliability fixes ([#66](https://github.com/FerroxLabs/wayland-core/issues/66)) ([aefdd39](https://github.com/FerroxLabs/wayland-core/commit/aefdd3993c47c0a0ba6e6c7f16fbaf917cc325cd))
* **hooks:** framework-blind HookDispatcher + SessionStart injection ([26b4eed](https://github.com/FerroxLabs/wayland-core/commit/26b4eeda6c98ae0f94f9d44979832fdac7811f75))
* **hooks:** host McpHookDispatcher bridges hook names to MCP tools ([ee49d11](https://github.com/FerroxLabs/wayland-core/commit/ee49d113956d88acbebe57974febeaf6051d63ce))
* **hooks:** inject SessionStart hook prelude on cold turns (gated, budgeted) ([03e014a](https://github.com/FerroxLabs/wayland-core/commit/03e014ab08f6d30747ce59c36fcc5571e2118351))
* **hooks:** wire PrePrompt contribution into per-turn request, cache-safely ([32fd3e1](https://github.com/FerroxLabs/wayland-core/commit/32fd3e173d20008c9bbce4b5d77b9c59d4df7e46))
* **init:** ship discipline scaffold + scanned context in WAYLAND.md ([667d44d](https://github.com/FerroxLabs/wayland-core/commit/667d44dd8c8f089c1d48fc3448929b70893dcf86))
* isolated profiles — CLI-isolation slice (Phase 0 + 1 + 3A + 2) ([#70](https://github.com/FerroxLabs/wayland-core/issues/70)) ([3177b17](https://github.com/FerroxLabs/wayland-core/commit/3177b1763d0334ba03057992d689904b9f810554))
* load + namespace skills from installed marketplace plugins (Lane D3 / G2+G4) ([f120fe6](https://github.com/FerroxLabs/wayland-core/commit/f120fe666dcc82fff567d972653f5fe182f51550))
* **mcp:** /mcp connect — one-command zero-config Forge MCP connect (Slice 3, Piece 3) ([17973e6](https://github.com/FerroxLabs/wayland-core/commit/17973e6bbae98189aeefacd4bdc798e55bbf8b3a))
* **mcp:** capture per-server connect health instead of discarding it ([7314090](https://github.com/FerroxLabs/wayland-core/commit/7314090d1609a3da41d44a92418c659a851e5653))
* **mcp:** DISCOVERED row-to-connect + boot-hero Forge line (Slice 3b polish) ([509fd69](https://github.com/FerroxLabs/wayland-core/commit/509fd69a9d3e14ca5211cfbe04b4d559f7c92db8))
* **mcp:** Forge connect flow — ${cred:KEY} headers + live token grant (Slice 3) ([3f66b9f](https://github.com/FerroxLabs/wayland-core/commit/3f66b9f0457bf11c5f66fd9519c016639c6a8952))
* **mcp:** Forge connect polish — selectable DISCOVERED row + boot-hero line (Slice 3b) ([d19af5b](https://github.com/FerroxLabs/wayland-core/commit/d19af5bf85dc1271dd736a53f7e5f8b3701c1289))
* **mcp:** Forge loopback grant client — liveness probe + scoped token (Slice 3) ([df9d1c9](https://github.com/FerroxLabs/wayland-core/commit/df9d1c9ba8bc4e8f08fb1028cbc0dcd7a246e84a))
* **mcp:** Forge zero-config local-MCP discovery — keystone + reader + grant client + connect flow (Slice 3, headless) ([106b869](https://github.com/FerroxLabs/wayland-core/commit/106b8696412d04ca6f53ded3baab453b5de21f66))
* **mcp:** MCP server executes real built-in tools via injectable executor ([8b5f79a](https://github.com/FerroxLabs/wayland-core/commit/8b5f79a88a256bcef827c345741f214080c2cf74))
* **mcp:** opt-in allow_local to connect trusted loopback MCP servers ([68b0a6b](https://github.com/FerroxLabs/wayland-core/commit/68b0a6ba4902aea9fcfc578e655fa92ebda38939))
* **mcp:** provider tool-count cap + real MCP provenance + web-fetch de-flake (0.12.10) ([#87](https://github.com/FerroxLabs/wayland-core/issues/87)) ([78969c5](https://github.com/FerroxLabs/wayland-core/commit/78969c58d3f7987f6b845b97e8ea2a7409aae679))
* **mcp:** surface pre-connect-skipped MCP servers as a ⊘ row ([d0f44ed](https://github.com/FerroxLabs/wayland-core/commit/d0f44ed4abc66079182a5036610e3b3254b6a5f5))
* **msteams:** Bot Framework JWT validation — un-mutes msteams inbound ([d32e87c](https://github.com/FerroxLabs/wayland-core/commit/d32e87c269d7390d022549e53b40110db0a5c5e4))
* native per-command Bash output compaction ([97174c3](https://github.com/FerroxLabs/wayland-core/commit/97174c398aa178048426cc78ae6203a261e4636c))
* **oauth:** add ChatGPT device-code login (headless/remote path) ([2a6a4e6](https://github.com/FerroxLabs/wayland-core/commit/2a6a4e69118b1af2d3f06dc98d5613f6608f4fee))
* **oauth:** chatgpt token manager with rotating refresh, JWT account-id decode, and flow descriptor ([9a1b5c1](https://github.com/FerroxLabs/wayland-core/commit/9a1b5c156061515b12bab85da2cba5ecedb4b6e1))
* **oauth:** extra authorize params, configurable redirect host/path with dual-stack loopback bind, id_token capture ([765c11a](https://github.com/FerroxLabs/wayland-core/commit/765c11adb9137c28541dda88529a13fdd596dc28))
* **oauth:** import chatgpt tokens from codex cli ([630688d](https://github.com/FerroxLabs/wayland-core/commit/630688d051a0e6302829efa5edb2821847efefd8))
* **observability:** builder support for ToolCallTrace cancelled/partial (rank 58, partial) ([79656e5](https://github.com/FerroxLabs/wayland-core/commit/79656e5257135439fd02097267e92b1fe6d43f65))
* **observability:** compaction savings aggregation + per-call trace log ([f29e8c3](https://github.com/FerroxLabs/wayland-core/commit/f29e8c3c7df7d43f7844e4e4b9884def2cba2f60))
* **observability:** populate ToolCallTrace.cancelled + TurnTrace.hook_actions ([b44bf15](https://github.com/FerroxLabs/wayland-core/commit/b44bf1572a57cbfe3685f0eb59368f7e88cad9e9))
* **observability:** ToolCallTrace.compaction_bytes for savings telemetry ([a4711f7](https://github.com/FerroxLabs/wayland-core/commit/a4711f709d6c23177ac8be5c256dae4681823908))
* **openai:** route gpt-5 family to the Responses API (/v1/responses) ([b08cb97](https://github.com/FerroxLabs/wayland-core/commit/b08cb9748efbd808f5e26f3d126247e9dcaad67f))
* **orchestration:** relay WorkflowRunner sub-agent events to the host (ForgeFlows-Live Phase 1) ([345de84](https://github.com/FerroxLabs/wayland-core/commit/345de8465cf93f9f034b315c430902d7207d5bae))
* **plugin-api:** declarative [[hooks]] + [mcp_server] manifest block ([d81d4c2](https://github.com/FerroxLabs/wayland-core/commit/d81d4c27801b5e400c5167ec630f96d45faa6fc1))
* **plugin-api:** MCP spawn-consent key (Lane E1) ([14accd2](https://github.com/FerroxLabs/wayland-core/commit/14accd22f5f1ff6b31419a12f73308ac093a98a6))
* **plugin:** gate marketplace MCP spawn on install consent (Lane E + D4) ([2bb539a](https://github.com/FerroxLabs/wayland-core/commit/2bb539ad449de5fcb1cde27ae0bba69496d5040e))
* **plugins:** declarative on-disk plugin kind wires hooks+mcp into C1 ([599d1ed](https://github.com/FerroxLabs/wayland-core/commit/599d1ed32186b63678c47b336d227ced6c7d3475))
* **plugins:** discover on-disk plugins under the C3 profile home ([be46478](https://github.com/FerroxLabs/wayland-core/commit/be46478933157199931840b2195c26f20e2475ae))
* **pluginsrc:** canonical plugin model + Claude Code / MCP-registry adapters ([b1916cb](https://github.com/FerroxLabs/wayland-core/commit/b1916cba210b462f5531eae2f70cf5ac689c114c))
* **pluginsrc:** InstallPlan spine — dry-run consent object + grades ([631442f](https://github.com/FerroxLabs/wayland-core/commit/631442fae5012b3d84ea8812a9bec77cf090b8db))
* **pluginsrc:** prompt-asset injection scan → plan warning (Lane E2) ([5d2d13b](https://github.com/FerroxLabs/wayland-core/commit/5d2d13b69d4820d87c9871726c783c78f8596490))
* **pluginsrc:** transactional commit to self-contained native plugin dir ([045d20e](https://github.com/FerroxLabs/wayland-core/commit/045d20ef91263f056a02d9e03908d6e2a43cd2c5))
* **protocol:** add McpFailed event so connect failures reach the UI ([7cb578d](https://github.com/FerroxLabs/wayland-core/commit/7cb578db4a4e2623d495bf84e016aa09532d412a))
* **protocol:** WorkflowStarted/WorkflowFinished events (ForgeFlows-Live Phase 3) ([e1eddb7](https://github.com/FerroxLabs/wayland-core/commit/e1eddb793ccefa5c2d3ec927fe51319b4d260794))
* **providers:** add key fingerprinting for paste-to-detect config ([e71d8ca](https://github.com/FerroxLabs/wayland-core/commit/e71d8ca1d63a98c0c5890481eae9f7a00053686b))
* **providers:** add live key-validation ladder for paste-to-detect ([c576df9](https://github.com/FerroxLabs/wayland-core/commit/c576df9d6104ec3fc53fb57bfe8fb035d16fa82d))
* **providers:** add MiniMax provider via Anthropic-compatible endpoint ([703ba14](https://github.com/FerroxLabs/wayland-core/commit/703ba14ce25f5b23a19a06cea00aebdb16631bc4))
* **providers:** add Sakana AI (Fugu) — OpenAI-compatible endpoint ([#82](https://github.com/FerroxLabs/wayland-core/issues/82)) ([a531f22](https://github.com/FerroxLabs/wayland-core/commit/a531f220d9ffbc089815b9dfb78478ff6affa4bd))
* **providers:** capability-first tools gate for tool-incapable local models (supersedes [#97](https://github.com/FerroxLabs/wayland-core/issues/97)) ([#98](https://github.com/FerroxLabs/wayland-core/issues/98)) ([fcc4b83](https://github.com/FerroxLabs/wayland-core/commit/fcc4b83c5805d25bc294b7987bca512a67ccd2a7))
* **providers:** extend KeyPool rotation to all API-key providers ([7a8644d](https://github.com/FerroxLabs/wayland-core/commit/7a8644d2fc65629dbe83850181637bce7c702583))
* **providers:** live Bedrock model discovery via ListFoundationModels ([27a25dc](https://github.com/FerroxLabs/wayland-core/commit/27a25dcb0e533eaab1a67ca6bc79224a626b7ff6))
* **providers:** live Gemini model discovery ([ed2126e](https://github.com/FerroxLabs/wayland-core/commit/ed2126e6410fa39f26c575e86308dca5c1119f98))
* **providers:** make runtime provider construction OAuth-aware for openai-chatgpt ([3e067c1](https://github.com/FerroxLabs/wayland-core/commit/3e067c1a414a37a9d4df70c3d44ecb7ca176e257))
* **providers:** ModelCatalog.refresh_connected live discovery service ([0bc02bc](https://github.com/FerroxLabs/wayland-core/commit/0bc02bce82c4c1529f36fcd50138050226b9c237))
* **providers:** openai-chatgpt provider over async oauth bearer source ([c19a795](https://github.com/FerroxLabs/wayland-core/commit/c19a795fde0dfa833e6463f7df66d3816fd465d6))
* **providers:** orchestrate paste-to-detect (fingerprint + validate) ([804373e](https://github.com/FerroxLabs/wayland-core/commit/804373ef44a94af336bc1f3ebca8174cc871f14e))
* **providers:** per-provider model-list disk cache (24h TTL) ([785704e](https://github.com/FerroxLabs/wayland-core/commit/785704ec5d8dbf3d854712187ca7d3ec7975ec5e))
* **providers:** wire Azure AadBearer auth mode at bootstrap (R77) ([6b13cb0](https://github.com/FerroxLabs/wayland-core/commit/6b13cb0fbfd54014e1e3fe83bad77b5f6495e4dc))
* **read:** semantic slicing — symbol= returns one definition, not the whole file (token-opt) ([86fc13b](https://github.com/FerroxLabs/wayland-core/commit/86fc13b26bf4b06176c1ce7653cfe12a70584715))
* **sandbox:** wire fs_read_allow/fs_write_allow to AppContainer DACLs (R61) ([114e7f4](https://github.com/FerroxLabs/wayland-core/commit/114e7f414cab2857ee62daa55097f83802335dc4))
* **sandbox:** WorkspacePolicy + OS secret-read-deny + Landlock Option A ([#59](https://github.com/FerroxLabs/wayland-core/issues/59)) ([dfa5aa2](https://github.com/FerroxLabs/wayland-core/commit/dfa5aa29c9d4f2a7cdf363f701339ed5147e37ad))
* Sign in with ChatGPT (OpenAI Codex OAuth) ([5ccc0fc](https://github.com/FerroxLabs/wayland-core/commit/5ccc0fcc48ecf1ccc7203277375c853069cf08c8))
* **skills:** per-skill max_turns/max_tokens fork-budget frontmatter overrides ([d9f5168](https://github.com/FerroxLabs/wayland-core/commit/d9f51683b2e112282e3648491f8c44800d8e4dba))
* **swarm:** wire Consensus/Debate as user-selectable reducer modes ([a1ec27e](https://github.com/FerroxLabs/wayland-core/commit/a1ec27e9523ff33360da32a0a10a432a48124079))
* **telegram:** batch multi-attachment replies via sendMediaGroup ([724570f](https://github.com/FerroxLabs/wayland-core/commit/724570ffb838e625835a1672ed0cc6dd776fc6f7))
* **tools:** bash_compact dispatcher skeleton + classifier fallback ([7de0ec1](https://github.com/FerroxLabs/wayland-core/commit/7de0ec10a50747e58760af76e7ecbae0fad22331))
* **tools:** block-aware cargo/git/testrun/grep output compactors ([f75655f](https://github.com/FerroxLabs/wayland-core/commit/f75655f369683af3e6bb65939ca46cb556eb36a9))
* **tools:** config key [tools] windows_shell for the PowerShell selector ([#45](https://github.com/FerroxLabs/wayland-core/issues/45)) ([130dc3d](https://github.com/FerroxLabs/wayland-core/commit/130dc3da1d4720ac407423125f058aacb6c2390d))
* **tools:** Parallel free search default + Firecrawl/Exa/SearXNG ladder ([3d0215f](https://github.com/FerroxLabs/wayland-core/commit/3d0215fcda19c26aca20e74d6957e48374bef819))
* **tools:** Parallel free search default + Firecrawl/Exa/SearXNG ladder ([6e4d5a9](https://github.com/FerroxLabs/wayland-core/commit/6e4d5a95ef453dd371172f6a0ddbb1881c593d08))
* **tui:** /model picker reads live cached models + refreshes on open ([f94e2c0](https://github.com/FerroxLabs/wayland-core/commit/f94e2c02561b6b9812b56ff3faede7547394d9f6))
* **tui:** /plugins marketplace overlay (Lane F2) ([0fe7b6b](https://github.com/FerroxLabs/wayland-core/commit/0fe7b6ba8f5ca633e6ac1c15b3ceb9acb28bf928))
* **tui:** add live Workflows drill-in pane (ForgeFlows-Live Phase 2) ([c6eeb4d](https://github.com/FerroxLabs/wayland-core/commit/c6eeb4d0d49e9c4096d72d9bcc58621e95475f77))
* **tui:** Advanced config tier — observability/storage/security editors (S6) ([94dc918](https://github.com/FerroxLabs/wayland-core/commit/94dc9182c22de94cf9bfe589f9ccce5dec2cc447))
* **tui:** arrow-key /model and /provider pickers (cross-provider) ([4b46606](https://github.com/FerroxLabs/wayland-core/commit/4b466061e4073a5a8443948cb512086998ff844a))
* **tui:** boot-screen provider discovery + Tab always switches tabs (FIX-5, FIX-7) ([b7f03d9](https://github.com/FerroxLabs/wayland-core/commit/b7f03d906b011f0cc12cf2118a6abe109c18fac8))
* **tui:** branded boot splash so a slow MCP connect never shows a blank screen ([9739f5d](https://github.com/FerroxLabs/wayland-core/commit/9739f5d7199d9c40525754ca1d02585c0a00d914))
* **tui:** collection list editors — tools/egress/failover (S7) ([299cdb7](https://github.com/FerroxLabs/wayland-core/commit/299cdb7432eddcf4162115bcd859f60473a8f0e1))
* **tui:** config-posture health section in /doctor (S8) ([4f1cb34](https://github.com/FerroxLabs/wayland-core/commit/4f1cb345fb4ab0b74710d823ab09a24620caf07d))
* **tui:** Essentials config home — Tools + Wallet rows, posture + health/cost (S5) ([fbaa431](https://github.com/FerroxLabs/wayland-core/commit/fbaa431d31beed947aad16869b511480323bf127))
* **tui:** make /provider picker connection-aware ([130bc72](https://github.com/FerroxLabs/wayland-core/commit/130bc7288d8c9522bae46b34a16a1ed98a18ca9e))
* **tui:** MCP servers section in /doctor ([3ae4841](https://github.com/FerroxLabs/wayland-core/commit/3ae48418d4c3265732667e6fd0e30dfb1db27cc4))
* **tui:** open the command palette with / from any surface ([2f21d06](https://github.com/FerroxLabs/wayland-core/commit/2f21d0688a71e0e956bc3d108a9bf6a9ef4f6fad))
* **tui:** paste-to-connect door in the Config Providers tier (FIX-3) ([e16f293](https://github.com/FerroxLabs/wayland-core/commit/e16f293abb407d7dac1d8a21a62159c9dd14d22f))
* **tui:** paste-to-detect modal state machine + view-model (S4a) ([6cb6e25](https://github.com/FerroxLabs/wayland-core/commit/6cb6e250425ee521177f88aeb3ad695bed628187))
* **tui:** resolve [@session](https://github.com/session) references to a past-session summary (rank 6) ([634a96b](https://github.com/FerroxLabs/wayland-core/commit/634a96b1da67ec8bfc24f11386aee4be05a60ce3))
* **tui:** resolve [@symbol](https://github.com/symbol) references via the repomap index (rank 6) ([20300a9](https://github.com/FerroxLabs/wayland-core/commit/20300a96649db3de31be95dd505e16f2393555cd))
* **tui:** resolve [@url](https://github.com/url) and [@output](https://github.com/output) — rank 6 complete (7/7 kinds) ([12c422a](https://github.com/FerroxLabs/wayland-core/commit/12c422a7c4e6db3b9b22884e6609e06358ea2a5b))
* **tui:** resolve @file/@dir/[@diff](https://github.com/diff) references into prompt context at send (rank 6) ([c34cffd](https://github.com/FerroxLabs/wayland-core/commit/c34cffd2a915b2db6c874f510429b81d34054f7e))
* **tui:** self-configure discovery section in /doctor (S11 v1) ([f01c9f9](https://github.com/FerroxLabs/wayland-core/commit/f01c9f940b1f8448bc054f10475df98e3feeda94))
* **tui:** thread MCP connect health into the inventory snapshot ([64bd809](https://github.com/FerroxLabs/wayland-core/commit/64bd809b306b9da8a73be54ce6db38733772c036))
* **tui:** wire the paste-to-detect /connect overlay (S4b) ([7b75549](https://github.com/FerroxLabs/wayland-core/commit/7b75549b8c2120c247dc6940cd5a840af5a01dd1))
* **types:** codex model aliases for openai-chatgpt ([daa6210](https://github.com/FerroxLabs/wayland-core/commit/daa6210a5ded3e1d95015ab1a0c195cbc9d18cca))


### Bug Fixes

* **#200:** unblock native Gemini egress + stop silent finish_reason=error turns ([#60](https://github.com/FerroxLabs/wayland-core/issues/60)) ([8d95578](https://github.com/FerroxLabs/wayland-core/commit/8d955782faf43d8c473606537337db0384ad0e9e))
* **#282:** tolerate live Flux context-overflow shapes (found by live E2E) ([#79](https://github.com/FerroxLabs/wayland-core/issues/79)) ([c5aadd6](https://github.com/FerroxLabs/wayland-core/commit/c5aadd636505fb008f5dfa735ff9b09d2b0fe18c))
* **#285:** never emit orphaned tool_result during compaction (DeepSeek 400) ([#75](https://github.com/FerroxLabs/wayland-core/issues/75)) ([5f3aaf7](https://github.com/FerroxLabs/wayland-core/commit/5f3aaf78d01d9bab3fbf80766e97761f024eb4df))
* **#293:** authenticate openai-chatgpt from ~/.codex/auth.json ([#80](https://github.com/FerroxLabs/wayland-core/issues/80)) ([7f0c7cc](https://github.com/FerroxLabs/wayland-core/commit/7f0c7cc1559526f5a5814fd72a8a099500218699))
* **acp:** honor --base-url on the ACP serve path ([da404e6](https://github.com/FerroxLabs/wayland-core/commit/da404e68a4e177b7b32c9b6835fd5556ae101acd))
* **agent,tools:** close two real Windows bugs (unbounded project-context walk + glob sandbox bypass) ([#64](https://github.com/FerroxLabs/wayland-core/issues/64)) ([fea2c52](https://github.com/FerroxLabs/wayland-core/commit/fea2c52f6069f1e32f1bfbcb7640818a7820b397))
* **agent:** 429 is retryable + honest introspection sort key (ranks 48, 60) ([6232264](https://github.com/FerroxLabs/wayland-core/commit/623226424817aaac24f97dafb1efe29857f59767))
* **agent:** dispatch plugin hooks for TurnStart/TurnEnd/PostToolUse into context ([ec0d320](https://github.com/FerroxLabs/wayland-core/commit/ec0d32067e20c2884f929c076620965ca724adeb))
* **agent:** don't surface a cache full-miss as a user error ([573d879](https://github.com/FerroxLabs/wayland-core/commit/573d879aa3ef893c53caea7b159a8de7d3fcceb9))
* **agent:** don't surface a cache full-miss as a user error ([#101](https://github.com/FerroxLabs/wayland-core/issues/101)) ([3c5def2](https://github.com/FerroxLabs/wayland-core/commit/3c5def2e7a8dedca4d82c208e20cbeafef9141a8))
* **agent:** feed ResilientProvider its fallback chain from config ([3dc0b31](https://github.com/FerroxLabs/wayland-core/commit/3dc0b31f0ba8cc4c22138f72109e9726146cd6c9))
* **agent:** implement SSRF-safe remote video download for VideoSource::RemoteUrl ([4654a25](https://github.com/FerroxLabs/wayland-core/commit/4654a25a49bea59edbdd9a3cf4bd0853707b1b70))
* **agent:** implement TLS for postgres_schema (sslmode=require/verify-ca/verify-full) ([96f7601](https://github.com/FerroxLabs/wayland-core/commit/96f76011ef5a39d855490a50bf8363cb11370207))
* **agent:** manual /compact runs real micro-compaction, not canned truncation ([2cd4d9d](https://github.com/FerroxLabs/wayland-core/commit/2cd4d9de62b85a25c540f6883941b42423174297))
* **agent:** stop registering Piper TTS backend until synthesis lands ([bc63850](https://github.com/FerroxLabs/wayland-core/commit/bc63850e90aa097bd10beb3bc920137e3176137f))
* **agent:** surface workflow no-barrier pipeline partial failures (rank 51) ([91974f3](https://github.com/FerroxLabs/wayland-core/commit/91974f38b3bcb2b51387303c0930d66d81179b3c))
* **agent:** wire engine writers to session state so status/telemetry aren't zero (rank 38) ([714d508](https://github.com/FerroxLabs/wayland-core/commit/714d5083ca628a9968a61c9e6d4373e312f91eea))
* **agent:** wire StreamingContext in production so tool-chunk streaming works ([eb1c5c7](https://github.com/FerroxLabs/wayland-core/commit/eb1c5c7708f4289c3b3989617c3803b2d216d71c))
* ALSA optional ([#14](https://github.com/FerroxLabs/wayland-core/issues/14)), config scroll ([#16](https://github.com/FerroxLabs/wayland-core/issues/16)), mutation-nightly noise ([#1](https://github.com/FerroxLabs/wayland-core/issues/1)-[#5](https://github.com/FerroxLabs/wayland-core/issues/5)) ([cf784da](https://github.com/FerroxLabs/wayland-core/commit/cf784daa2d294c60d36253be845f19774507b618))
* **audio:** make ALSA optional behind off-by-default `voice` feature ([e0e9dae](https://github.com/FerroxLabs/wayland-core/commit/e0e9daec26a5472dcd4b819fbdbd055042096436)), closes [#14](https://github.com/FerroxLabs/wayland-core/issues/14)
* **audit:** 19 low/medium defects — browser, sandbox, channels, tools, TUI ([8c589ad](https://github.com/FerroxLabs/wayland-core/commit/8c589ad36be0e4e8605ca1e49c770a52ce6f3385))
* **audit:** 7 high-severity defects — sandbox, provider protocol, unbounded reads ([8273b2a](https://github.com/FerroxLabs/wayland-core/commit/8273b2ac1e56937e816101c45415954a6d4ea6b6))
* **audit:** provider resilience + egress/secret hygiene (8 fixes) ([0e893d9](https://github.com/FerroxLabs/wayland-core/commit/0e893d99f38b623a4deaa65ea27d3c51c424c8eb))
* **bash:** fall back to cmd when PowerShell shell is selected under AppContainer ([#105](https://github.com/FerroxLabs/wayland-core/issues/105)) ([d698c66](https://github.com/FerroxLabs/wayland-core/commit/d698c663f0f361912ed25f532a83e519305c246a))
* **browser:** AxTree/Snapshot honesty on Browserbase + Chromium hint surfacing (ranks 62, 65) ([4e42adb](https://github.com/FerroxLabs/wayland-core/commit/4e42adbbfb95b1ad33d38917ad834b1b350dc142))
* **browser:** schedule the healthcheck loop + drop terminated child handles (ranks 63, 64) ([aaf7649](https://github.com/FerroxLabs/wayland-core/commit/aaf764918b5949fc360cf6dc9b8c850a1565c7b3))
* **channels:** allowlist-gate Discord/Slack inbound media fetch (SSRF + token exfil) ([d34e72d](https://github.com/FerroxLabs/wayland-core/commit/d34e72dba566279ba2504cc56ae7ae04d7eafe3b))
* **channels:** bound channel media downloads to prevent OOM DoS (rank 22) ([a108539](https://github.com/FerroxLabs/wayland-core/commit/a1085393ea39a857001e73cccd5a0923cf0740be))
* **channels:** close 7 findings from the cross-audit of phases 3-7 ([822205f](https://github.com/FerroxLabs/wayland-core/commit/822205f819aa4f66ee941d270219e4f0b38ab9cc))
* **channels:** decouple inbound dispatch from the broadcast drain loop (R13) ([d926b6c](https://github.com/FerroxLabs/wayland-core/commit/d926b6c0a508d0670f500ec3ec917a5b9fee516d))
* **channels:** detect dead connector background tasks and trigger supervised reconnect ([706b78e](https://github.com/FerroxLabs/wayland-core/commit/706b78e50ac693967893a03bb6f00383fa072692))
* **channels:** start_all continues past per-channel failures (rank 15) ([7d2a8fa](https://github.com/FerroxLabs/wayland-core/commit/7d2a8fa29f18cc81bafcf0f22a79bf092bbf10cd))
* **channels:** thread reply-context so the bot can quote back (WhatsApp + shared) ([f81272b](https://github.com/FerroxLabs/wayland-core/commit/f81272b4bb1ba733b3b14e376cfd5481663648d0))
* **channels:** three bugs the live Telegram test surfaced ([717b384](https://github.com/FerroxLabs/wayland-core/commit/717b3848c0d478d563bbbaa4d05b30187ad0fc33))
* **ci:** disposition rsa RUSTSEC-2023-0071 (Marvin Attack) to unblock audit ([3160f4d](https://github.com/FerroxLabs/wayland-core/commit/3160f4dda36e38b736e8a3364ba629b0d3a1b333))
* **ci:** disposition rsa RUSTSEC-2023-0071 (Marvin Attack) to unblock audit ([bf2211d](https://github.com/FerroxLabs/wayland-core/commit/bf2211db7d5cad206ac159c0547fbaf58daa61e4))
* **cli:** enable monitor/review_artifact approval components by default (rank 90) ([f20f959](https://github.com/FerroxLabs/wayland-core/commit/f20f95982f1c7ea6f47cfa8dd2ff9ed4c8af6d5e))
* **cli:** make `plugin list` marketplace-aware and sidecar-tolerant ([b48f76a](https://github.com/FerroxLabs/wayland-core/commit/b48f76adb8c6ea3f4972f78c5bebe11c19d31ae5))
* **cli:** probe real audio input devices for voice_mode status (rank 46) ([771e495](https://github.com/FerroxLabs/wayland-core/commit/771e4950301affd739ac8a440a3e8c0f49273cf0))
* **cli:** reject absolute source/subdir paths in quarantine (path traversal) ([5d3d615](https://github.com/FerroxLabs/wayland-core/commit/5d3d6158b15e9685ec193bda5af1fbbb7b4b979d))
* **cli:** surface a clear, Ollama-aware reason on init failure instead of bare exit 1 ([#186](https://github.com/FerroxLabs/wayland-core/issues/186)) ([#61](https://github.com/FerroxLabs/wayland-core/issues/61)) ([b37b3d1](https://github.com/FerroxLabs/wayland-core/commit/b37b3d12663fdf45b472933bf5eb12f0164fc8db))
* **cli:** wire the --no-memory flag (rank 47) ([f5e3881](https://github.com/FerroxLabs/wayland-core/commit/f5e3881de7a7f19dbd28875fb38f68dc4afbdb52))
* **config:** default credentials to keyring with plaintext fallback (F16) ([6c57160](https://github.com/FerroxLabs/wayland-core/commit/6c5716080da4429f32a0ccfc9acd0399cfe6bd3f))
* **config:** stop legacy-yaml migration from clobbering config.toml every launch ([93d8852](https://github.com/FerroxLabs/wayland-core/commit/93d8852018a1073874177972a05a4ca79b52ad42))
* **core:** Windows MCP stdio launch ([#164](https://github.com/FerroxLabs/wayland-core/issues/164)) + Anthropic unrecoverable-conversation ([#161](https://github.com/FerroxLabs/wayland-core/issues/161)) ([38b85e6](https://github.com/FerroxLabs/wayland-core/commit/38b85e6fb6895100e24218366586b08da6dd62d4))
* **cron:** dispatch daemon jobs + stop falsely advancing last_fired on no-sink fires ([d39d9cd](https://github.com/FerroxLabs/wayland-core/commit/d39d9cd1e3f40aeace5b5b73595f431822ab1002))
* **cua:** AxTree returns a typed gap error instead of an empty stub ([b2b1a79](https://github.com/FerroxLabs/wayland-core/commit/b2b1a79d9c834cc592fd89161611f5a00682b7a6))
* **deps:** bump postgres-protocol/tokio-postgres to clear RUSTSEC-2026-0178/0179/0180 ([067dee3](https://github.com/FerroxLabs/wayland-core/commit/067dee3eb50316c030676e3e97d41ed622500703))
* **discord:** resolve bot_id at start + add DIRECT_MESSAGES intent (bot can reply) ([e08520a](https://github.com/FerroxLabs/wayland-core/commit/e08520aebf383cc4d75b1dab4fad0b75855716fa))
* **egress:** allowlist Flux Router out of the box + accept full-host entries ([1fa6407](https://github.com/FerroxLabs/wayland-core/commit/1fa6407e907227e7c09b7431e968dbd3920e95d0))
* **egress:** allowlist NVIDIA NIM, Cerebras, MiniMax-failover & Qwen hosts ([#48](https://github.com/FerroxLabs/wayland-core/issues/48)) ([a68f2d9](https://github.com/FerroxLabs/wayland-core/commit/a68f2d917f8c950004a9d92ba57cce9d759cbe4d))
* **email:** accept only data: URLs for outbound attachments (drop local-path read) ([dd57ab9](https://github.com/FerroxLabs/wayland-core/commit/dd57ab9ea013b4b99e592cfd3fda89eb7ebfc4f0))
* **email:** retain inbound attachment bytes + serve them via fetch_media ([ce6e88e](https://github.com/FerroxLabs/wayland-core/commit/ce6e88e99f38339406dd11dee2edc22b512bb160))
* **email:** seed + persist IMAP UID watermark (no mailbox replay on connect/restart) ([a8b8e67](https://github.com/FerroxLabs/wayland-core/commit/a8b8e67214ecca6c43287c4c7c1d3d6c1fcf1e51))
* **email:** send outbound attachments as multipart/mixed (was silently dropped) ([ee35adf](https://github.com/FerroxLabs/wayland-core/commit/ee35adfe8b8572d38f0dc7283ba49db697cc0d61))
* **engine:** app-reported tickets — NVIDIA base_url ([#48](https://github.com/FerroxLabs/wayland-core/issues/48)), orphaned tool results ([#85](https://github.com/FerroxLabs/wayland-core/issues/85)), silent local-endpoint responses ([#86](https://github.com/FerroxLabs/wayland-core/issues/86)) ([c26eb48](https://github.com/FerroxLabs/wayland-core/commit/c26eb48c5d7809e7af2830d7b24a6d39f084b489))
* **engine:** never let the reasoning budget starve the visible answer ([#426](https://github.com/FerroxLabs/wayland-core/issues/426)) ([#107](https://github.com/FerroxLabs/wayland-core/issues/107)) ([60f8e7d](https://github.com/FerroxLabs/wayland-core/commit/60f8e7d649a4a2fa684c4228b620a9ea8d0491fd))
* **eval-scenarios:** scope the cfg(unix)-only config read to its block (Windows clippy) ([1cbf782](https://github.com/FerroxLabs/wayland-core/commit/1cbf782ada1aa8ed43cfc75d81fcbeba4fda719f))
* **evolve:** persist GEPA winners to evolved_prompts (record_outcome) ([291f577](https://github.com/FerroxLabs/wayland-core/commit/291f5777d8fab48e8a29dfa174c3215a2bcf6b82))
* **evolve:** stamp real generation into curator Lineage (rank 55) ([5d23f34](https://github.com/FerroxLabs/wayland-core/commit/5d23f34fc3e2d384664d92944a5e2568f380ddb7))
* **forge-mcp:** close token-exfil SSRF + 4 reliability defects in discovery flow ([bd2f40d](https://github.com/FerroxLabs/wayland-core/commit/bd2f40d23aa98d64aff2406f5e7d6b8b45a304ba))
* **gemini:** inject default items for array schemas so tool registration stops 400ing ([16b91a1](https://github.com/FerroxLabs/wayland-core/commit/16b91a1a57ab193a5e7f72e1dba4d4350b0e4f00))
* **gemini:** split SSE frames on CRLF so 2.5-class models stop false-truncating ([5daf551](https://github.com/FerroxLabs/wayland-core/commit/5daf55137977ba4fb1ff197d9afc9d9a3876cbf2))
* **hooks:** byte-compare trust-tag defang to avoid multibyte panic (audit r2) ([35c044d](https://github.com/FerroxLabs/wayland-core/commit/35c044da0f706861fd453a520899b19bb0398b56))
* **hooks:** correct PrePrompt ephemeral dedup; defang recall block (F3/F1) ([a2a4657](https://github.com/FerroxLabs/wayland-core/commit/a2a46571d0d5acd6da9674b6e2d599f7517424e8))
* **hooks:** defang trust-tag delimiters + sanitize source ident (audit F1/F2) ([5215d98](https://github.com/FerroxLabs/wayland-core/commit/5215d98b996901770950e9638d7441ce86329756))
* **ijfw:** PATHEXT-aware npx detection on Windows (closes [#6](https://github.com/FerroxLabs/wayland-core/issues/6)) ([873c14d](https://github.com/FerroxLabs/wayland-core/commit/873c14ddef572b2f307f0cf311b3f98a7608a93c))
* **ijfw:** PATHEXT-aware npx detection on Windows (closes [#6](https://github.com/FerroxLabs/wayland-core/issues/6)) ([4730df2](https://github.com/FerroxLabs/wayland-core/commit/4730df28aee813d60700a233e05937452221fd83))
* **ijfw:** run the blocking MCP reachability probe off the async worker (rank 57) ([b4a1322](https://github.com/FerroxLabs/wayland-core/commit/b4a1322dd1a1d8f038dc393e4450148600697e81))
* **imessage:** address group/DM replies by chat GUID, not a hex heuristic ([38e3831](https://github.com/FerroxLabs/wayland-core/commit/38e38319089cb98303ab7fdf54dae1896f7734cf))
* **imessage:** extract inbound attachments + read them via fetch_media ([16d09fd](https://github.com/FerroxLabs/wayland-core/commit/16d09fdba71a96a265281e1c20b1bbf130f7b136))
* **matrix:** route 1:1 rooms as Direct + declare a message-length cap ([a2c8f69](https://github.com/FerroxLabs/wayland-core/commit/a2c8f6985f8a2dd2819a6725dd065a9b9863e63c))
* **mcp/bootstrap:** cap hook response + guard ambiguous binding (F4/F5/F6/F10) ([089f2be](https://github.com/FerroxLabs/wayland-core/commit/089f2beb73f9cdd82ccfef77f5ebcdf59e11142f))
* **mcp:** bound the SSE POST send with a timeout (rank 25) ([f697bbc](https://github.com/FerroxLabs/wayland-core/commit/f697bbc32c3db94f0c808a5e65c25388e80fa831))
* **mcp:** collapse nested if in tool dispatch; fmt wired features ([2867773](https://github.com/FerroxLabs/wayland-core/commit/286777333a8a19bb19b7ad82549606db5da42a30))
* **mcp:** don't caret-escape the program name in Windows stdio launch ([371f619](https://github.com/FerroxLabs/wayland-core/commit/371f619ee47f1c9beb8d4b984c6f8acc979ce132))
* **mcp:** reap stdio grandchildren + fail SSE waiters on listener exit (ranks 24, 26) ([44d9f24](https://github.com/FerroxLabs/wayland-core/commit/44d9f24638b0f4a1f25395faffe82a62d0bf59a7))
* **mcp:** surface id-less JSON-RPC error frames instead of discarding them (rank 86) ([91e7237](https://github.com/FerroxLabs/wayland-core/commit/91e7237256c554e34838a667d77cafbd263bac0b))
* **memory:** persist per-skill latency_ms instead of discarding it (rank 75) ([0002b26](https://github.com/FerroxLabs/wayland-core/commit/0002b2660d5cd1d1f23bb1a1c006391f1580ad32))
* **memory:** select embedder from config instead of hardcoding HashedEmbedder ([276f41b](https://github.com/FerroxLabs/wayland-core/commit/276f41b17cfae49cff6b0095d930fecd726e50f1))
* **model-catalog:** tag a floored model fetch BuiltIn, not a live "synced" ([0bca1a7](https://github.com/FerroxLabs/wayland-core/commit/0bca1a7545c8a5e4d8e7fa155e63f1e694d3014c))
* **model-picker:** load UI-saved provider keys + connection-aware live /model picker ([3a8929f](https://github.com/FerroxLabs/wayland-core/commit/3a8929fd45e9c5ef26ddabe79cf1904d570fd931))
* **msteams:** flag bot-role activities as is_bot (loop guard) ([b0978b7](https://github.com/FerroxLabs/wayland-core/commit/b0978b7214e6b5c6e2ca86e85757a879f52bd7ec))
* **msteams:** implement send_typing (Bot Framework typing activity) ([e9108c7](https://github.com/FerroxLabs/wayland-core/commit/e9108c7ef561f1d594ca8078d3179a31818533a8))
* **msteams:** stop reflecting OAuth2 token-fetch body into errors/logs (rank 28) ([9cb4fe3](https://github.com/FerroxLabs/wayland-core/commit/9cb4fe3147c9b84406cbcb1d8bbf3d3c93a2a333))
* **oauth:** stop advertising a non-existent `wayland auth login grok` command ([#47](https://github.com/FerroxLabs/wayland-core/issues/47)) ([42e16ec](https://github.com/FerroxLabs/wayland-core/commit/42e16ec5009883a1cff42478f2d347ac4fee7a13))
* **observability:** scrub tool input before storing it on ToolCallTrace (rank 59) ([c133b72](https://github.com/FerroxLabs/wayland-core/commit/c133b72e415e7bcc9bfdafd8b74db65818573095))
* **ollama:** fail loudly on ToolUse/ToolResult blocks (rank 36/17) ([3bcd542](https://github.com/FerroxLabs/wayland-core/commit/3bcd54250b2ca7a7237c5f985ea5691960d1d952))
* **ollama:** make base URL and default model configurable (rank 56) ([4201225](https://github.com/FerroxLabs/wayland-core/commit/4201225c5363866d33036fcbb87e6a469cc22e0a))
* OpenAI image default (gpt-image-1) + DeepSeek v4-flash 1M context ([#265](https://github.com/FerroxLabs/wayland-core/issues/265), [#255](https://github.com/FerroxLabs/wayland-core/issues/255)) ([#69](https://github.com/FerroxLabs/wayland-core/issues/69)) ([30dad57](https://github.com/FerroxLabs/wayland-core/commit/30dad572cb15b2ff3cdb0d7f2b936525d7e5ac06))
* OpenAI tool-name sanitization ([#297](https://github.com/FerroxLabs/wayland-core/issues/297)) + WSL canonicalize off-reactor ([#287](https://github.com/FerroxLabs/wayland-core/issues/287)) ([#84](https://github.com/FerroxLabs/wayland-core/issues/84)) ([af69bdc](https://github.com/FerroxLabs/wayland-core/commit/af69bdc046bef94671426a20a8a1fb7327c91d30))
* **orchestration:** coerce unwired non-Direct templates to honest Direct turns (rank 5) ([f717db6](https://github.com/FerroxLabs/wayland-core/commit/f717db6c8ba08cd899dcc87f646c8adf7f3f5bad))
* **plugin-api:** gate ScopedMemoryClient behind a manifest permission check (rank 79) ([9fc80d6](https://github.com/FerroxLabs/wayland-core/commit/9fc80d6883e730d6480e29d1906b97537e3f299e))
* **pricing,providers:** load custom WAYLAND_PRICING_PATH catalogs, resolve dotted model slugs, omit temperature for Opus 4.x ([#93](https://github.com/FerroxLabs/wayland-core/issues/93)) ([d4159e0](https://github.com/FerroxLabs/wayland-core/commit/d4159e057ec9a15da8fcbaefaf45b489e82bccc2))
* **protocol:** add explicit Capabilities.memory_enabled flag (rank 85) ([6544f16](https://github.com/FerroxLabs/wayland-core/commit/6544f16c811aac96c93820dbc489cfbc55019ed6))
* **protocol:** cap stdin line length to stop newline-less OOM DoS ([8b8c1d7](https://github.com/FerroxLabs/wayland-core/commit/8b8c1d77945fee96c0077d2c7cde2289f5e1cf4c))
* **providers,agent:** surface empty/incompatible OpenAI-compatible responses ([57b02e7](https://github.com/FerroxLabs/wayland-core/commit/57b02e73b85c0bd8a3f6f60d30b7fec1258674e0))
* **providers:** accept codex response.done/incomplete as terminal frames ([0bc0ed6](https://github.com/FerroxLabs/wayland-core/commit/0bc0ed62a96ef8048c67e8a56e962a1ed8f93cff))
* **providers:** Bedrock/Vertex "connected" only with real ambient credentials ([7245065](https://github.com/FerroxLabs/wayland-core/commit/72450658c87fb78c642a91b54ce041f5dcf7cc1d))
* **providers:** cap Bedrock buffered response body to bound memory (rank 23) ([3370b49](https://github.com/FerroxLabs/wayland-core/commit/3370b49fc156bab166b92182c9c06bfa24f2f509))
* **providers:** default moonshot/qwen to international endpoints + pin api_path ([d15429c](https://github.com/FerroxLabs/wayland-core/commit/d15429ca99ebc3e8147b65689a7308ded3f3b023))
* **providers:** don't request encrypted reasoning until we round-trip it ([52eeceb](https://github.com/FerroxLabs/wayland-core/commit/52eecebb3ae3ea70caa4d074a1b4cc68b9890ef4))
* **providers:** drop unsigned thinking blocks when building Anthropic messages ([cdd0968](https://github.com/FerroxLabs/wayland-core/commit/cdd0968dc66acf53471748ebdd40c460b2630b3c))
* **providers:** drop unused json import; lock socket2/base64 direct edges ([fd9100e](https://github.com/FerroxLabs/wayland-core/commit/fd9100ec250b2cc674887ed47d2cb48e437f5ff6))
* **providers:** forward list_models on OpenAI-compat newtypes (paste-connect) ([efbddba](https://github.com/FerroxLabs/wayland-core/commit/efbddba218df0f854f914a7ee77ff9e4b2fd324d))
* **providers:** keyless self-hosted endpoints (no more "OpenAI API key is required" on local Ollama) ([#102](https://github.com/FerroxLabs/wayland-core/issues/102)) ([28d5eac](https://github.com/FerroxLabs/wayland-core/commit/28d5eac64851b9e404bd371f59768ee41890d9e9))
* **providers:** make MiniMax visible in pickers + bound tool-input accumulator ([e8ac0f2](https://github.com/FerroxLabs/wayland-core/commit/e8ac0f29642e75a97143ec73d9172cb185f5eb1a))
* **providers:** non-empty error bodies + OpenAI chat cache_read_tokens (ranks 68, 39) ([c5c4adc](https://github.com/FerroxLabs/wayland-core/commit/c5c4adc8d8e7905de1059f24b9dcf0726fe6c0eb))
* **providers:** normalize OpenAI-compatible tool schemas so strict servers accept them ([107f244](https://github.com/FerroxLabs/wayland-core/commit/107f244df5647f8681fc38bb29bbfcc9e8e9de2a))
* **providers:** pin api_path so 8 native providers stop 404ing out of the box ([bd2cc89](https://github.com/FerroxLabs/wayland-core/commit/bd2cc898181aa21f554a3fa8c1e3a3dbceeb63e6))
* **providers:** provider auth robustness — Grok OAuth, region failover, auth errors ([#42](https://github.com/FerroxLabs/wayland-core/issues/42)) ([4dfc566](https://github.com/FerroxLabs/wayland-core/commit/4dfc566af50b6a233f4543e837f84efa5ee8490a))
* **providers:** replay reasoning_content for strict reasoners routed via a router ([#417](https://github.com/FerroxLabs/wayland-core/issues/417)) ([#108](https://github.com/FerroxLabs/wayland-core/issues/108)) ([fac4bde](https://github.com/FerroxLabs/wayland-core/commit/fac4bde7eecc2b3c31ec7c20927034432eea4bfa))
* **providers:** ResilientProvider delegates alias_key/list_models to primary ([4c409c1](https://github.com/FerroxLabs/wayland-core/commit/4c409c1da6e5506c615a9279cbd092f41bcb56fe))
* **providers:** strip empty/missing tool_call_id before sending (DeepSeek 400 guard) ([#50](https://github.com/FerroxLabs/wayland-core/issues/50)) ([c97424d](https://github.com/FerroxLabs/wayland-core/commit/c97424d463f5e976c1e2863db65cebaf74b0a6a7))
* **providers:** strip orphaned tool results in OpenAI builder ([b59a344](https://github.com/FerroxLabs/wayland-core/commit/b59a3446c0432e1310cce2c1c3f84267cdfedd57))
* **read:** guard the unchanged-file stub against compaction + sub-agents ([1ceca0f](https://github.com/FerroxLabs/wayland-core/commit/1ceca0f81a75048c8be9e9ca7e999c62b0205dd4))
* **read:** stop post-write reads from false-stubbing on stale content (token-opt) ([6da2205](https://github.com/FerroxLabs/wayland-core/commit/6da22050f99fdf2c27aadb1a09d1002e68726ea6))
* **sandbox:** reap AppContainer job tree before draining pipes ([#100](https://github.com/FerroxLabs/wayland-core/issues/100)) ([#101](https://github.com/FerroxLabs/wayland-core/issues/101)) ([a635fef](https://github.com/FerroxLabs/wayland-core/commit/a635fef5a7b21c3fb2c45d24e58e787c222090da))
* **sandbox:** skip non-existent paths in AppContainer DACL grant/deny ([#321](https://github.com/FerroxLabs/wayland-core/issues/321)-324 follow-up) ([#99](https://github.com/FerroxLabs/wayland-core/issues/99)) ([b22f515](https://github.com/FerroxLabs/wayland-core/commit/b22f515396697c5569d829beb115a0ebc1e625b0))
* **security:** close Grep RCE, skill/rules prompt injection, hook shell injection ([5773326](https://github.com/FerroxLabs/wayland-core/commit/57733260b278621f1cacad384bfa82b3893e8a33))
* **security:** migrate jsonwebtoken to aws_lc_rs, drop rsa (RUSTSEC-2023-0071) ([d6c42c0](https://github.com/FerroxLabs/wayland-core/commit/d6c42c0580bb5b140a4dedeb821a44a16587e143))
* **security:** migrate jsonwebtoken to aws_lc_rs, drop rsa (RUSTSEC-2023-0071) ([53a4cb7](https://github.com/FerroxLabs/wayland-core/commit/53a4cb7d8c6d7fc1419397c6d1b0c05e365c5445))
* **security:** reject flag-shaped [@diff](https://github.com/diff) base ref (argv flag smuggling) ([1e4a9a3](https://github.com/FerroxLabs/wayland-core/commit/1e4a9a35605c35d3675cc980ef69fdd5535ee7fe))
* **self-update:** verify keyless provenance via gh, drop dead ed25519 path (R16) ([42fd1a0](https://github.com/FerroxLabs/wayland-core/commit/42fd1a0a415cd38a03346796b11e088cd8c73cd5))
* **shell:** accept .exe and absolute-path Windows shell selectors ([#197](https://github.com/FerroxLabs/wayland-core/issues/197)) ([#62](https://github.com/FerroxLabs/wayland-core/issues/62)) ([9b332e7](https://github.com/FerroxLabs/wayland-core/commit/9b332e7eedc9bf4ec9141dbbdceaff6b01a3873b))
* **signal:** surface inbound attachments + read them via fetch_media ([da6a3c6](https://github.com/FerroxLabs/wayland-core/commit/da6a3c62a21843f61344ae0e10ae4208f6f70234))
* **skills:** hide unreviewed auto-drafted skills from the model catalog ([#56](https://github.com/FerroxLabs/wayland-core/issues/56)) ([a2c0de4](https://github.com/FerroxLabs/wayland-core/commit/a2c0de415e8ce51ee8f0232b8590119276d6e152))
* **skills:** keep the hello test fixture out of the shipped catalog ([#55](https://github.com/FerroxLabs/wayland-core/issues/55)) ([35d334f](https://github.com/FerroxLabs/wayland-core/commit/35d334f7f10b7ca215fb1c674fbb7c64e654f507))
* **slack:** handle app_mention events and set was_mentioned (channel mentions work) ([68e4b57](https://github.com/FerroxLabs/wayland-core/commit/68e4b57e3d568eb93881ba03cb4e33e2bd8910f0))
* **sms:** implement fetch_media for inbound MMS media (Twilio) ([f638d68](https://github.com/FerroxLabs/wayland-core/commit/f638d68f55a28c4fa96e9bd382e5250b0b57a95f))
* **spawner:** propagate host cancel into spawned sub-agents ([a1090bd](https://github.com/FerroxLabs/wayland-core/commit/a1090bd85050868bad46e43885550f5add050055))
* **streaming:** stop UTF-8 corruption of split codepoints + fail closed on bad tool JSON ([b2603bc](https://github.com/FerroxLabs/wayland-core/commit/b2603bce89a6f97145a90c78a836a8470e2603f1))
* **telegram:** bound outbound sends with a request timeout ([63f1e69](https://github.com/FerroxLabs/wayland-core/commit/63f1e69960ee3cf86aa5004732a06614452694de))
* **telegram:** circuit-break on auth failure instead of retrying 401 forever (rank 29) ([b58da62](https://github.com/FerroxLabs/wayland-core/commit/b58da6239018a37fbf21dab3cb21676309f13d54))
* **telegram:** coalesce media-group albums into one inbound message (rank 31) ([46c1ab4](https://github.com/FerroxLabs/wayland-core/commit/46c1ab4c2e513535661ef56ed66af64135d1321c))
* **telegram:** HTML-escape replies + deleteWebhook on start (ranks 34, 19) ([503d342](https://github.com/FerroxLabs/wayland-core/commit/503d342d74749356f7950ec733abafa22625c5ea))
* **telegram:** model sticker/audio/video_note inbound + pin allowed_updates (ranks 30, 32) ([ee3e5ba](https://github.com/FerroxLabs/wayland-core/commit/ee3e5ba8f3c470f764595fdd8fe352273b32f0c2))
* **telegram:** never store the bot token in Attachment.url (resolve lazily) ([9344368](https://github.com/FerroxLabs/wayland-core/commit/93443680b89f0950098d4725b0de71c3c236e293))
* **telegram:** persist getUpdates offset + handle channel_post (ranks 33, 35) ([29f5ce3](https://github.com/FerroxLabs/wayland-core/commit/29f5ce3b1bdc3393f621b2d95df58008a22d0326))
* **telegram:** read inbound media caption (rank 18) ([0528a54](https://github.com/FerroxLabs/wayland-core/commit/0528a540e05b8f9c7e28330d452618fdbcfbff48))
* **telegram:** surface edited_message inbound instead of silently dropping it (rank 83) ([d55d99a](https://github.com/FerroxLabs/wayland-core/commit/d55d99aaafa74057ef2d5345f763ebf37f87cfa2))
* **telegram:** truthful abort on stop() + enforce documented poll-timeout cap (ranks 81, 82) ([94ed8bb](https://github.com/FerroxLabs/wayland-core/commit/94ed8bbbc2b72e7a99f772079d966326a576ecc3))
* **token-opt:** clippy doc-lint + workspace fmt after fan-out integration ([04b598f](https://github.com/FerroxLabs/wayland-core/commit/04b598fa4ff12d46bd3aea05537c5d3b0c6c4e49))
* **tools:** bash_compact parses `2>&1` correctly + refresh stale voice_mode doc (ranks 74, 88) ([cc85652](https://github.com/FerroxLabs/wayland-core/commit/cc856525c936455058ca616e06b3124f26dfb7d8))
* **tools:** content-equality read dedup to stop token-burn re-reads ([ac6f77d](https://github.com/FerroxLabs/wayland-core/commit/ac6f77da316980ca1b224b4e8752a998c1753de9))
* **tools:** content-equality read dedup to stop token-burn re-reads ([#61](https://github.com/FerroxLabs/wayland-core/issues/61)) ([c9f6367](https://github.com/FerroxLabs/wayland-core/commit/c9f63676fdaa53c6d52a4f9d9581c21f8e7121cd))
* **tools:** wire fuzzy_find_and_replace as an opt-in EditTool fallback (rank 41) ([72b1a2f](https://github.com/FerroxLabs/wayland-core/commit/72b1a2f3d0ef8747b40389bd876eb642807c2400))
* **tui,orchestration:** resolve ForgeFlows-Live cross-audit findings ([395e51d](https://github.com/FerroxLabs/wayland-core/commit/395e51dcdbaab2d81c6121e552f8e63eaecdbc64))
* **tui:** Config Esc saves pending toggles instead of reverting ([854f065](https://github.com/FerroxLabs/wayland-core/commit/854f0657843aee2ce2b4af0e0029adfedec45d62))
* **tui:** scroll the providers config pane to keep the focused row visible ([fa2226e](https://github.com/FerroxLabs/wayland-core/commit/fa2226e760ae6a29ad6c20f18e7cbe5d7dcd2bcd)), closes [#16](https://github.com/FerroxLabs/wayland-core/issues/16)
* **tui:** show em-dash for unrecorded spend in the status bar ([f8e5d65](https://github.com/FerroxLabs/wayland-core/commit/f8e5d6540a370d3a3398161c2e15437da3127f85))
* **tui:** stop /doctor from freezing the whole TUI on live probes ([4121652](https://github.com/FerroxLabs/wayland-core/commit/4121652ebd66cae28084d67d3d64ea6107da020c))
* **tui:** widen Advanced label pad so the value isn't glued to it ([1cb6578](https://github.com/FerroxLabs/wayland-core/commit/1cb65780e38e374606454eea865d520b20798087))
* **web:** bound readability extraction + reset breakers per turn + telemetry schema ([#403](https://github.com/FerroxLabs/wayland-core/issues/403)) ([#106](https://github.com/FerroxLabs/wayland-core/issues/106)) ([43c7aac](https://github.com/FerroxLabs/wayland-core/commit/43c7aac819e649c72383edd58be446d94856ace7))
* **whatsapp:** send outbound attachments + revalidate media host before token (rank 27 + outbound-attachments) ([1cbd0a0](https://github.com/FerroxLabs/wayland-core/commit/1cbd0a0cffe7e805871db82551a63330cba0aa2b))
* **windows:** 4 Windows-only failures ([#257](https://github.com/FerroxLabs/wayland-core/issues/257) CRLF edit, [#262](https://github.com/FerroxLabs/wayland-core/issues/262)/[#263](https://github.com/FerroxLabs/wayland-core/issues/263) MCP stdio quoting, [#267](https://github.com/FerroxLabs/wayland-core/issues/267) sandbox \\?\ path) ([#72](https://github.com/FerroxLabs/wayland-core/issues/72)) ([d7ccbef](https://github.com/FerroxLabs/wayland-core/commit/d7ccbef78194fbbb7ad5ed7e87c7f0afb5370f0f))


### Performance Improvements

* **agent:** release the turn-cell lock before agent dispatch so parallel graph siblings don't serialize ([922c87f](https://github.com/FerroxLabs/wayland-core/commit/922c87f852ca0d01310fcab7d6702e511fafda92))
* **cache:** stabilize the prompt-cache prefix across turns (token-opt) ([dffd20e](https://github.com/FerroxLabs/wayland-core/commit/dffd20e44b9f2b92a5ed54dd3898a277bb1b8fb6))
* **channels:** don't hold the channel-map borrow across async webhook ingest (rank 73) ([2c8a217](https://github.com/FerroxLabs/wayland-core/commit/2c8a217209e23f9ca1a4b5b1971f60bcce7e4e73))
* **channels:** relax the outer ChannelManager lock to RwLock so channels don't cross-block ([da0ada2](https://github.com/FerroxLabs/wayland-core/commit/da0ada2220df6995bbb5720f666891b76131eebf))
* **compact:** prune reads superseded by later edits (token-opt) ([94835fc](https://github.com/FerroxLabs/wayland-core/commit/94835fcd3160a7293f4bdc8dfa29adfcb90cc999))
* **output:** fluff stop-sequences + static terseness directive (token-opt) ([cd12a80](https://github.com/FerroxLabs/wayland-core/commit/cd12a809c0171d50931ca7c420e6060da74b476e))
* **read-once:** backref repeated Grep/Glob/Bash outputs instead of re-sending (token-opt) ([611ff72](https://github.com/FerroxLabs/wayland-core/commit/611ff729700905aeba606aefe5ba289e69a3a768))
* **read:** diff-resend — answer external-change re-reads with a line diff (token-opt) ([c00b79a](https://github.com/FerroxLabs/wayland-core/commit/c00b79acd0074948d8e532c9242f80e3ed46a3aa))
* **token-spend:** wire routing tier, cheap+accurate compaction, bound retries, cache hygiene ([#65](https://github.com/FerroxLabs/wayland-core/issues/65)) ([2c70b7b](https://github.com/FerroxLabs/wayland-core/commit/2c70b7b828eb5f4defb4f60f29492d9c3fedf129))


### Code Refactoring

* **cli:** drop AnimClock dead last_tick field (rank 89) ([2e7128c](https://github.com/FerroxLabs/wayland-core/commit/2e7128c1743324e8bd0f48c3a008badc79de5124))
* **cli:** make IJFW declarative-install-only, drop compiled-in plugin ([4940fad](https://github.com/FerroxLabs/wayland-core/commit/4940fad8e6e1fccec68b174c3c8221fbde64e27c))
* **providers:** dedup reasoning-effort predicate to one source (R78) ([325df09](https://github.com/FerroxLabs/wayland-core/commit/325df09c6a84d73c4631743395d6f13183dffa2c))
* **providers:** delete dead in-tree Ollama provider ([082f841](https://github.com/FerroxLabs/wayland-core/commit/082f841010fadc2324982f7997371c8dc256b5b5))


### Documentation

* add concept diagrams (fleet spawn, fail-closed security, resilience, architecture) ([7785d05](https://github.com/FerroxLabs/wayland-core/commit/7785d053981f97a08032dc540e4dcffd6ec0da46))
* **advanced:** document native Bash output compaction (compact_bash) ([d7760ec](https://github.com/FerroxLabs/wayland-core/commit/d7760ec52dc9e390eb8c861035d6c93f49098217))
* **agent:** drop stale WorkflowRunner "validate_stub is a no-op" doc block (rank 87) ([9ce7814](https://github.com/FerroxLabs/wayland-core/commit/9ce78149c48fb6f9b4f245136a4a9c56f99441ee))
* **channels:** document ack modes, inbound webhook host, parity follow-ups ([343bbb7](https://github.com/FerroxLabs/wayland-core/commit/343bbb79d853e11a38e8d3d0ed9ad133044e343b))
* **channels:** document inbound access policy + tool posture ([7afca75](https://github.com/FerroxLabs/wayland-core/commit/7afca75d84877a9f2d52e24a78f1e734a650d853))
* **hooks:** note Windows literal-! behavior under cmd delayed expansion ([1e91bcf](https://github.com/FerroxLabs/wayland-core/commit/1e91bcf506d55f4d42bbdb7805b82f3ed5dd8abb))
* **providers:** document Sign in with ChatGPT ([90e0c62](https://github.com/FerroxLabs/wayland-core/commit/90e0c6216347e4da8ae068729e7dd1b7104d093c))
* public-readiness fixes for launch ([bd1b714](https://github.com/FerroxLabs/wayland-core/commit/bd1b714de233e82fb81c21991f16710767737a12))
* **readme:** add built-in tools comparison graphic ([35be0fb](https://github.com/FerroxLabs/wayland-core/commit/35be0fb788136b27d73340b21decd70a47690cab))
* rebuild README with branded comparison graphics, resilience trial, and real screenshots ([a06e0dc](https://github.com/FerroxLabs/wayland-core/commit/a06e0dc0e4ea71ca40e1f5999343b58c547a8e4f))
* refresh across the board for 0.12.x ([#46](https://github.com/FerroxLabs/wayland-core/issues/46)) ([273c764](https://github.com/FerroxLabs/wayland-core/commit/273c764af7a936b2dc8c73beaf82a310df55b7a2))
* replace AionUI host references with the Wayland desktop app ([3b71802](https://github.com/FerroxLabs/wayland-core/commit/3b71802d27a50965306603ef0610cb92590d6877))
* rework README around anchor features + Crucible headliner ([#96](https://github.com/FerroxLabs/wayland-core/issues/96)) ([8bf262d](https://github.com/FerroxLabs/wayland-core/commit/8bf262dfb9e894c1c93a1a301afa31e6305601bb))
* **sandbox:** correct stale comments that described shipped backends as unbuilt ([5b74c63](https://github.com/FerroxLabs/wayland-core/commit/5b74c63bd3e8475fdbb6ba525c37a83e45894245))
* **workflows:** document workflow_live_mode + ForgeFlows-Live observability ([7ce9562](https://github.com/FerroxLabs/wayland-core/commit/7ce95622d6c37d9a6aae7d9dc6272c3abd1a59ed))


### Miscellaneous Chores

* release 0.12.2 ([0323931](https://github.com/FerroxLabs/wayland-core/commit/03239313f4c02ec36f615cf5bcae7bf3b0590435))
* release 0.12.3 ([cd03533](https://github.com/FerroxLabs/wayland-core/commit/cd03533fb210d9cf7cb5727407bfbd211ff5a4b4))


### Build System

* **release:** prepare 0.12.1-rc.1 prerelease ([9c5922b](https://github.com/FerroxLabs/wayland-core/commit/9c5922b12b9fe35ba5636421619b756043a596ab))
* **release:** prepare 0.12.1-rc.2 prerelease ([93975b7](https://github.com/FerroxLabs/wayland-core/commit/93975b72dfa485896e336181dabb85d858d052a6))
* **release:** promote 0.12.1 stable ([d50bfbb](https://github.com/FerroxLabs/wayland-core/commit/d50bfbb1f19d173d4fb56350d8ae633d583e7686))

## [0.12.16](https://github.com/FerroxLabs/wayland-core/compare/v0.12.15...v0.12.16) (2026-06-29)


### Bug Fixes

* **bash:** fall back to cmd when PowerShell shell is selected under AppContainer ([#105](https://github.com/FerroxLabs/wayland-core/issues/105)) ([d698c66](https://github.com/FerroxLabs/wayland-core/commit/d698c663f0f361912ed25f532a83e519305c246a))
* **engine:** never let the reasoning budget starve the visible answer ([#426](https://github.com/FerroxLabs/wayland-core/issues/426)) ([#107](https://github.com/FerroxLabs/wayland-core/issues/107)) ([60f8e7d](https://github.com/FerroxLabs/wayland-core/commit/60f8e7d649a4a2fa684c4228b620a9ea8d0491fd))
* **providers:** replay reasoning_content for strict reasoners routed via a router ([#417](https://github.com/FerroxLabs/wayland-core/issues/417)) ([#108](https://github.com/FerroxLabs/wayland-core/issues/108)) ([fac4bde](https://github.com/FerroxLabs/wayland-core/commit/fac4bde7eecc2b3c31ec7c20927034432eea4bfa))
* **web:** bound readability extraction + reset breakers per turn + telemetry schema ([#403](https://github.com/FerroxLabs/wayland-core/issues/403)) ([#106](https://github.com/FerroxLabs/wayland-core/issues/106)) ([43c7aac](https://github.com/FerroxLabs/wayland-core/commit/43c7aac819e649c72383edd58be446d94856ace7))

## [0.12.15](https://github.com/FerroxLabs/wayland-core/compare/v0.12.14...v0.12.15) (2026-06-28)


### Bug Fixes

* **providers:** keyless self-hosted endpoints (no more "OpenAI API key is required" on local Ollama) ([#102](https://github.com/FerroxLabs/wayland-core/issues/102)) ([28d5eac](https://github.com/FerroxLabs/wayland-core/commit/28d5eac64851b9e404bd371f59768ee41890d9e9))

## [0.12.14](https://github.com/FerroxLabs/wayland-core/compare/v0.12.13...v0.12.14) (2026-06-28)

A focused Windows reliability release: it makes the sandboxed shell tool work end-to-end on Windows, fixing two AppContainer defects that left tool-use broken in the field.

### Highlights

- **Windows shell tools no longer hard-fail on machines without dev caches.** The AppContainer filesystem allowlist always includes optional developer caches (`~/.cache`, `~/.cargo`, `~/.npm`, `~/.rustup`). On any machine that doesn't have them — i.e. virtually every non-developer Windows box — applying the DACL grant aborted the *entire* command with `GetNamedSecurityInfoW … 0x2`, so every sandboxed shell command failed before it ran. Absent allowlist paths are now skipped, the grant succeeds, and commands execute normally. This is why the earlier AppContainer subprocess fixes ([#321](https://github.com/FerroxLabs/wayland-core/issues/321)–[#324](https://github.com/FerroxLabs/wayland-core/issues/324)) didn't translate into working shells in the field.
- **Sandboxed commands can no longer hang past their timeout.** `cmd.exe` spawns a console host (`conhost.exe`) that can outlive the command and keep the captured stdout/stderr pipes open; the output drain then blocked waiting for an EOF that never arrived — observed as a 120-second "command timed out" with no output on disconnected RDP sessions. The backend now reaps the entire job tree before draining, so output always flushes and the call returns a bounded result (or a clean, prompt timeout) instead of hanging. ([#100](https://github.com/FerroxLabs/wayland-core/issues/100))

## [0.12.13](https://github.com/FerroxLabs/wayland-core/compare/v0.12.12...v0.12.13) (2026-06-27)

A reliability-focused release: a new **capability-first tools gate** so models that can't do function calling degrade gracefully instead of failing the turn, a major Windows sandbox fix, and a round of audited provider- and config-layer hardening.

### Highlights

- **Tool-incapable models just work now — across local and cloud backends.** Point Wayland Core at a model that doesn't support function calling and the turn no longer dies on a raw provider error. Ollama models are detected up front via `/api/show` and have their `tools` array dropped before the request is even sent. Any backend that rejects tools with a `400` — llama.cpp started without `--jinja` (`tools param requires --jinja flag`), or an Ollama model that 400s with `does not support tools` — is caught, retried without tools, and **remembered**, so every later turn for that model skips tools pre-emptively. Tool-incapable Bedrock models (DeepSeek-R1 reasoning, Stability image, Titan/Cohere embedding) are name-gated the same way. Tool-*capable* models are unaffected — they keep their tools and call them exactly as before. ([#389](https://github.com/FerroxLabs/wayland-core/issues/389))
- **The Windows sandbox runs real subprocesses again.** The AppContainer backend no longer caps active processes too aggressively (`ActiveProcessLimit` raised to 512), resolves the launch shell correctly, and emits clearer diagnostics when a shell can't be found — so multi-step tool use works under the sandbox on Windows. ([#321](https://github.com/FerroxLabs/wayland-core/issues/321), [#322](https://github.com/FerroxLabs/wayland-core/issues/322), [#323](https://github.com/FerroxLabs/wayland-core/issues/323), [#324](https://github.com/FerroxLabs/wayland-core/issues/324))

### Provider reliability

- **Anthropic errors are classified correctly.** Non-credit Anthropic API errors are no longer misread as out-of-credit / billing failures, so genuine transient errors surface instead of a misleading "purchase credits" signal. ([#329](https://github.com/FerroxLabs/wayland-core/issues/329))
- **Flux reasoning summaries render as thinking.** A FluxRouter `reasoning_summary` is now decoded into a per-turn thinking subject, so reasoning summaries appear as proper thinking content. ([#318](https://github.com/FerroxLabs/wayland-core/issues/318))

### Configuration & hygiene

- **Config surface tightened.** `env_passthrough` is now wired through, unknown configuration keys produce a warning (via `serde_ignored`) instead of being silently dropped, and the sandbox configuration surface is exposed as a toggle. ([#325](https://github.com/FerroxLabs/wayland-core/issues/325), [#326](https://github.com/FerroxLabs/wayland-core/issues/326), [#327](https://github.com/FerroxLabs/wayland-core/issues/327))

## [0.12.12](https://github.com/FerroxLabs/wayland-core/compare/v0.12.11...v0.12.12) (2026-06-27)

### Crucible reliability & cost accuracy

This release hardens the Crucible (Mixture-of-Providers) council and the pricing engine behind it — every fix here was found by putting Crucible through a live, cross-vendor proof run and watching where it strained.

- **Bring-your-own pricing catalogs now load.** A custom `WAYLAND_PRICING_PATH` catalog parses reliably, so you can price any model the bundled catalog doesn't yet cover — and Crucible can certify a real spend ceiling against it.
- **Accurate Gemini pricing.** Gemini's live API slugs (e.g. `gemini-2.5-flash`) now resolve to the catalog correctly, so Gemini members are priced — and counted — when Crucible assembles a cost-diverse council.
- **Broader Opus support in councils.** Anthropic's Opus 4.x models, which decline an explicit sampling temperature, are now handled cleanly both as proposers and as the fusing judge.

Backed by new regression tests across `wcore-pricing` and `wcore-providers`.

## [0.12.11](https://github.com/FerroxLabs/wayland-core/compare/v0.12.10...v0.12.11) (2026-06-27)

This release is headlined by **Crucible**, our cross-provider Mixture-of-Providers council — wayland-core's answer to single-model ceilings — folded together with two audited reliability and security fixes.

### ✨ Headliner — Crucible (Mixture-of-Providers)

* **crucible:** a cross-provider council you run with `wayland-core crucible "<task>"`. N proposers, **each pinned to a different LLM provider**, work the task in parallel; a fenced, read-only **aggregator** fuses their answers into one. Three ways to run it: `--auto` gates convening behind a cheap difficulty classifier (trivial tasks get a single direct call, high-stakes tasks convene the full council); `--advisor` injects the fused synthesis into the normal trusted agent loop as private guidance (the agent then reasons, acts, and uses tools on it); `--terminal` prints the fused answer and stops. Includes per-tier proposer/aggregator temperatures, provenance-fenced injection containment, per-proposer **and** global soft deadlines with quorum, and `[crucible]` budget/daily-cap guards. Tri-model cross-audited; 151 dedicated tests. ([#91](https://github.com/FerroxLabs/wayland-core/pull/91))

### Enhancements

* **tools:** `image_generate` and `text_to_speech` now follow your active provider instead of assuming a single hardcoded host. FluxRouter and native OpenAI sessions route to the correct endpoint with the correct key (with proper `/v1` API-root resolution), gracefully fall back to FAL / Gemini Imagen / Hugging Face FLUX via their env keys, and **fail closed** on a base URL carrying embedded credentials. ([#310](https://github.com/FerroxLabs/wayland/issues/310))

### Security & Hardening

* **mcp:** MCP tool curation is now driven purely by **BM25 relevance + recency**. Removed a name-based "rescue" boost that a third-party MCP server could exploit by naming a tool like a built-in to jump the curation budget — closing a budget-hijack vector with no impact on built-in tools (which are never curated). ([#89](https://github.com/FerroxLabs/wayland/issues/89))

### Validation

* Full cross-platform gate green — **9,411 tests** across Linux, macOS, and Windows.

## [0.12.10](https://github.com/FerroxLabs/wayland-core/compare/v0.12.9...v0.12.10) (2026-06-27)


### Features

* **mcp:** provider-aware hard cap on total tool count + real MCP server provenance + BM25 relevance curation — caps the outbound tool array to the model's limit (OpenAI 128), fixing API-400 overflow with large MCP servers (Google Workspace, etc.); fixes uniquely-named MCP tools being misclassified as built-ins ([#86](https://github.com/FerroxLabs/wayland-core/issues/86), [#344](https://github.com/FerroxLabs/wayland-core/issues/344)/[#359](https://github.com/FerroxLabs/wayland-core/issues/359)) ([#87](https://github.com/FerroxLabs/wayland-core/issues/87))


### Bug Fixes

* **deps:** bump pdf-extract 0.12 → lopdf 0.42 ([RUSTSEC-2026-0187](https://rustsec.org/advisories/RUSTSEC-2026-0187)) ([#87](https://github.com/FerroxLabs/wayland-core/issues/87))
* **web-fetch:** wall-clock timeout message now contains "timed out" (de-flake) ([#87](https://github.com/FerroxLabs/wayland-core/issues/87))

## [0.12.9](https://github.com/FerroxLabs/wayland-core/compare/v0.12.8...v0.12.9) (2026-06-25)


### Bug Fixes

* OpenAI tool-name sanitization ([#297](https://github.com/FerroxLabs/wayland-core/issues/297)) + WSL canonicalize off-reactor ([#287](https://github.com/FerroxLabs/wayland-core/issues/287)) ([#84](https://github.com/FerroxLabs/wayland-core/issues/84)) ([af69bdc](https://github.com/FerroxLabs/wayland-core/commit/af69bdc046bef94671426a20a8a1fb7327c91d30))

## [0.12.8](https://github.com/FerroxLabs/wayland-core/compare/v0.12.7...v0.12.8) (2026-06-24)


### Features

* **providers:** add Sakana AI (Fugu) — OpenAI-compatible endpoint ([#82](https://github.com/FerroxLabs/wayland-core/issues/82)) ([a531f22](https://github.com/FerroxLabs/wayland-core/commit/a531f220d9ffbc089815b9dfb78478ff6affa4bd))

## [0.12.7](https://github.com/FerroxLabs/wayland-core/compare/v0.12.6...v0.12.7) (2026-06-23)


### Features

* **#255:** active-window kernel — context % vs the post-swap active model ([#74](https://github.com/FerroxLabs/wayland-core/issues/74)) ([7d22c84](https://github.com/FerroxLabs/wayland-core/commit/7d22c847718e48871bde90d666c906de350aecb8))
* **#279:** JSON-stream observability — active-window %, agent-run correlation, structured traces ([#76](https://github.com/FerroxLabs/wayland-core/issues/76)) ([3b9b070](https://github.com/FerroxLabs/wayland-core/commit/3b9b07006f399af3ccd9689166d028d94f2de003))
* **#280:** smart auto-compaction at active-window threshold (default-off, Flux-aware, memory handoff) ([#78](https://github.com/FerroxLabs/wayland-core/issues/78)) ([508d9e8](https://github.com/FerroxLabs/wayland-core/commit/508d9e8e790771f23f82b8577edecfd511624096))
* **#282:** Flux context-routing contract — client side V1 ([#77](https://github.com/FerroxLabs/wayland-core/issues/77)) ([508af81](https://github.com/FerroxLabs/wayland-core/commit/508af81c533b36e0cdedc0e48f55e6f695c70e1d))
* isolated profiles — CLI-isolation slice (Phase 0 + 1 + 3A + 2) ([#70](https://github.com/FerroxLabs/wayland-core/issues/70)) ([3177b17](https://github.com/FerroxLabs/wayland-core/commit/3177b1763d0334ba03057992d689904b9f810554))


### Bug Fixes

* **#282:** tolerate live Flux context-overflow shapes (found by live E2E) ([#79](https://github.com/FerroxLabs/wayland-core/issues/79)) ([c5aadd6](https://github.com/FerroxLabs/wayland-core/commit/c5aadd636505fb008f5dfa735ff9b09d2b0fe18c))
* **#285:** never emit orphaned tool_result during compaction (DeepSeek 400) ([#75](https://github.com/FerroxLabs/wayland-core/issues/75)) ([5f3aaf7](https://github.com/FerroxLabs/wayland-core/commit/5f3aaf78d01d9bab3fbf80766e97761f024eb4df))
* **#293:** authenticate openai-chatgpt from ~/.codex/auth.json ([#80](https://github.com/FerroxLabs/wayland-core/issues/80)) ([7f0c7cc](https://github.com/FerroxLabs/wayland-core/commit/7f0c7cc1559526f5a5814fd72a8a099500218699))
* OpenAI image default (gpt-image-1) + DeepSeek v4-flash 1M context ([#265](https://github.com/FerroxLabs/wayland-core/issues/265), [#255](https://github.com/FerroxLabs/wayland-core/issues/255)) ([#69](https://github.com/FerroxLabs/wayland-core/issues/69)) ([30dad57](https://github.com/FerroxLabs/wayland-core/commit/30dad572cb15b2ff3cdb0d7f2b936525d7e5ac06))
* **windows:** 4 Windows-only failures ([#257](https://github.com/FerroxLabs/wayland-core/issues/257) CRLF edit, [#262](https://github.com/FerroxLabs/wayland-core/issues/262)/[#263](https://github.com/FerroxLabs/wayland-core/issues/263) MCP stdio quoting, [#267](https://github.com/FerroxLabs/wayland-core/issues/267) sandbox \\?\ path) ([#72](https://github.com/FerroxLabs/wayland-core/issues/72)) ([d7ccbef](https://github.com/FerroxLabs/wayland-core/commit/d7ccbef78194fbbb7ad5ed7e87c7f0afb5370f0f))

## [0.12.6](https://github.com/FerroxLabs/wayland-core/compare/v0.12.5...v0.12.6) (2026-06-22)


### Features

* ChatGPT-sub model filtering ([#158](https://github.com/FerroxLabs/wayland-core/issues/158)) + MiniMax cost catalog ([#240](https://github.com/FerroxLabs/wayland-core/issues/240)) ([#68](https://github.com/FerroxLabs/wayland-core/issues/68)) ([f807397](https://github.com/FerroxLabs/wayland-core/commit/f807397dab29b9eea1fe18a9ef0f80e9ead3edfd))
* FluxRouter capabilities (image/fetch/web_search) + per-model max_tokens + reliability fixes ([#66](https://github.com/FerroxLabs/wayland-core/issues/66)) ([aefdd39](https://github.com/FerroxLabs/wayland-core/commit/aefdd3993c47c0a0ba6e6c7f16fbaf917cc325cd))


### Performance Improvements

* **token-spend:** wire routing tier, cheap+accurate compaction, bound retries, cache hygiene ([#65](https://github.com/FerroxLabs/wayland-core/issues/65)) ([2c70b7b](https://github.com/FerroxLabs/wayland-core/commit/2c70b7b828eb5f4defb4f60f29492d9c3fedf129))

## [0.12.5](https://github.com/FerroxLabs/wayland-core/compare/v0.12.4...v0.12.5) (2026-06-21)


### Features

* **sandbox:** WorkspacePolicy + OS secret-read-deny + Landlock Option A ([#59](https://github.com/FerroxLabs/wayland-core/issues/59)) ([dfa5aa2](https://github.com/FerroxLabs/wayland-core/commit/dfa5aa29c9d4f2a7cdf363f701339ed5147e37ad))


### Bug Fixes

* **#200:** unblock native Gemini egress + stop silent finish_reason=error turns ([#60](https://github.com/FerroxLabs/wayland-core/issues/60)) ([8d95578](https://github.com/FerroxLabs/wayland-core/commit/8d955782faf43d8c473606537337db0384ad0e9e))
* **agent,tools:** close two real Windows bugs (unbounded project-context walk + glob sandbox bypass) ([#64](https://github.com/FerroxLabs/wayland-core/issues/64)) ([fea2c52](https://github.com/FerroxLabs/wayland-core/commit/fea2c52f6069f1e32f1bfbcb7640818a7820b397))
* **cli:** surface a clear, Ollama-aware reason on init failure instead of bare exit 1 ([#186](https://github.com/FerroxLabs/wayland-core/issues/186)) ([#61](https://github.com/FerroxLabs/wayland-core/issues/61)) ([b37b3d1](https://github.com/FerroxLabs/wayland-core/commit/b37b3d12663fdf45b472933bf5eb12f0164fc8db))
* **shell:** accept .exe and absolute-path Windows shell selectors ([#197](https://github.com/FerroxLabs/wayland-core/issues/197)) ([#62](https://github.com/FerroxLabs/wayland-core/issues/62)) ([9b332e7](https://github.com/FerroxLabs/wayland-core/commit/9b332e7eedc9bf4ec9141dbbdceaff6b01a3873b))

## [0.12.4](https://github.com/FerroxLabs/wayland-core/compare/v0.12.3...v0.12.4) (2026-06-20)


### Bug Fixes

* **skills:** hide unreviewed auto-drafted skills from the model catalog ([#56](https://github.com/FerroxLabs/wayland-core/issues/56)) ([a2c0de4](https://github.com/FerroxLabs/wayland-core/commit/a2c0de415e8ce51ee8f0232b8590119276d6e152))
* **skills:** keep the hello test fixture out of the shipped catalog ([#55](https://github.com/FerroxLabs/wayland-core/issues/55)) ([35d334f](https://github.com/FerroxLabs/wayland-core/commit/35d334f7f10b7ca215fb1c674fbb7c64e654f507))

## [0.12.3](https://github.com/FerroxLabs/wayland-core/compare/v0.12.2...v0.12.3) (2026-06-19)


### Features

* **tools:** PowerShell shell for the Bash tool on Windows — selectable via the `WAYLAND_BASH_SHELL` env var or the `[tools] windows_shell` config key (`powershell`/`pwsh`); precedence env > config > default `cmd`, scoped to the Bash tool ([#45](https://github.com/FerroxLabs/wayland-core/issues/45)) ([130dc3d](https://github.com/FerroxLabs/wayland-core/commit/130dc3da1d4720ac407423125f058aacb6c2390d))


### Bug Fixes

* **egress:** allowlist NVIDIA NIM, Cerebras, MiniMax-failover & Qwen hosts ([#48](https://github.com/FerroxLabs/wayland-core/issues/48)) ([a68f2d9](https://github.com/FerroxLabs/wayland-core/commit/a68f2d917f8c950004a9d92ba57cce9d759cbe4d))
* **oauth:** stop advertising a non-existent `wayland auth login grok` command ([#47](https://github.com/FerroxLabs/wayland-core/issues/47)) ([42e16ec](https://github.com/FerroxLabs/wayland-core/commit/42e16ec5009883a1cff42478f2d347ac4fee7a13))
* **providers:** strip empty/missing tool_call_id before sending (DeepSeek 400 guard) ([#50](https://github.com/FerroxLabs/wayland-core/issues/50)) ([c97424d](https://github.com/FerroxLabs/wayland-core/commit/c97424d463f5e976c1e2863db65cebaf74b0a6a7))


### Documentation

* refresh across the board for 0.12.x ([#46](https://github.com/FerroxLabs/wayland-core/issues/46)) ([273c764](https://github.com/FerroxLabs/wayland-core/commit/273c764af7a936b2dc8c73beaf82a310df55b7a2))


### Miscellaneous Chores

* release 0.12.3 ([cd03533](https://github.com/FerroxLabs/wayland-core/commit/cd03533fb210d9cf7cb5727407bfbd211ff5a4b4))

## [0.12.2](https://github.com/FerroxLabs/wayland-core/compare/v0.12.1...v0.12.2) (2026-06-18)


### Bug Fixes

* **providers:** provider auth robustness — Grok OAuth, region failover, auth errors ([#42](https://github.com/FerroxLabs/wayland-core/issues/42)) ([4dfc566](https://github.com/FerroxLabs/wayland-core/commit/4dfc566af50b6a233f4543e837f84efa5ee8490a))


### Miscellaneous Chores

* release 0.12.2 ([0323931](https://github.com/FerroxLabs/wayland-core/commit/03239313f4c02ec36f615cf5bcae7bf3b0590435))

## [0.12.1](https://github.com/FerroxLabs/wayland-core/compare/v0.12.0...v0.12.1) (2026-06-18)

Stable release rolling up everything from the `0.12.1-rc.1` and `0.12.1-rc.2`
prereleases (full per-commit detail in the sections below).

### Highlights

* **Sign in with ChatGPT** — OpenAI Codex OAuth provider with rotating-refresh token manager, device-code login for headless/remote, and token import from the Codex CLI.
* **MiniMax provider** — via the Anthropic-compatible endpoint, visible in the provider/model pickers.
* **Forge zero-config MCP discovery** — one-command `/mcp connect` to a trusted loopback MCP server, scoped-token grant with `${cred:KEY}` headers (token never lands in `config.toml`), opt-in `allow_local`, and a selectable DISCOVERED row in `/doctor`.
* **Config cockpit** — paste-to-connect with live key fingerprinting + a validation ladder, an Essentials/Advanced settings surface, collection editors (tools/egress/failover), config-posture health and self-configure discovery in `/doctor`, a redacted `/effective` config preview, and channel-integration visibility.
* **Live model discovery** — Bedrock (`ListFoundationModels`), Gemini, and a connected-provider catalog refresh, backed by a per-provider 24h disk cache.
* **TUI** — arrow-key cross-provider `/model` and `/provider` pickers, the command palette on `/` from any surface, connection-aware provider listing.
* **Security & stability** — a 42-defect deep-sweep remediation: closed a Forge-MCP token-exfil SSRF, a Glob sandbox bypass, unbounded reads across MCP/Matrix/ACP, a provider key-pool poison DoS, skill-arg shell injection, and MCP header secret leaks; credentials now default to keyring with plaintext fallback (F16).
* **Core fixes** — Windows MCP stdio launch (#164) and the Anthropic unrecoverable-conversation `thinking.signature` 400 (#161); Flux Router reachable out of the box under the egress guard.

### Build System

* **release:** promote 0.12.1 stable ([d50bfbb](https://github.com/FerroxLabs/wayland-core/commit/d50bfbb1f19d173d4fb56350d8ae633d583e7686))

## [0.12.1-rc.2](https://github.com/FerroxLabs/wayland-core/compare/v0.12.1-rc.1...v0.12.1-rc.2) (2026-06-18)


### Features

* **providers:** add MiniMax provider via Anthropic-compatible endpoint ([703ba14](https://github.com/FerroxLabs/wayland-core/commit/703ba14ce25f5b23a19a06cea00aebdb16631bc4))


### Bug Fixes

* **audit:** 19 low/medium defects — browser, sandbox, channels, tools, TUI ([8c589ad](https://github.com/FerroxLabs/wayland-core/commit/8c589ad36be0e4e8605ca1e49c770a52ce6f3385))
* **audit:** 7 high-severity defects — sandbox, provider protocol, unbounded reads ([8273b2a](https://github.com/FerroxLabs/wayland-core/commit/8273b2ac1e56937e816101c45415954a6d4ea6b6))
* **audit:** provider resilience + egress/secret hygiene (8 fixes) ([0e893d9](https://github.com/FerroxLabs/wayland-core/commit/0e893d99f38b623a4deaa65ea27d3c51c424c8eb))
* **config:** default credentials to keyring with plaintext fallback (F16) ([6c57160](https://github.com/FerroxLabs/wayland-core/commit/6c5716080da4429f32a0ccfc9acd0399cfe6bd3f))
* **core:** Windows MCP stdio launch ([#164](https://github.com/FerroxLabs/wayland-core/issues/164)) + Anthropic unrecoverable-conversation ([#161](https://github.com/FerroxLabs/wayland-core/issues/161)) ([38b85e6](https://github.com/FerroxLabs/wayland-core/commit/38b85e6fb6895100e24218366586b08da6dd62d4))
* **egress:** allowlist Flux Router out of the box + accept full-host entries ([1fa6407](https://github.com/FerroxLabs/wayland-core/commit/1fa6407e907227e7c09b7431e968dbd3920e95d0))
* **forge-mcp:** close token-exfil SSRF + 4 reliability defects in discovery flow ([bd2f40d](https://github.com/FerroxLabs/wayland-core/commit/bd2f40d23aa98d64aff2406f5e7d6b8b45a304ba))
* **mcp:** don't caret-escape the program name in Windows stdio launch ([371f619](https://github.com/FerroxLabs/wayland-core/commit/371f619ee47f1c9beb8d4b984c6f8acc979ce132))
* **providers:** drop unsigned thinking blocks when building Anthropic messages ([cdd0968](https://github.com/FerroxLabs/wayland-core/commit/cdd0968dc66acf53471748ebdd40c460b2630b3c))
* **providers:** make MiniMax visible in pickers + bound tool-input accumulator ([e8ac0f2](https://github.com/FerroxLabs/wayland-core/commit/e8ac0f29642e75a97143ec73d9172cb185f5eb1a))


### Build System

* **release:** prepare 0.12.1-rc.2 prerelease ([93975b7](https://github.com/FerroxLabs/wayland-core/commit/93975b72dfa485896e336181dabb85d858d052a6))

## [0.12.1-rc.1](https://github.com/FerroxLabs/wayland-core/compare/v0.12.0...v0.12.1-rc.1) (2026-06-17)


### Features

* **agent:** allow chatgpt.com egress when the chatgpt provider is active ([b3372ac](https://github.com/FerroxLabs/wayland-core/commit/b3372ac8af6b639934b293e0915e21d0c604aebb))
* **agent:** wire openai-chatgpt provider with oauth bearer source ([18a50d6](https://github.com/FerroxLabs/wayland-core/commit/18a50d626b45f8bc78ef729f6836732193f9a971))
* **channels,tui:** surface channel integrations in /doctor + fix F-019 (S10 v1) ([6958c1c](https://github.com/FerroxLabs/wayland-core/commit/6958c1cfbb11e648166af0571c3b42772339584f))
* **cli:** wayland auth login/logout/status for chatgpt ([060dc45](https://github.com/FerroxLabs/wayland-core/commit/060dc4533e6df3781a0fefb8021c31500fa5ecd8))
* **config,tui:** redacted effective-config preview (S9 v1) ([ff30d20](https://github.com/FerroxLabs/wayland-core/commit/ff30d2051303c85cf1019951b59cfccc7cc8287b))
* **config:** chatgpt_defaults compat preset ([8fac871](https://github.com/FerroxLabs/wayland-core/commit/8fac87162af5dd40c9f26c0a7b2196d1590aca55))
* **config:** config cockpit — paste-to-connect, editors, /doctor health, /effective, channels, discovery ([8fe5559](https://github.com/FerroxLabs/wayland-core/commit/8fe5559f04131ea02a0ffba23402f5a36a76f6df))
* **config:** connected_providers credential helper ([4cffba9](https://github.com/FerroxLabs/wayland-core/commit/4cffba9030a56ad6d7c4fdedf08bf80a5060414c))
* **config:** openai-chatgpt provider type + parsing ([5709f87](https://github.com/FerroxLabs/wayland-core/commit/5709f87ae5de3e1633b4f6cf6141e9213a70627d))
* **config:** read the Forge local-MCP discovery file (Slice 3) ([1014e21](https://github.com/FerroxLabs/wayland-core/commit/1014e212eab7bf472f4ac38c02fe9939c2116cc4))
* **mcp:** /mcp connect — one-command zero-config Forge MCP connect (Slice 3, Piece 3) ([17973e6](https://github.com/FerroxLabs/wayland-core/commit/17973e6bbae98189aeefacd4bdc798e55bbf8b3a))
* **mcp:** DISCOVERED row-to-connect + boot-hero Forge line (Slice 3b polish) ([509fd69](https://github.com/FerroxLabs/wayland-core/commit/509fd69a9d3e14ca5211cfbe04b4d559f7c92db8))
* **mcp:** Forge connect flow — ${cred:KEY} headers + live token grant (Slice 3) ([3f66b9f](https://github.com/FerroxLabs/wayland-core/commit/3f66b9f0457bf11c5f66fd9519c016639c6a8952))
* **mcp:** Forge connect polish — selectable DISCOVERED row + boot-hero line (Slice 3b) ([d19af5b](https://github.com/FerroxLabs/wayland-core/commit/d19af5bf85dc1271dd736a53f7e5f8b3701c1289))
* **mcp:** Forge loopback grant client — liveness probe + scoped token (Slice 3) ([df9d1c9](https://github.com/FerroxLabs/wayland-core/commit/df9d1c9ba8bc4e8f08fb1028cbc0dcd7a246e84a))
* **mcp:** Forge zero-config local-MCP discovery — keystone + reader + grant client + connect flow (Slice 3, headless) ([106b869](https://github.com/FerroxLabs/wayland-core/commit/106b8696412d04ca6f53ded3baab453b5de21f66))
* **mcp:** opt-in allow_local to connect trusted loopback MCP servers ([68b0a6b](https://github.com/FerroxLabs/wayland-core/commit/68b0a6ba4902aea9fcfc578e655fa92ebda38939))
* **oauth:** add ChatGPT device-code login (headless/remote path) ([2a6a4e6](https://github.com/FerroxLabs/wayland-core/commit/2a6a4e69118b1af2d3f06dc98d5613f6608f4fee))
* **oauth:** chatgpt token manager with rotating refresh, JWT account-id decode, and flow descriptor ([9a1b5c1](https://github.com/FerroxLabs/wayland-core/commit/9a1b5c156061515b12bab85da2cba5ecedb4b6e1))
* **oauth:** extra authorize params, configurable redirect host/path with dual-stack loopback bind, id_token capture ([765c11a](https://github.com/FerroxLabs/wayland-core/commit/765c11adb9137c28541dda88529a13fdd596dc28))
* **oauth:** import chatgpt tokens from codex cli ([630688d](https://github.com/FerroxLabs/wayland-core/commit/630688d051a0e6302829efa5edb2821847efefd8))
* **providers:** add key fingerprinting for paste-to-detect config ([e71d8ca](https://github.com/FerroxLabs/wayland-core/commit/e71d8ca1d63a98c0c5890481eae9f7a00053686b))
* **providers:** add live key-validation ladder for paste-to-detect ([c576df9](https://github.com/FerroxLabs/wayland-core/commit/c576df9d6104ec3fc53fb57bfe8fb035d16fa82d))
* **providers:** live Bedrock model discovery via ListFoundationModels ([27a25dc](https://github.com/FerroxLabs/wayland-core/commit/27a25dcb0e533eaab1a67ca6bc79224a626b7ff6))
* **providers:** live Gemini model discovery ([ed2126e](https://github.com/FerroxLabs/wayland-core/commit/ed2126e6410fa39f26c575e86308dca5c1119f98))
* **providers:** make runtime provider construction OAuth-aware for openai-chatgpt ([3e067c1](https://github.com/FerroxLabs/wayland-core/commit/3e067c1a414a37a9d4df70c3d44ecb7ca176e257))
* **providers:** ModelCatalog.refresh_connected live discovery service ([0bc02bc](https://github.com/FerroxLabs/wayland-core/commit/0bc02bce82c4c1529f36fcd50138050226b9c237))
* **providers:** openai-chatgpt provider over async oauth bearer source ([c19a795](https://github.com/FerroxLabs/wayland-core/commit/c19a795fde0dfa833e6463f7df66d3816fd465d6))
* **providers:** orchestrate paste-to-detect (fingerprint + validate) ([804373e](https://github.com/FerroxLabs/wayland-core/commit/804373ef44a94af336bc1f3ebca8174cc871f14e))
* **providers:** per-provider model-list disk cache (24h TTL) ([785704e](https://github.com/FerroxLabs/wayland-core/commit/785704ec5d8dbf3d854712187ca7d3ec7975ec5e))
* Sign in with ChatGPT (OpenAI Codex OAuth) ([5ccc0fc](https://github.com/FerroxLabs/wayland-core/commit/5ccc0fcc48ecf1ccc7203277375c853069cf08c8))
* **tui:** /model picker reads live cached models + refreshes on open ([f94e2c0](https://github.com/FerroxLabs/wayland-core/commit/f94e2c02561b6b9812b56ff3faede7547394d9f6))
* **tui:** Advanced config tier — observability/storage/security editors (S6) ([94dc918](https://github.com/FerroxLabs/wayland-core/commit/94dc9182c22de94cf9bfe589f9ccce5dec2cc447))
* **tui:** arrow-key /model and /provider pickers (cross-provider) ([4b46606](https://github.com/FerroxLabs/wayland-core/commit/4b466061e4073a5a8443948cb512086998ff844a))
* **tui:** boot-screen provider discovery + Tab always switches tabs (FIX-5, FIX-7) ([b7f03d9](https://github.com/FerroxLabs/wayland-core/commit/b7f03d906b011f0cc12cf2118a6abe109c18fac8))
* **tui:** collection list editors — tools/egress/failover (S7) ([299cdb7](https://github.com/FerroxLabs/wayland-core/commit/299cdb7432eddcf4162115bcd859f60473a8f0e1))
* **tui:** config-posture health section in /doctor (S8) ([4f1cb34](https://github.com/FerroxLabs/wayland-core/commit/4f1cb345fb4ab0b74710d823ab09a24620caf07d))
* **tui:** Essentials config home — Tools + Wallet rows, posture + health/cost (S5) ([fbaa431](https://github.com/FerroxLabs/wayland-core/commit/fbaa431d31beed947aad16869b511480323bf127))
* **tui:** make /provider picker connection-aware ([130bc72](https://github.com/FerroxLabs/wayland-core/commit/130bc7288d8c9522bae46b34a16a1ed98a18ca9e))
* **tui:** open the command palette with / from any surface ([2f21d06](https://github.com/FerroxLabs/wayland-core/commit/2f21d0688a71e0e956bc3d108a9bf6a9ef4f6fad))
* **tui:** paste-to-connect door in the Config Providers tier (FIX-3) ([e16f293](https://github.com/FerroxLabs/wayland-core/commit/e16f293abb407d7dac1d8a21a62159c9dd14d22f))
* **tui:** paste-to-detect modal state machine + view-model (S4a) ([6cb6e25](https://github.com/FerroxLabs/wayland-core/commit/6cb6e250425ee521177f88aeb3ad695bed628187))
* **tui:** self-configure discovery section in /doctor (S11 v1) ([f01c9f9](https://github.com/FerroxLabs/wayland-core/commit/f01c9f940b1f8448bc054f10475df98e3feeda94))
* **tui:** wire the paste-to-detect /connect overlay (S4b) ([7b75549](https://github.com/FerroxLabs/wayland-core/commit/7b75549b8c2120c247dc6940cd5a840af5a01dd1))
* **types:** codex model aliases for openai-chatgpt ([daa6210](https://github.com/FerroxLabs/wayland-core/commit/daa6210a5ded3e1d95015ab1a0c195cbc9d18cca))


### Bug Fixes

* **model-catalog:** tag a floored model fetch BuiltIn, not a live "synced" ([0bca1a7](https://github.com/FerroxLabs/wayland-core/commit/0bca1a7545c8a5e4d8e7fa155e63f1e694d3014c))
* **model-picker:** load UI-saved provider keys + connection-aware live /model picker ([3a8929f](https://github.com/FerroxLabs/wayland-core/commit/3a8929fd45e9c5ef26ddabe79cf1904d570fd931))
* **providers:** accept codex response.done/incomplete as terminal frames ([0bc0ed6](https://github.com/FerroxLabs/wayland-core/commit/0bc0ed62a96ef8048c67e8a56e962a1ed8f93cff))
* **providers:** Bedrock/Vertex "connected" only with real ambient credentials ([7245065](https://github.com/FerroxLabs/wayland-core/commit/72450658c87fb78c642a91b54ce041f5dcf7cc1d))
* **providers:** don't request encrypted reasoning until we round-trip it ([52eeceb](https://github.com/FerroxLabs/wayland-core/commit/52eecebb3ae3ea70caa4d074a1b4cc68b9890ef4))
* **providers:** drop unused json import; lock socket2/base64 direct edges ([fd9100e](https://github.com/FerroxLabs/wayland-core/commit/fd9100ec250b2cc674887ed47d2cb48e437f5ff6))
* **providers:** forward list_models on OpenAI-compat newtypes (paste-connect) ([efbddba](https://github.com/FerroxLabs/wayland-core/commit/efbddba218df0f854f914a7ee77ff9e4b2fd324d))
* **providers:** ResilientProvider delegates alias_key/list_models to primary ([4c409c1](https://github.com/FerroxLabs/wayland-core/commit/4c409c1da6e5506c615a9279cbd092f41bcb56fe))
* **tui:** Config Esc saves pending toggles instead of reverting ([854f065](https://github.com/FerroxLabs/wayland-core/commit/854f0657843aee2ce2b4af0e0029adfedec45d62))
* **tui:** show em-dash for unrecorded spend in the status bar ([f8e5d65](https://github.com/FerroxLabs/wayland-core/commit/f8e5d6540a370d3a3398161c2e15437da3127f85))
* **tui:** stop /doctor from freezing the whole TUI on live probes ([4121652](https://github.com/FerroxLabs/wayland-core/commit/4121652ebd66cae28084d67d3d64ea6107da020c))
* **tui:** widen Advanced label pad so the value isn't glued to it ([1cb6578](https://github.com/FerroxLabs/wayland-core/commit/1cb65780e38e374606454eea865d520b20798087))


### Documentation

* **providers:** document Sign in with ChatGPT ([90e0c62](https://github.com/FerroxLabs/wayland-core/commit/90e0c6216347e4da8ae068729e7dd1b7104d093c))


### Build System

* **release:** prepare 0.12.1-rc.1 prerelease ([9c5922b](https://github.com/FerroxLabs/wayland-core/commit/9c5922b12b9fe35ba5636421619b756043a596ab))

## [0.11.0-rc.1] - 2026-06-11

Release candidate for 0.11.0. The headline is **inbound channels** — Wayland Core now receives, not just sends — plus native per-command Bash output compaction, a JWT crypto-backend security fix, and a batch of provider and platform fixes. Still a public beta; cut as an RC to soak the new network-facing channel surface before the final 0.11.0.

### Highlights

* **Inbound channels.** Two-way messaging across Telegram, Discord, Slack, WhatsApp, Matrix, Microsoft Teams, and SMS: inbound receive (long-poll / `/sync` / webhook host), an engine-backed turn dispatcher with a tool-posture scope for channel-originated agents, reconnect supervision so channels survive disconnects, Microsoft Teams Bot Framework JWT validation, outbound chunking with per-platform size caps, an idempotency nonce to dedupe retried sends, and react/typing with ack reactions + a typing keepalive state machine.
* **Auth-aware inbound media.** Images and audio attachments are fetched and described/transcribed before the turn, with credentials kept inside each connector boundary.
* **Native Bash output compaction.** Verbose `cargo` / `git` / test-runner / `grep` output is compacted into the model's transcript (the human still sees full output) — block-aware, fail-open, size-gated, default-on via `ProviderCompat::compact_bash`, with per-call savings telemetry.
* **Security.** Migrated the JWT crypto backend to `aws_lc_rs`, dropping `rsa` and eliminating RUSTSEC-2023-0071 (Marvin Attack) at the source. Closed a Grep RCE, skill/rules prompt-injection, and hook shell-execution hardening; capped stdin line length (newline-less OOM DoS); fail-closed on UTF-8 split-codepoint corruption.

### Providers

* gpt-5 family now routes to the OpenAI Responses API (`/v1/responses`).
* Gemini 2.5-class: split SSE frames on CRLF (stops false truncation); inject default items for array schemas (stops tool-registration 400s).
* Default moonshot/qwen to their international endpoints; pin `api_path` so 8 native providers stop 404ing.

### Fixes

* ALSA is no longer a hard dependency — `cpal` is gated behind an off-by-default `voice` feature, so the default binary runs on minimal Linux without `libasound` (#14).
* The `/config` providers pane now scrolls to keep the focused row visible on short terminals (#16).
* PATHEXT-aware `npx` detection on Windows so the IJFW MCP server registers (#6).
* Legacy-YAML migration no longer clobbers an existing `config.toml`.

### Extensibility

* Declarative on-disk plugins under the profile home, wiring hooks + MCP into the engine.

## [0.10.0] - 2026-06-08

First public release. Wayland Core is a domain-agnostic autonomous-agent engine written in Rust: terminal-first, multi-provider, MCP-native, and embeddable. It ships as a **public beta**, capable and open, and still hardening under a continuous endurance soak (see "Built to endure" in the README).

### Highlights

* **Multi-provider.** 7 native provider integrations (Anthropic, OpenAI, Google Gemini, Google Vertex AI, AWS Bedrock with SigV4, Cohere, Azure OpenAI) plus a 104-entry models.dev catalog, all behind one provider-neutral engine and a declarative ProviderCompat layer. Circuit-breaker resilience, mid-stream reconnect, and multi-key rotation across every API-key provider.
* **Orchestration.** Sub-agents, a git-worktree-isolated parallel swarm with a dirty-tree guard, declarative ForgeFlows workflows that lower onto the engine's own execution graph, and selectable reducers via `wayland swarm --reduce mesh|fleet|consensus|debate`.
* **Security by default.** A fail-closed OS-native sandbox (bubblewrap, sandbox-exec, AppContainer), a CI-enforced egress chokepoint with an exfil-shape classifier, an always-on SSRF and metadata floor, and argv-safe shell execution.
* **Extensibility.** MCP in both directions (a client, and a server that advertises and executes its own built-in tools, with runtime injection), roughly 70 built-in tools, skills, blocking lifecycle hooks, and a plugin API.
* **Embeddable.** A typed JSON-Lines protocol drives the engine headlessly behind a host app.
* **Self-evolution (GEPA).** A scored optimizer that evolves prompts and skills against your own reference cases.

### Surfaces

One binary, three ways to run it: a one-shot command, an interactive TUI, or a headless JSON stream.

### Notes

This is a public beta. APIs and behavior may change before 1.0. A continuous, fault-injected endurance trial is ongoing; the method, measurements, and honesty bounds are documented in [docs/resilience.md](docs/resilience.md).
