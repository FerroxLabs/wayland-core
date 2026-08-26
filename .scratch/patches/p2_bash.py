import io

p = "crates/wcore-tools/src/bash.rs"
s = io.open(p, encoding="utf-8").read()

old_mod = "mod policy;\n"
assert s.count(old_mod) == 1
s = s.replace(old_mod, "mod policy;\nmod read_only;\n", 1)

old = """    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        // Shell commands can mutate arbitrary host state with no general reconciler.
        ToolEffectContract::default()
    }
"""
assert s.count(old) == 1, s.count(old)

new = '''    /// Opaque unless [`read_only::is_provably_read_only`] proves otherwise.
    ///
    /// A shell command can mutate arbitrary host state and nothing can
    /// photograph the result afterwards, so opaque remains the answer for
    /// anything this classifier does not model — which is most of what a
    /// shell can express. The exception is narrow and static: one simple
    /// command, no shell metacharacters at all, and a program with neither a
    /// write mode nor a route to a user-configured helper. Such a call cannot
    /// have changed anything, so an interruption leaves nothing for an
    /// operator to have an opinion about.
    fn effect_contract(&self, input: &Value) -> ToolEffectContract {
        match input.get("command").and_then(Value::as_str) {
            Some(command) if read_only::is_provably_read_only(command) => {
                wcore_types::tool::repeat_safe_contract(
                    wcore_types::tool::READ_ONLY_SHELL_RECONCILER,
                )
            }
            _ => ToolEffectContract::default(),
        }
    }
'''
s = s.replace(old, new, 1)
io.open(p, "w", encoding="utf-8").write(s)
print("p2 ok")
