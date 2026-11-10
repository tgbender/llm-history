"""Architecture facts from Cargo metadata and Rust syntax, without building Rust.

The AST rules cover explicit source paths/import aliases, fields and macros.
They do not expand Rust macros or implement Rust's name/type resolution.
"""
from __future__ import annotations

from collections import Counter
from dataclasses import dataclass
from pathlib import Path
import json
import re
import subprocess
import tomllib

from tree_sitter import Language, Node, Parser
import tree_sitter_rust

ROOT = Path(__file__).resolve().parents[2]
LANGUAGE = Language(tree_sitter_rust.language())
SCOPED = {"scoped_identifier", "scoped_type_identifier"}
COMMENTS = {"line_comment", "block_comment"}


def load_policy() -> dict:
    with Path(__file__).with_name("policy.toml").open("rb") as file:
        return tomllib.load(file)


def cargo_metadata(root: Path = ROOT) -> dict:
    result = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked", "--offline"],
        cwd=root, capture_output=True, text=True, timeout=30, check=False,
    )
    if result.returncode:
        raise AssertionError(f"Cargo metadata failed:\n{result.stderr}")
    return json.loads(result.stdout)


def workspace_packages(metadata: dict) -> dict[str, dict]:
    members = set(metadata["workspace_members"])
    return {p["name"]: p for p in metadata["packages"] if p["id"] in members}


def dependency_graph(packages: dict[str, dict]) -> dict[str, set[str]]:
    # Include build, optional and target-specific edges; dev-dependencies are separate.
    return {name: {d["name"] for d in p["dependencies"]
                   if d["name"] in packages and d["kind"] != "dev"}
            for name, p in packages.items()}


def find_cycle(graph: dict[str, set[str]]) -> list[str]:
    visited, active, stack = set(), set(), []

    def visit(node):
        if node in active:
            return stack[stack.index(node):] + [node]
        if node in visited:
            return []
        active.add(node)
        stack.append(node)
        for child in sorted(graph[node]):
            if cycle := visit(child):
                return cycle
        stack.pop()
        active.remove(node)
        visited.add(node)
        return []

    for node in sorted(graph):
        if cycle := visit(node):
            return cycle
    return []


def workspace_policy_errors(packages: dict[str, dict], policy: dict) -> list[str]:
    graph = dependency_graph(packages)
    owners = policy["workspace_dependencies"]
    errors = []
    if set(graph) != set(owners):
        errors.append(f"Describe owners for new/removed crates: actual={sorted(graph)}, policy={sorted(owners)}")
    legacy = {(e["source"], e["target"]): e for e in policy.get("legacy_dependency", [])}
    for source, targets in graph.items():
        for target in targets:
            if target not in owners.get(source, []) and (source, target) not in legacy:
                errors.append(f"Unreviewed production dependency: {source} -> {target}")
    for (source, target), exception in legacy.items():
        assert exception["reason"].strip(), f"dependency exception needs a reason: {exception}"
        if target not in graph.get(source, set()) or target in owners.get(source, []):
            errors.append(f"Remove stale dependency exception: {source} -> {target}")
    for name, package in packages.items():
        for dependency in package["dependencies"]:
            if dependency["kind"] == "dev":
                continue
            allowed = policy.get("external_dependency_owners", {}).get(dependency["name"])
            if allowed is not None and name not in allowed:
                errors.append(f"Backend/frontend dependency {dependency['name']} belongs to {allowed}, found in {name}")
    return errors


def text(node: Node) -> str:
    return node.text.decode("utf-8")


def expand(path: str, aliases: dict[str, str]) -> str:
    path = re.sub(r"\s+", "", path).lstrip(":").replace("r#", "")
    for _ in range(16):
        head, separator, tail = path.partition("::")
        if head not in aliases or aliases[head] == head:
            return path
        replacement = aliases[head] + (separator + tail if separator else "")
        if replacement == path:
            return path
        path = replacement
    raise AssertionError(f"Cyclic import aliases while inspecting {path}")


def use_bindings(node: Node, prefix: str = "") -> list[tuple[str, str]]:
    """Read grouped/renamed Rust use trees; do not search comments or string contents."""
    if node.type == "use_declaration":
        return use_bindings(node.child_by_field_name("argument"), prefix)
    if node.type == "use_list":
        return [binding for child in node.named_children for binding in use_bindings(child, prefix)]
    if node.type == "scoped_use_list":
        path = prefix + text(node.child_by_field_name("path")) + "::"
        return use_bindings(node.child_by_field_name("list"), path)
    if node.type == "use_as_clause":
        return [(text(node.child_by_field_name("alias")), prefix + text(node.child_by_field_name("path")))]
    if node.type == "use_wildcard":
        return [("*", prefix + text(node))]
    path = prefix + text(node)
    if node.type == "self":
        path = prefix.removesuffix("::")
    return [(path.split("::")[-1], path)]


def cfg_value(expression: str) -> bool | None:
    """Evaluate test=false; retain unknown feature/platform branches conservatively."""
    expression = expression.strip()
    if expression == "test":
        return False
    match = re.fullmatch(r"(all|any|not)\s*\((.*)\)", expression, re.S)
    if not match:
        return None
    operator, body = match.groups()
    parts, start, depth, quoted, escaped = [], 0, 0, False, False
    for i, char in enumerate(body):
        if char == '"' and not escaped:
            quoted = not quoted
        if not quoted:
            depth += (char == "(") - (char == ")")
            if char == "," and depth == 0:
                parts.append(body[start:i])
                start = i + 1
        escaped = char == "\\" and not escaped
    parts.append(body[start:])
    values = [cfg_value(part) for part in parts if part.strip()]
    if operator == "not":
        return None if len(values) != 1 or values[0] is None else not values[0]
    if operator == "all":
        return False if False in values else (None if None in values else True)
    return True if True in values else (None if None in values else False)


def production_children(node: Node) -> list[tuple[Node, list[str]]]:
    children, attributes = [], []
    for child in node.named_children:
        if child.type in COMMENTS:
            continue
        if child.type == "attribute_item":
            attributes.append(text(child))
            continue
        disabled = False
        for attribute in attributes:
            match = re.fullmatch(r"#\[\s*cfg\s*\((.*)\)\s*\]", attribute, re.S)
            disabled |= bool(match and cfg_value(match.group(1)) is False)
        if not disabled:
            children.append((child, attributes))
        attributes = []
    return children


@dataclass(frozen=True)
class Fact:
    rule: str
    path: str
    context: str
    detail: str
    line: int

    @property
    def key(self) -> tuple[str, str, str]:
        return self.rule, self.path, self.context

    def describe(self) -> str:
        return f"{self.path}:{self.line} [{self.rule}] {self.context}: {self.detail}"


@dataclass
class RustFile:
    path: str
    package: str
    module: str
    source: bytes
    aliases: dict[str, str]

    def inspect(self) -> tuple[list[Fact], list[tuple[str, list[str]]]]:
        tree = Parser(LANGUAGE).parse(self.source)
        assert not tree.root_node.has_error, f"Rust syntax parse failed: {self.path}"
        facts, modules = [], []

        def emit(rule, node, context, detail):
            facts.append(Fact(rule, self.path, context, detail, node.start_point.row + 1))

        def check_path(path, node, context, module):
            storage_packages = {"llm-history-store", "llm-history-store-engine", "llm-history-store-harnesses"}
            if self.package not in storage_packages and path.startswith("llm_history_store_engine::"):
                emit("engine-storage-boundary", node, context, path)
            if self.package == "llm-history-store-harnesses":
                acquisition = ("std::fs", "std::net", "std::process", "rusqlite", "reqwest",
                               "llm_history_ingest", "llm_history_store_engine::Database",
                               "llm_history_store_engine::DatabaseLock")
                if any(path == prefix or path.startswith(prefix + "::") for prefix in acquisition) or path.startswith("duckdb::Connection::open"):
                    emit("native-repository-acquisition", node, context, path)
            archive_api = {
                "llm_history_store::archives::verify_archive",
                "llm_history_store::archives::list_archives",
                "llm_history_store::archives::restore_archive",
            }
            if self.package == "llm-history-cli" and path.startswith("llm_history_store::") and path not in archive_api:
                emit("cli-storage-bypass", node, context, path)
            if self.package != "llm-history-cli" and path in {"std::io::stdout", "std::io::stderr", "std::process::exit"}:
                emit("library-frontend-effect", node, context, path)
            if self.package == "llm-history-app" and (module == "ingestion" or module.startswith("ingestion::")):
                variant = path.removeprefix("llm_history_types::Harness::")
                if variant != path and (variant == "*" or variant[:1].isupper()):
                    emit("app-ingestion-harness-policy", node, context, path)
            pure_decoder = self.package == "llm-history-types" or self.package == "llm-history-ingest" and (
                module in {"json", "presence", "decode", "state", "content", "codex::catalog", "claude_code::catalog"}
                or module.endswith(("::record", "::row", "::decode", "::state", "::content"))
            ) or self.package == "llm-history-core" and (
                module in {"provider", "json_string"}
            )
            data_contract = self.package == "llm-history-app" and (
                module == "types" or module == "api" or module.startswith("api::")
            )
            if pure_decoder or data_contract:
                banned = ("std::fs", "std::net", "std::process", "duckdb", "rusqlite", "reqwest", "clap")
                if any(path == prefix or path.startswith(prefix + "::") for prefix in banned):
                    emit("pure-boundary-effect", node, context, path)
            if data_contract and any(path == prefix or path.startswith(prefix + "::") for prefix in (
                "crate::History", "crate::operations", "crate::jobs", "crate::runtime", "llm_history_store::Store",
            )):
                emit("api-workflow-dependency", node, context, path)

        def walk(node, context, aliases, module):
            children = production_children(node)
            local = dict(aliases)
            for child, _ in children:
                if child.type == "use_declaration":
                    for alias, path in use_bindings(child):
                        if alias != "*":
                            local[alias] = expand(path, aliases)
            for child, attributes in children:
                current = context
                if child.type == "impl_item":
                    current = text(child.child_by_field_name("type"))
                elif child.type in {"struct_item", "enum_item", "trait_item"}:
                    current = text(child.child_by_field_name("name"))
                elif child.type in {"function_item", "function_signature_item"}:
                    name = text(child.child_by_field_name("name"))
                    current = f"{context}::{name}" if context else name
                if child.type == "mod_item":
                    name = text(child.child_by_field_name("name"))
                    submodule = "::".join(filter(None, (module, name)))
                    if body := child.child_by_field_name("body"):
                        walk(body, "", local, submodule)
                    else:
                        modules.append((submodule, attributes))
                    continue
                if child.type == "type_item" and any(
                    c.type == "visibility_modifier" and text(c) == "pub" for c in child.named_children
                ):
                    target = child.child_by_field_name("type")
                    if target is not None:
                        resolved = expand(text(target), local)
                        dependency = resolved.partition("::")[0]
                        if dependency.startswith("llm_history_") or dependency in self.aliases.values():
                            emit("foreign-type-alias", child, current, resolved)
                if child.type == "use_declaration":
                    for binding, path in use_bindings(child):
                        resolved = expand(path, local)
                        check_path(resolved, child, current, module)
                        public = any(c.type == "visibility_modifier" and text(c) == "pub"
                                     for c in child.named_children)
                        dependency = resolved.partition("::")[0]
                        foreign = dependency.startswith("llm_history_") or dependency in self.aliases.values()
                        if public and foreign:
                            emit("foreign-reexport", child, current, resolved)
                        if public and binding == "*":
                            emit("wildcard-public-export", child, current, resolved)
                    continue
                if child.type in SCOPED and node.type not in SCOPED:
                    check_path(expand(text(child), local), child, current, module)
                if child.type == "macro_invocation":
                    macro = expand(text(child.child_by_field_name("macro")), local)
                    if self.package != "llm-history-cli" and macro.split("::")[-1] in {"print", "println", "eprint", "eprintln", "dbg"}:
                        emit("library-terminal-macro", child, current, macro)
                if child.type == "field_expression" and text(child.child_by_field_name("field")) == "con" and self.package != "llm-history-store":
                    emit("connection-member", child, current, text(child))
                if child.type == "field_expression" and self.package == "llm-history-store-harnesses" and text(child.child_by_field_name("field")) in {
                    "commit", "rollback", "begin", "transaction", "unchecked_transaction",
                }:
                    emit("native-repository-acquisition", child, current, text(child))
                if child.type == "field_declaration" and any(c.type == "visibility_modifier" and text(c) == "pub" for c in child.named_children):
                    field_type = expand(text(child.child_by_field_name("type")), local)
                    if field_type in {"duckdb::Connection", "rusqlite::Connection", "llm_history_store_engine::Database"}:
                        emit("public-database-connection", child, current, text(child))
                walk(child, current, local, module)

        walk(tree.root_node, "", self.aliases, self.module)
        return facts, modules


def production_sources(packages: dict[str, dict], root: Path = ROOT) -> list[RustFile]:
    files, visited = [], set()

    def visit(path, package, module, aliases, module_dir):
        path = path.resolve()
        if (path, module) in visited:
            return
        visited.add((path, module))
        assert path.is_relative_to(root), f"source outside repository needs review: {path}"
        source = RustFile(path.relative_to(root).as_posix(), package, module, path.read_bytes(), aliases)
        files.append(source)
        _, modules = source.inspect()
        for name, attributes in modules:
            override = next((re.fullmatch(r'#\[path\s*=\s*"([^"]+)"\]', a) for a in attributes if a.startswith("#[path")), None)
            if override:
                child = path.parent / override.group(1)
            else:
                relative = Path(*name.removeprefix(module + "::").split("::")) if module else Path(*name.split("::"))
                candidates = [module_dir / relative.with_suffix(".rs"), module_dir / relative / "mod.rs"]
                found = [p for p in candidates if p.is_file()]
                assert len(found) == 1, f"resolve Rust module {package}::{name}: {candidates}"
                child = found[0]
            child_dir = child.parent if child.name == "mod.rs" else child.with_suffix("")
            visit(child, package, name, aliases, child_dir)

    for name, package in packages.items():
        aliases = {(d["rename"] or d["name"]).replace("-", "_"): d["name"].replace("-", "_")
                   for d in package["dependencies"] if d["kind"] != "dev"}
        for target in package["targets"]:
            if set(target["kind"]) & {"lib", "bin", "rlib", "proc-macro"}:
                path = Path(target["src_path"])
                visit(path, name, "", aliases, path.parent)
    return files


def source_policy_errors(facts: list[Fact], exceptions: list[dict]) -> list[str]:
    actual = Counter(f.key for f in facts)
    expected = Counter()
    errors = []
    for exception in exceptions:
        key = exception["rule"], exception["path"], exception["context"]
        assert exception["reason"].strip(), f"exception needs a reason: {key}"
        assert key not in expected, f"duplicate exception: {key}"
        expected[key] = exception["count"]
    for key, count in actual.items():
        if count > expected[key]:
            errors.extend(f.describe() for f in facts if f.key == key)
    for key, count in expected.items():
        if actual[key] < count:
            errors.append(f"Remove/shrink stale exception {key}: allowed {count}, observed {actual[key]}")
    return errors
