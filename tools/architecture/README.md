# Architecture checks

Run `mise run architecture` from the repository root. `check` and `test` include it.
The task creates/syncs this directory's `.venv` from `uv.lock` with frozen
dependencies and invokes pytest. It never installs into system/user Python.

- `policy.toml`: selected crate ownership rules and narrow, explained legacy debt.
- `architecture.py`: Cargo metadata, production Rust module discovery and AST rules.
- `tests/test_architecture.py`: checks this repository.
- `tests/test_rules.py`: adversarial cases proving violations are rejected while
  comments, strings, test-only code and supported imports are handled correctly.

The dated [architecture assessment](../../docs/architecture.md) describes current
ownership, candidate evolution, testing levels and reassessment triggers. Its
module sketch is descriptive. The executable rules are a smaller reviewed set of
invariants; legitimate boundary changes can update them with an explanation.

Update dependencies intentionally:

```sh
UV_EXCLUDE_NEWER=3d mise exec -- uv lock --project tools/architecture
mise run architecture
```

Review pyproject/lockfile diffs. Tree-sitter 0.25.2 is pinned after 0.26.0 reproduced
a garbage-collection segfault during the full repository scan on the pinned Python
3.12/macOS environment. Dependency upgrades should exercise the full scan as well
as the small rule cases.

No Rust build is needed for these checks. Cargo metadata runs locked/offline with
a 30-second bound. The AST scanner follows runtime module declarations, excludes
test-only branches and conservatively includes unknown feature/platform branches.
It does not expand macros or provide full Rust name/type resolution. Prefer
compiler visibility/dependency boundaries where practical and use these checks
for targeted architectural edges. Keep the public behavior suites as well.

Public exports may expose deliberately owned items from private same-crate
modules. Foreign public re-exports, direct public aliases to foreign types and
wildcard public exports fail source checks,
including grouped/renamed imports. CLI storage access is restricted to the three
public archive-file functions; database workflows stay behind History. Test-only
storage setup/fault injection is assessed separately.

Engine types are available only to the three storage packages. Native table
repositories accept borrowed transactions; they cannot acquire source files,
SQLite databases or engine connections, nor depend on ingestion implementations.
Store's former raw-connection/public-field exceptions have been retired.
The application ingestion workflow may work with harness values, but direct
references to concrete harness variants belong in ingest.

## Codex history documentation inventory

The source-pinned harness specification has an optional, explicit-checkout checker:

```sh
uv run --frozen --no-sync --project tools/architecture python \
  tools/architecture/codex_history_docs.py --source <codex-checkout>
```

It compares recorded enum sets with source ASTs, checks four historical source
snapshots, replays public SQLite migrations in memory, and validates source links,
JSON catalogs and examples. It does not access live history or build Codex.
It is separate from the standard architecture task because that task requires only
this repository. Schema/catalog regeneration flags are documented in the script's
help; review the source pins and changes before regenerating them.
