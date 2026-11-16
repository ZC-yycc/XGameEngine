---
name: no-compat-refactor
description: Enforce XGameEngine's no-compatibility-layer rule for framework migrations and refactors. Use when a task adds, replaces, renames, or deletes a core type; changes scene/prefab format, plugin ABI, reflection metadata, editor state, or schema; or when anyone proposes a V2 type, legacy/compat module, shim, deprecated alias, serde alias, double-read/double-write, or "keep both for now" (兼容旧框架 / 平滑迁移 / 过渡层).
---

# No-Compat Refactor

**The rule: migration = replace + dismantle. Old equipment is torn out in the same change.
Two working implementations of one concept are never allowed in the trunk.**

The canonical policy is `Docs/架构收敛约定.md`. Read it before starting; this skill is only
the trigger and the short checklist.

## Hard sequence

1. **Model the replacement.** One concept = one type = one source of truth. If you cannot
   name the single entry point, the design is not finished.
2. **Change every call site.** `cargo check --workspace --all-targets` is the checklist.
   Never add a compat path just to make the tree compile.
3. **Delete the old implementation.** Types, files, modules, tests, docs. No `pub use`
   alias, no `#[deprecated]`, no `#[serde(alias)]`, no `#[allow(dead_code)]` leftovers.
4. **Rewrite data once.** A throwaway script, committed output, script deleted. No runtime
   double-read, no double-write, no auto-migration.
5. **Update derived contracts in the same change.** Schema, `--export-*` manifests, script
   API docs, layout validation.
6. **Run the gates.**
7. **Report** the deletion list and the symbol-count delta in the PR/commit description.

Done means `grep` for the old symbol returns 0 — not "the new path works".

## Stop signs (reject the design, do not "make it safe")

- `Foo` + `FooV2` / `_v2` / `FooNew` / `FooOld`
- `legacy/`, `compat/`, `shim.rs`, `adapter/` for internal code
- `impl From<Old> for New` as a long-lived bridge
- two metadata tables / two discriminator enums / two registries for one concept
- save path that reads the old file back to reconcile
- `#[deprecated]`, `#[serde(alias)]`, `#[serde(default)]` for renamed fields
- "temporarily keep" comments without an issue number and a deletion date

## Allowed exceptions (contracts, not compat layers)

Only things referenced **outside the repository** may be preserved: `guid://` asset identity,
cross-process entity-ID semantics, MCP/CLI JSON field names and exit codes, script API names.
Keep the contract, replace the implementation. Disk format, plugin ABI, internal types,
editor state and schema generation are **fair game for one-shot breakage**.

Ask: *does this leave two working implementations of one concept in the repo?*
Yes → forbidden. No → allowed.

## Gates

```sh
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets
cargo test --workspace --all-targets
powershell -File tools/check-no-compat.ps1
```

Use `-Enforce` when the branch is in convergence mode. When behaviour must not change, also
run the byte/screenshot/headless baselines listed in `AGENTS.md`.

## When the user asks for backward compatibility

State the cost first: two implementations, permanent drift, deletion cost grows every week.
Then offer a one-shot rewrite plus a version bump. Do not silently build a shim.
