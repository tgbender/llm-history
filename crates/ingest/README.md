# llm-history-ingest

Ingestion adapters for native coding-agent histories. One crate owns this specific
integration set, with a module per harness and shared JSON support inside it.
Other adapter families can have their own owners when they are needed.

```text
crates/ingest/
  Cargo.toml
  src/
    lib.rs
    json.rs
    presence.rs
    decode.rs
    state.rs
    catalog.rs
    prepare.rs
    source.rs
    codex/{mod.rs, record.rs, decode.rs, state.rs, catalog.rs, source.rs}
    claude_code/{mod.rs, record.rs, decode.rs, state.rs, catalog.rs}
    cursor/{mod.rs, decode.rs, content.rs, source.rs, wire.rs}
    opencode/{mod.rs, row.rs}
    pi/{mod.rs, record.rs}
    tau/{mod.rs, record.rs}
  tests/
    json.rs
    codex.rs
    claude_code.rs
    opencode.rs
    pi.rs
    tau.rs
    decoder_compatibility.rs
    reference.rs
```

This is the implemented snapshot on **2026-11-09**. Reassess module and crate
boundaries as real dependencies and responsibilities grow. Add modules such as
content, tools, state or source when they have actual implementation to own.

## Current boundary

The Codex, Claude, and Cursor semantic decoders and continuation state live
here. Live import and retained replay use the same `ParserState`.
The checkpoint field layout, parser revision 6 and fact output are preserved.
Native path/session metadata, context/token decoding and exclusions are owned by
their respective harness modules. Shared sequencing stays in `decode.rs`.

`llm-history-types` owns shared identities, facts and metadata values. Consumers
import them from that crate directly. `core` supplies bounded file capture,
timestamp/title helpers and search-text policy. `source` owns native transcript
directories and stable keys. `prepare` selects
unchanged, append, or replacement work and requests previously counted usage keys
only when resuming a Claude transcript. The app supplies those keys from Store
and coordinates publication. Storage owns the transaction and checkpoint checks.

All harness modules also have open native envelope/row views with typed access
to a few identity/order fields. Their tests use public synthetic examples from
the [harness format reference](../../docs/harness-history/README.md). These views
preserve unknown values; the migrated semantic decoders retain existing behavior.
OpenCode/Pi/Tau require semantic decoding before application ingestion support.

Harness modules own native interpretation and compatibility handling. Pure record
decoding accepts retained inputs. Codex SQLite thread/project acquisition now lives
in `codex::source`, pure global-catalog decoding in the harness catalog modules,
and bounded file/fingerprint/selection handling in `catalog`. Bounded capture mechanics, storage/CAS
writes and application workflow sequencing retain their own owners. Architecture
checks protect pure decoder modules and prevent storage/model/CLI dependencies.
Persistence receives captured metadata values through the
[storage harness owner](../store/README.md); table writers perform no acquisition.

`fixtures/decoder-v7.json` freezes complete facts and serialized checkpoints from
the implementation before extraction: ten synthetic cases and 52 records. Revision 7
changed which user-role records count as harness-written and left these facts as they were. Tests
compare uninterrupted decoding and restoration of existing checkpoints. Resume
expectations also preserve the original metadata-revision behavior caused by
fields omitted from checkpoint serialization. Existing public app/CLI contracts
protect end-to-end behavior. Changing these expectations requires reviewing an
intentional behavior change.

Use a fixture and a focused reader change for a real structural break. Routine
new fields and variants pass through the open JSON boundary. A vendor release
does not require a new module or reader version.

## JSON preservation

Shared primitives are exported from the crate root:

- `JsonRecord`: one JSON object, complete original source and raw field values.
- `Presence<T>`: `Missing`, `Null` or `Value(T)`.
- Typed accessors and `FieldError`: explicit interpretation of known fields.

Serde's `raw_value` retains unfamiliar values without normalizing numbers or
string escapes. `source()` retains the complete input, including whitespace and
duplicate keys. Field lookup follows Serde's last-key behavior; iteration is
sorted. Changed known-field types fail on that accessor while the record remains
available. Syntax/non-object errors return to the caller, which owns its input.

```rust
use llm_history_ingest::{Presence, codex::Record};

fn read_record() -> Result<(), Box<dyn std::error::Error>> {
    let record = Record::parse(
        r#"{"type":"response_item","payload":{"type":"new_item","body":"hello"},"future":true}"#,
    )?;
    assert_eq!(record.record_type()?, Presence::Value("response_item".into()));
    let Presence::Value(payload) = record.payload()? else { return Ok(()) };
    assert_eq!(payload.field::<String>("type")?, Presence::Value("new_item".into()));
    assert!(record.extras().any(|(name, _)| name == "future"));
    Ok(())
}
```

`opencode::Row` receives a table name and JSON representation of native SQL columns
from acquisition, with SQL TEXT represented as strings and SQL NULL as null.
`json_text_column` distinguishes missing columns, SQL NULL and TEXT containing the
JSON value `null`. This is our row-input boundary; the upstream format reference
documents OpenCode's actual storage. Binary blobs need separate acquisition.

Native source retention and content identity have separate roles. Each supported
transcript adapter selects known reusable text and tool strings for CAS in its
`content` module. The shared extractor returns byte spans and JSON pointers;
the archive owner hashes, compresses and persists the bodies. IDs, timestamps and
other contextual metadata stay on their occurrences. See the
[retained archive design](../../docs/schema/README.md) for format and scope.

Cursor inventory selects one representation per session within the registered
profile. JSONL uses the file capture path. For CLI SQLite, the adapter uses a
read transaction, follows the selected root, and passes bounded retained rows
through the same publication and replay boundary. SQLite access stays in
`cursor::source`; wire and message decoding stay independent of the connection.
