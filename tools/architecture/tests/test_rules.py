"""Adversarial cases for the guards: meaningful violations fail; test code stays local."""
import pytest

from architecture import (
    Fact, RustFile, cfg_value, dependency_graph, find_cycle, source_policy_errors,
    workspace_policy_errors,
)


def inspect(source, package="llm-history-app", module="", aliases=None):
    return RustFile("sample.rs", package, module, source.encode(), aliases or {}).inspect()[0]


@pytest.mark.parametrize("source,rule", [
    ('fn run() { println!("hello"); }', "library-terminal-macro"),
    ('fn run() { std::eprintln!("hello"); }', "library-terminal-macro"),
    ('use std::{println as emit}; fn run() { emit!("hello"); }', "library-terminal-macro"),
    ('use std::io::stdout as out; fn run() { let _ = out(); }', "library-frontend-effect"),
    ('fn run() { std::process::exit(1); }', "library-frontend-effect"),
    ('impl History { fn run(&self) { self.con.query_row(); } }', "connection-member"),
])
def test_effects_are_detected_from_syntax_including_aliases(source, rule):
    assert rule in {f.rule for f in inspect(source)}


@pytest.mark.parametrize("source", [
    '// println!("fake");\nfn run() { let _ = "x.con"; }',
    'fn run() { let _ = r#"use llm_history_store::Store; println!("fake")"#; }',
    '#[cfg(test)] mod tests { fn run() { println!("test"); x.con; } }',
    '#[cfg(all(test, unix))] fn run() { println!("test"); x.con; }',
    '#[cfg(not(any(not(test), feature = "other")))] fn run() { x.con; }',
])
def test_comments_strings_and_test_only_code_are_not_production(source):
    assert inspect(source) == []


@pytest.mark.parametrize("cfg", ["not(test)", 'any(test, feature = "other")', 'all(not(test), unix)'])
def test_production_and_unknown_feature_branches_remain_checked(cfg):
    assert inspect(f"#[cfg({cfg})] fn run() {{ x.con; }}")


def test_cli_test_dependency_is_not_a_production_storage_escape_hatch():
    source = 'use llm_history_store::{Store as Db}; fn run() { Db::open(); }'
    assert "cli-storage-bypass" in {f.rule for f in inspect(source, "llm-history-cli")}
    assert inspect('#[cfg(test)] mod tests { ' + source + ' }', "llm-history-cli") == []


def test_renamed_backend_connection_exports_are_detected():
    source = 'use backend::Connection as Conn; pub struct Store { pub con: Conn }'
    facts = inspect(source, "llm-history-store", aliases={"backend": "duckdb"})
    assert {f.rule for f in facts} == {"public-database-connection"}


def test_public_engine_wrappers_cannot_reopen_the_store_connection_escape_hatch():
    source = 'use llm_history_store_engine::Database as Db; pub struct Store { pub database: Db }'
    assert {f.rule for f in inspect(source, "llm-history-store")} == {"public-database-connection"}


def test_parser_state_can_use_paths_but_cannot_open_files():
    source = 'use std::{fs::File as F, path::Path}; impl ParserState { fn new() { Path::new("x"); F::open("x"); } }'
    facts = inspect(source, "llm-history-ingest", module="state")
    assert {f.rule for f in facts} == {"pure-boundary-effect"}

@pytest.mark.parametrize("source", [
    "use llm_history_types::Harness as H; fn run(h: H) { if h == H::Claude {} }",
    "use llm_history_types::Harness::Codex as C; fn run(h: llm_history_types::Harness) { if h == C {} }",
])
def test_app_ingestion_leaves_concrete_harness_policy_to_adapters(source):
    assert "app-ingestion-harness-policy" in {
        f.rule for f in inspect(source, module="ingestion::legacy")
    }
    assert inspect(source, "llm-history-ingest", module="source") == []


@pytest.mark.parametrize("module", [
    "json", "presence", "codex::record", "claude_code::record", "opencode::row",
    "pi::record", "tau::record",
    "decode", "state", "codex::decode", "codex::state",
    "claude_code::decode", "claude_code::state",
])
@pytest.mark.parametrize("source", [
    'use std::fs::File as Input; fn read() { Input::open("history"); }',
    'use rusqlite::Connection; fn read() { Connection::open("history"); }',
])
def test_ingestion_decoders_keep_acquisition_outside_decoding(module, source):
    facts = inspect(source, "llm-history-ingest", module=module)
    assert "pure-boundary-effect" in {f.rule for f in facts}


@pytest.mark.parametrize("source", [
    'use std::fs::File; fn open() { File::open("history"); }',
    'use rusqlite::Connection; pub struct Request { con: Connection }',
])
def test_shared_types_are_independent_of_acquisition_and_storage(source):
    assert "pure-boundary-effect" in {f.rule for f in inspect(source, "llm-history-types")}


@pytest.mark.parametrize("source", [
    'pub use llm_history_types::{Fact, Harness};',
    'use llm_history_types as values; pub use values::Fact as OldFact;',
])
def test_shared_types_are_imported_from_their_owner(source):
    assert "foreign-reexport" in {f.rule for f in inspect(source, "llm-history-core")}


@pytest.mark.parametrize("source", [
    'pub use llm_history_store::{Filter, Message};',
    'pub use llm_history_models::Embedder;',
    'use llm_history_search as search; pub use search::check_rerank;',
])
def test_application_exports_do_not_forward_other_crates(source):
    assert "foreign-reexport" in {f.rule for f in inspect(source)}


def test_external_library_type_alias_exports_are_also_rejected():
    facts = inspect('pub use json::Error as JsonError;', "llm-history-ingest",
                    aliases={"json": "serde_json"})
    assert "foreign-reexport" in {f.rule for f in facts}


def test_public_glob_exports_are_rejected_but_private_globs_are_unrelated():
    assert "wildcard-public-export" in {f.rule for f in inspect('pub use api::*;')}
    assert inspect('use api::*;') == []


@pytest.mark.parametrize("source", [
    'pub type OldMessage = llm_history_types::query::Message;',
    'use llm_history_types::query::Message as M; pub type OldMessage = M;',
])
def test_public_type_aliases_cannot_recreate_foreign_export_paths(source):
    assert "foreign-type-alias" in {f.rule for f in inspect(source)}


def test_private_and_owned_compound_type_aliases_remain_available():
    assert inspect('type Row = (String, usize);') == []
    assert inspect('pub type Rows = Vec<String>;') == []


def test_cli_can_use_the_archive_file_api_without_access_to_store():
    assert inspect('use llm_history_store::archives::{verify_archive, restore_archive, list_archives};',
                   "llm-history-cli") == []
    assert "cli-storage-bypass" in {f.rule for f in inspect(
        'use llm_history_store::Store; fn run() { Store::open(); }', "llm-history-cli")}


@pytest.mark.parametrize("source", [
    'use std::fs::File; fn read() { File::open("history"); }',
    'use rusqlite::Connection; fn read() { Connection::open("source"); }',
    'use duckdb::Connection as Db; fn read() { Db::open("store"); }',
    'use llm_history_ingest::ParserState;',
    'use llm_history_store_engine::Database;',
    'fn write(tx: Transaction) { tx.commit(); }',
    'fn write(tx: Transaction) { tx.rollback(); }',
    'fn write(db: Database) { db.begin(); }',
])
def test_native_storage_borrows_engine_transactions_and_never_acquires_sources(source):
    facts = inspect(source, "llm-history-store-harnesses")
    assert "native-repository-acquisition" in {f.rule for f in facts}


def test_native_storage_can_accept_borrowed_transaction_capabilities():
    assert inspect('use llm_history_store_engine::WriteTransaction; fn write(tx: &WriteTransaction) { tx.connection(); }',
                   "llm-history-store-harnesses") == []


@pytest.mark.parametrize("package", ["llm-history-app", "llm-history-cli", "llm-history-ingest"])
def test_engine_connections_stay_inside_the_storage_family(package):
    assert "engine-storage-boundary" in {f.rule for f in inspect(
        'use llm_history_store_engine::Database;', package)}


def test_private_shared_type_imports_and_same_crate_exports_are_allowed():
    assert inspect('use llm_history_types::Fact;', "llm-history-core") == []
    assert inspect('pub use fact::Fact;', "llm-history-types") == []


@pytest.mark.parametrize("module", ["types", "api", "api::search"])
def test_application_contract_modules_cannot_depend_on_workflows(module):
    facts = inspect('use crate::jobs::Runner; pub struct Request { run: Runner }', module=module)
    assert "api-workflow-dependency" in {f.rule for f in facts}


def test_inline_contract_modules_have_the_same_boundary():
    facts = inspect('pub mod api { use crate::runtime::Engine; }')
    assert "api-workflow-dependency" in {f.rule for f in facts}


def test_library_callers_can_supply_output_writers():
    assert inspect('use std::io::Write; fn export(out: &mut impl Write) { let _ = out.write_all(b"x"); }') == []


def test_exception_is_narrow_and_shrinks_when_debt_is_removed():
    fact = Fact("connection-member", "sample.rs", "History::open", "s.con", 1)
    exception = dict(rule=fact.rule, path=fact.path, context=fact.context, count=1, reason="typed operation pending")
    assert source_policy_errors([fact], [exception]) == []
    assert source_policy_errors([fact, fact], [exception])
    assert source_policy_errors([], [exception])
    assert source_policy_errors([Fact(fact.rule, fact.path, "new_operation", fact.detail, 2)], [exception])


def test_acyclicity_handles_diamonds_and_reports_cycle_path():
    graph = {"app": {"search", "store"}, "search": {"store"}, "store": set()}
    assert find_cycle(graph) == []
    graph["store"].add("app")
    cycle = find_cycle(graph)
    assert cycle[0] == cycle[-1]
    assert all(b in graph[a] for a, b in zip(cycle, cycle[1:]))


def test_target_and_optional_production_edges_are_checked_and_dev_edges_are_separate():
    packages = {
        "app": {"dependencies": [dict(name="core", kind=None, optional=True, target="cfg(windows)")]},
        "core": {"dependencies": [dict(name="app", kind="dev")]},
    }
    assert dependency_graph(packages) == {"app": {"core"}, "core": set()}
    policy = {"workspace_dependencies": {"app": [], "core": []}}
    assert "Unreviewed production dependency" in workspace_policy_errors(packages, policy)[0]


def test_dependency_aliases_do_not_bypass_backend_ownership():
    packages = {"core": {"dependencies": [dict(name="duckdb", rename="backend", kind=None)]}}
    policy = {"workspace_dependencies": {"core": []}, "external_dependency_owners": {"duckdb": ["store"]}}
    assert "duckdb" in workspace_policy_errors(packages, policy)[0]


def test_stale_dependency_exception_is_reported():
    packages = {"cli": {"dependencies": []}, "core": {"dependencies": []}}
    policy = {"workspace_dependencies": {"cli": [], "core": []},
              "legacy_dependency": [dict(source="cli", target="core", reason="temporary helper use")]}
    assert "stale" in workspace_policy_errors(packages, policy)[0]


@pytest.mark.parametrize("expression,value", [
    ("test", False), ("not(test)", True), ("all(test, unix)", False),
    ('any(test, feature = "x,y")', None), ('all(not(test), feature = "x")', None),
    ("not(any(not(test), unix))", False),
])
def test_cfg_projection_keeps_production_feature_arms(expression, value):
    assert cfg_value(expression) is value
