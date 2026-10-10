#!/usr/bin/env python3
"""Offline checks for scripts/affected_tests.py. No cargo, no git network."""

from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location(
    "affected_tests", ROOT / "scripts" / "affected_tests.py"
)
assert SPEC and SPEC.loader
affected = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(affected)


def commands(paths: list[str]) -> list[str]:
    graph = affected.direct_deps()
    plan = affected.plan_for(paths, graph)
    return affected.cargo_lines(plan, locked=False, features="")


class AffectedTests(unittest.TestCase):
    def test_inline_module_filter_runs(self) -> None:
        # spinner.rs keeps its tests inline (`mod tests` in the same file).
        # cli depends on tui, but a leaf module must not pull cli's tests.
        lines = commands(["crates/tui/src/ui/spinner.rs"])
        self.assertEqual(lines, ["cargo test -p whycodes-tui --lib ui::spinner::"])

    def test_path_attr_test_file_maps_to_its_owner(self) -> None:
        lines = commands(["crates/agent/src/agent/turn_tests.rs"])
        joined = "\n".join(lines)
        self.assertIn("cargo test -p whycodes-agent --lib agent::turn::", joined)
        self.assertNotIn("--workspace", joined)

    def test_test_file_does_not_pull_dependents(self) -> None:
        lines = commands(["crates/agent/src/agent/turn_tests.rs"])
        joined = "\n".join(lines)
        self.assertNotIn("whycodes-cli", joined)
        self.assertNotIn("whycodes-tui", joined)

    def test_named_include_follows_normal_module_rules(self) -> None:
        # behavior_eval_tests.rs is included from lib.rs as `mod behavior_eval`
        # and contains no #[test] fns. It is not a `mod tests` file, so it
        # follows the crate-root rule: this crate's tests, plus dependents' --lib.
        lines = commands(["crates/agent/src/behavior_eval_tests.rs"])
        joined = "\n".join(lines)
        self.assertTrue(any(line == "cargo test -p whycodes-agent" for line in lines), lines)
        self.assertNotIn("behavior_eval::", joined)
        self.assertNotIn("--workspace", joined)

    def test_integration_test_stays_on_that_target(self) -> None:
        lines = commands(["crates/cli/tests/cli_args.rs"])
        self.assertEqual(lines, ["cargo test -p whycodes-cli --test cli_args"])

    def test_lib_rs_runs_the_crate_not_unrelated_crates(self) -> None:
        lines = commands(["crates/auth/src/lib.rs"])
        joined = "\n".join(lines)
        self.assertTrue(any(line == "cargo test -p whycodes-auth" for line in lines))
        self.assertIn("whycodes-cli --bin", joined)
        self.assertNotIn("whycodes-cli --lib", joined)
        self.assertNotIn("whycodes-storage", joined)
        self.assertNotIn("--workspace", joined)

    def test_workspace_manifest_runs_everything(self) -> None:
        lines = commands(["Cargo.toml"])
        self.assertEqual(len(lines), 1)
        self.assertIn("cargo test --workspace", lines[0])
        self.assertIn("--skip provider_and_model_dialogs", lines[0])

    def test_docs_select_nothing(self) -> None:
        self.assertEqual(commands(["docs/knowhow.md", "README.md"]), [])

    def test_storage_bundled_feature_follows_who_can_activate_it(self) -> None:
        # `cargo test -p whycodes-config --features whycodes-storage/bundled`
        # is "does not contain this feature". Config never links sqlite.
        # Tools reaches storage through memory's bundled-sqlite alias.
        graph = affected.direct_deps()
        config = affected.cargo_lines(
            affected.plan_for(["crates/config/src/load.rs"], graph),
            locked=True,
            features="whycodes-storage/bundled",
        )
        self.assertEqual(len(config), 1)
        self.assertNotIn("--features", config[0])
        self.assertIn("-p whycodes-config", config[0])

        tools = affected.cargo_lines(
            affected.plan_for(["crates/tools/src/blocking.rs"], graph),
            locked=True,
            features="whycodes-storage/bundled",
        )
        self.assertEqual(len(tools), 1)
        self.assertIn("--features bundled-sqlite", tools[0])
        self.assertNotIn("whycodes-storage/bundled", tools[0])

        memory = affected.cargo_lines(
            affected.plan_for(["crates/memory/src/lib.rs"], graph),
            locked=True,
            features="whycodes-storage/bundled",
        )
        memory_line = next(line for line in memory if "-p whycodes-memory" in line)
        self.assertIn("--features whycodes-storage/bundled", memory_line)

        agent = affected.cargo_lines(
            affected.plan_for(["crates/agent/src/agent/turn.rs"], graph),
            locked=True,
            features="whycodes-storage/bundled",
        )
        agent_line = next(line for line in agent if "-p whycodes-agent" in line)
        self.assertIn("--features whycodes-memory/bundled-sqlite", agent_line)

        # mcp has no alias of its own and reaches storage two hops down
        # (tools -> memory). Without a feature its tests fail to link.
        mcp = affected.cargo_lines(
            affected.plan_for(["crates/mcp/src/lib.rs"], graph),
            locked=True,
            features="whycodes-storage/bundled",
        )
        mcp_line = next(line for line in mcp if "-p whycodes-mcp" in line)
        self.assertIn("--features whycodes-tools/bundled-sqlite", mcp_line)

        workspace = affected.cargo_lines(
            affected.plan_for(["Cargo.toml"], graph),
            locked=True,
            features="whycodes-storage/bundled",
        )
        self.assertIn("--workspace", workspace[0])
        self.assertIn("--features whycodes-storage/bundled", workspace[0])

    def test_several_lib_filters_are_one_testname_plus_harness_args(self) -> None:
        # cargo test takes a single TESTNAME. A second filter before `--`
        # is rejected (`unexpected argument 'merge::'`).
        lines = commands(
            [
                "crates/config/src/load.rs",
                "crates/config/src/merge.rs",
                "crates/config/src/types.rs",
            ]
        )
        self.assertEqual(len(lines), 1)
        line = lines[0]
        self.assertTrue(line.startswith("cargo test -p whycodes-config --lib "))
        self.assertIn(" -- ", line)
        before, after = line.split(" -- ", 1)
        self.assertEqual(before.count("::"), 1)
        self.assertEqual(after.split(), ["merge::", "types::"])

    def test_binary_only_crate_uses_bin_not_lib(self) -> None:
        # whycodes-cli has `src/main.rs` and no lib.rs. `--lib` is
        # `no library targets found` and Test (linux) exits before any test.
        lines = commands(["crates/cli/src/cmd/hang.rs"])
        self.assertEqual(len(lines), 1)
        self.assertIn("-p whycodes-cli --bin whycodes", lines[0])
        self.assertIn("cmd::hang::", lines[0])
        self.assertNotIn("--lib", lines[0])

        several = commands(
            [
                "crates/cli/src/cmd/hang.rs",
                "crates/cli/src/cmd/github.rs",
            ]
        )
        self.assertEqual(len(several), 1)
        self.assertIn("--bin whycodes cmd::github:: -- cmd::hang::", several[0])
        self.assertNotIn("--lib", several[0])

    def test_mod_rs_is_a_module_not_the_crate(self) -> None:
        lines = commands(["crates/tui/src/ui/mod.rs"])
        self.assertEqual(lines, ["cargo test -p whycodes-tui --lib ui::"])

    def test_graph_has_no_edge_from_core_to_cli(self) -> None:
        graph = affected.direct_deps()
        self.assertNotIn("cli", graph["core"])
        self.assertIn("core", graph["cli"])


if __name__ == "__main__":
    raise SystemExit(unittest.main())
