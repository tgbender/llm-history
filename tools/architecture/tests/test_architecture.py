"""Repository architecture contracts, deliberately independent of file counts/layout."""
import pytest

from architecture import (
    cargo_metadata, dependency_graph, find_cycle, load_policy, production_sources,
    source_policy_errors, workspace_packages, workspace_policy_errors,
)


@pytest.fixture(scope="session")
def packages():
    return workspace_packages(cargo_metadata())


@pytest.fixture(scope="session")
def policy():
    return load_policy()


@pytest.fixture(scope="session")
def source_facts(packages):
    sources = production_sources(packages)
    assert {s.package for s in sources} == set(packages), "A workspace runtime target was not inspected"
    return [fact for source in sources for fact in source.inspect()[0]]


def test_production_crate_dependencies_are_acyclic(packages):
    cycle = find_cycle(dependency_graph(packages))
    assert not cycle, "Production crate cycle: " + " -> ".join(cycle)


def test_crate_dependency_ownership_and_legacy_edges(packages, policy):
    errors = workspace_policy_errors(packages, policy)
    assert not errors, "\n".join(errors)


@pytest.mark.parametrize("rule", [
    "cli-storage-bypass",
    "library-frontend-effect",
    "library-terminal-macro",
    "pure-boundary-effect",
    "api-workflow-dependency",
    "connection-member",
    "public-database-connection",
    "foreign-reexport",
    "foreign-type-alias",
    "wildcard-public-export",
    "engine-storage-boundary",
    "native-repository-acquisition",
])
def test_rust_source_boundary(rule, source_facts, policy):
    errors = source_policy_errors(
        [f for f in source_facts if f.rule == rule],
        [e for e in policy.get("legacy_source", []) if e["rule"] == rule],
    )
    assert not errors, "\n".join(errors)
