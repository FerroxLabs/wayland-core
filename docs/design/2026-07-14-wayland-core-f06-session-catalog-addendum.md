# Wayland Core F06 Session-Catalog Addendum

**Status:** implemented; targeted Linux remote proof complete; native Windows proof pending

**Parent contracts:**

- `2026-07-13-wayland-core-frontier-build-plan.md`, F06
- `2026-07-13-wayland-core-f06-containment-contract.md`

## 1. Conflict and decision

The emergency F06 contract deliberately left the general bundled registry API
unchanged. The master F06 plan separately requires preventing cross-session
catalog pollution until F09 supplies the broader runtime-scoped state model.
The released process-global registry cannot satisfy both requirements: a skill
registered for one bootstrap is observable by another bootstrap in the same
process.

This addendum owns that follow-up only. It does not rewrite the completed
emergency-containment receipt. It replaces the process-global bundled registry
with a catalog owned by one bootstrap/session.

Security and truthful behavior take precedence over source compatibility for
the internal Rust API. A deprecated global wrapper, process-global template,
thread-local registry, or silent no-op would preserve either the defect or a
misleading API and is rejected.

## 2. Runtime contract

1. Every bootstrap creates a fresh `BundledSkillCatalog`.
2. Embedded definitions are copied into that catalog first.
3. Plugin definitions are appended to that catalog in discovery order.
4. Loader entry points used by bootstrap receive the catalog explicitly.
5. No registration or load operation mutates process-global skill state.
6. An A -> B -> A sequence in one process returns A, then B, then A without
   union, stale entries, or cross-session reference files.
7. Two catalogs containing the same skill name and relative reference-file path
   use distinct extraction roots and retain their own bytes.
8. The legacy loader entry points remain fixture-free and do not manufacture a
   production `hello` skill.
9. Existing disk, MCP, project, user, and plugin precedence remains unchanged
   except that plugin-bundled entries are visible only to their owning session.

## 3. Rust API migration

This is an intentional source-breaking change in the pre-1.0 `wcore-skills`
crate:

| Released API | Session-scoped replacement |
|---|---|
| `register_bundled_skill(def)` | `catalog.register(def.into())` or `register_bundled_skill(&mut catalog, def)` |
| `get_bundled_skills()` | `catalog.get_bundled_skills()` |
| `prepare_bundled_skills().await` | `catalog.prepare_bundled_skills().await` |
| `init_bundled_skills()` for global mutation | `init_bundled_skills()` returning a fresh catalog |
| `load_catalog(...)` when bundled entries are required | `load_catalog_with_bundled(..., &catalog)` |

Callers that do not contribute bundled skills may continue using the legacy
loader functions; those functions use an empty embedded catalog. Plugin authors
continue to target `wcore-plugin-api`; the host performs this migration and the
wire/plugin manifest contract does not change.

The release notes for the first version containing this change must name this
Rust API migration. Reintroducing global state solely to avoid a compile-time
migration is prohibited.

## 4. Reference-file isolation

The existing private per-process extraction root remains the shutdown cleanup
unit. Each catalog receives a unique in-process namespace beneath that root,
and each skill extracts beneath its catalog namespace. Namespace uniqueness
must not depend on a skill name or caller-provided bytes.

Extracted files remain available for the lifetime expected by returned
`SkillMetadata`. Graceful process shutdown removes the per-process root through
the existing cleanup path. Relative-path validation, owner-only directories,
exclusive creation, no-follow behavior, and Windows ACL hardening remain
mandatory.

On Windows, the exclusively-created random process leaf receives its protected
`TokenUser` owner/DACL atomically through `CreateDirectoryW` security attributes,
then is immediately opened no-follow and without `FILE_SHARE_DELETE`. The
process retains that pinned handle and performs catalog, skill,
nested-directory, and file operations relative to it. Every descendant is
created with `NtCreateFile` against the retained parent handle and receives the
final `TokenUser` owner plus protected owner-only DACL in the same create call.
Existing directory components are reopened with `FollowSymlinks::No`, checked
for directory and reparse-point attributes, and retained before they become the
base for another operation. The current process `TokenUser` SID is read from the
process token, and the protected owner-only policy is reapplied through
`SetSecurityInfo` to each exact retained handle. No account-name environment
variable, executable lookup, or ambient pathname participates in creation or
post-creation ACL hardening.
Hardening is fail-closed before file bytes are written. The CLI cleanup guard is
declared before bootstrap/session state; signal shutdown first drops that state,
then cleanup releases the pinned root handle and removes the exact process root.

## 5. Required proof

- Session A plugin sentinel is absent from B and returns unchanged in A2.
- The same skill name and `guide.md` path with different A/B bytes produces
  distinct roots and exact A/B/A contents.
- Parallel catalogs cannot observe one another's entries or files.
- Bare and supplemental loaders remain fixture-free.
- Plugin bootstrap wiring and skill-delivery tests pass.
- `wcore-skills` and focused `wcore-agent` tests pass on Hetzner.
- Workspace clippy with warnings denied passes.
- The exact integrated commit passes the full workspace gate before M0 closes.

## 6. Boundaries

This addendum does not redesign draft governance, PromptStore, MCP discovery,
disk-skill precedence, or the F23 promotion transaction. F09 still owns the
remaining process-global policy and runtime state. No Desktop/Core protocol
schema changes are introduced here.

## 7. Verification record

The committed seal passed these Hetzner gates through `remote-cargo.sh`:

- `cargo check -p wcore-skills --tests`
- `cargo test -p wcore-skills`
- `cargo test -p wcore-agent --test plugin_bootstrap_wiring`
- `cargo test -p wcore-agent --test memory_context_integration --test skills_e2e --test tool_guidance_prompt_test`
- `cargo test -p wcore-cli native_shutdown_signals_remove_exact_bundled_root -- --nocapture`

The cross-critique command ran with reduced lineage because one auditor emitted
no parseable findings; the contributing auditor's findings were generic
project-level objections unrelated to this F06 diff. The remote host has the
`x86_64-pc-windows-gnu` Rust standard-library target, but both `--tests` and
`--lib` cross-check attempts stopped in third-party `ring`/SQLite build scripts
before F06 code because the host has no `x86_64-w64-mingw32-gcc`. It also has
no native Windows runner. The Windows ACL, junction, and Ctrl+C tests therefore
remain source-reviewed but not compiled or executed for Windows.

## 8. Rollback

Reverting this addendum's code restores the released global API but reopens
cross-session catalog contamination. Such a rollback requires an explicit
security exception; it is not a compatibility-only rollback.
