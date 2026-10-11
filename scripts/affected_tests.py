#!/usr/bin/env python3
"""Print the cargo test commands that cover a set of changed files.

CONTRIBUTING.md sends pull requests here instead of `cargo test --workspace`.

A change inside one module should not re-run the rest of that crate, and it
must not re-run crates that do not depend on it. This walks the workspace
dependency graph (the same edges `check_dependency_boundaries.py` reads) and
maps each Rust file to the narrowest cargo filter that still compiles its
tests:

- a library module        -> that crate's `--lib <module>::`
- a binary-only module    -> `--bin <name> <module>::` (no lib target)
- a `#[path]` test file   -> the module that includes it
- `crates/<c>/tests/<t>`  -> `--test <t>` on that crate only
- a public surface        -> the changed crate, plus direct dependents' `--lib`
- crate / workspace root  -> that crate, or the whole workspace

`--execute` runs those commands. Without it, the script only prints them
(one cargo invocation per line) so CI and agents can see the selection.
When more than one crate is selected, `--execute` compiles every selected
test target in one `cargo test --no-run` and then runs each binary with its
own filters. Separate `cargo test -p` calls resolve features per package,
so each one rebuilt serde/tokio/reqwest and most workspace crates again.

`Cargo.lock` widens to the workspace only when an external package that
already existed changes version or dependencies (a bump can reach every
crate). Added or removed packages and workspace-member entries follow the
crate manifests in the same diff.

Usage:
    python scripts/affected_tests.py [--base origin/main] [--execute]
    python scripts/affected_tests.py --paths crates/tui/src/ui/spinner.rs
    git diff --name-only | python scripts/affected_tests.py --stdin
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

try:  # Python 3.11+. Without it Cargo.lock stays a workspace root.
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - old runner python3
    tomllib = None  # type: ignore[assignment]

ROOT = Path(__file__).resolve().parent.parent
CRATES = ROOT / "crates"

# Cargo package names drop the directory hyphen: command-risk -> whycodes-command-risk.
PKG_PREFIX = "whycodes-"

# Process-wide WHYCODES_HOME tests race when many crates share one `cargo test`
# process. CI Test already skips them; keep the skip on any workspace-wide run.
WORKSPACE_SKIPS = (
    "provider_and_model_dialogs_load_custom_from_isolated_home",
    "fill_model_catalog_from_disk_is_a_noop_when_config_load_fails",
)

# A change here can affect every crate's tests.
WORKSPACE_ROOTS = {
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "rustfmt.toml",
}

PATH_ATTR = "#[path"
MOD_LINE = re.compile(r"^\s*(?:pub\s+)?mod\s+([A-Za-z0-9_]+)\s*;")


def has_lib_target(crate: str) -> bool:
    """True when `cargo test -p` can take `--lib`. Binary-only crates cannot."""
    return (CRATES / crate / "src" / "lib.rs").is_file()


def bin_names(crate: str) -> list[str]:
    """Binary target names. Explicit `[[bin]]` wins; otherwise `src/main.rs`."""
    text = (CRATES / crate / "Cargo.toml").read_text(encoding="utf-8")
    names: list[str] = []
    in_bin = False
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("[[") and stripped.endswith("]]"):
            in_bin = stripped == "[[bin]]"
            continue
        if stripped.startswith("[") and stripped.endswith("]"):
            in_bin = False
            continue
        if not in_bin or not stripped.startswith("name") or "=" not in stripped:
            continue
        match = re.search(r'"([^"]+)"', stripped)
        if match and match.group(1) not in names:
            names.append(match.group(1))
    if names:
        return names
    if (CRATES / crate / "src" / "main.rs").is_file():
        return [package_name(crate)]
    return []


def unit_selectors(crate: str) -> list[list[str]]:
    """Cargo args that select this crate's unit tests.

    Library crates use `--lib`. A package with no library (`whycodes-cli`)
    keeps its `#[cfg(test)]` modules on the binary, so `--lib` is
    `no library targets found`.
    """
    if has_lib_target(crate):
        return [["--lib"]]
    return [["--bin", name] for name in bin_names(crate)]


def package_name(crate_dir: str) -> str:
    return PKG_PREFIX + crate_dir


def crate_dirs() -> list[str]:
    return sorted(
        p.name for p in CRATES.iterdir() if (p / "Cargo.toml").is_file()
    )


def direct_deps() -> dict[str, set[str]]:
    """crate directory -> workspace crates it depends on (from its Cargo.toml)."""
    graph: dict[str, set[str]] = {}
    for crate in crate_dirs():
        text = (CRATES / crate / "Cargo.toml").read_text(encoding="utf-8")
        deps: set[str] = set()
        for line in text.splitlines():
            stripped = line.strip()
            if not stripped.startswith("whycodes-"):
                continue
            name = stripped.split("=", 1)[0].strip()
            dep = name.removeprefix(PKG_PREFIX)
            if dep != crate and (CRATES / dep).is_dir():
                deps.add(dep)
        graph[crate] = deps
    return graph


def dependents_of(graph: dict[str, set[str]]) -> dict[str, set[str]]:
    rev: dict[str, set[str]] = defaultdict(set)
    for crate, deps in graph.items():
        for dep in deps:
            rev[dep].add(crate)
    return rev


def changed_files(args: argparse.Namespace) -> list[str]:
    if args.paths:
        return [_norm(p) for p in args.paths]
    if args.stdin:
        return [_norm(line) for line in sys.stdin.read().splitlines() if line.strip()]
    return [_norm(n) for n in _git_changed(args.base) if n.strip()]


def _git_changed(base: str) -> list[str]:
    """Files changed since `base` (PR) or in HEAD (push, `base` is HEAD).

    `base...HEAD` is empty when base is HEAD. A push to main still has to
    test the commit that just landed, so that case reads the commit itself.
    """
    if base != "HEAD":
        triple = subprocess.run(
            ["git", "diff", "--name-only", f"{base}...HEAD"],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
        if triple.returncode == 0 and triple.stdout.strip():
            names = triple.stdout.splitlines()
        else:
            names = _head_commit_files()
    else:
        names = _head_commit_files()
    # Working tree on top of HEAD (local agent runs). CI checkouts are clean.
    dirty = subprocess.run(
        ["git", "diff", "--name-only", "HEAD"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    if dirty.returncode == 0:
        names.extend(dirty.stdout.splitlines())
    return names


def _head_commit_files() -> list[str]:
    head = subprocess.run(
        ["git", "diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    return head.stdout.splitlines() if head.returncode == 0 else []


def _norm(path: str) -> str:
    return path.replace("\\", "/").lstrip("./")


def crate_of(path: str) -> str | None:
    parts = path.split("/")
    if len(parts) >= 2 and parts[0] == "crates" and (CRATES / parts[1]).is_dir():
        return parts[1]
    return None


def is_test_surface(path: str) -> bool:
    """True when the file is a crate root or an integration test.

    `lib.rs` / `main.rs` / `Cargo.toml` only count at the crate root.
    `ui/mod.rs` is a module, not the crate root, even though its name is `mod.rs`.
    """
    name = path.rsplit("/", 1)[-1]
    if "/tests/" in f"/{path}" and name.endswith(".rs"):
        return True
    if name == "Cargo.toml" and path.count("/") == 2 and path.startswith("crates/"):
        return True
    if name in {"lib.rs", "main.rs"} and "/src/" in f"/{path}":
        parent = path.rsplit("/", 1)[0]
        return parent.endswith("/src")
    return False


def module_filter(crate: str, rel_under_src: str) -> str | None:
    """Cargo `--lib` filter for a file under `src/`, or None when the file is
    the crate root (lib.rs / main.rs) and the whole lib must run.
    """
    if rel_under_src in {"lib.rs", "main.rs"}:
        return None
    stem_path = rel_under_src[:-3] if rel_under_src.endswith(".rs") else rel_under_src
    parts = stem_path.split("/")
    if parts[-1] == "mod":
        parts = parts[:-1]
    if not parts:
        return None
    return "::".join(parts) + "::"


def path_attr_owner(crate: str, filename: str) -> tuple[str, str] | None:
    """`(owner module filter, included module name)` for a `#[path]` file.

    `mod tests` is filtered as the owner (`agent::turn::`). Any other include
    (`mod behavior_eval`) is filtered by that module name. The first crate-wide
    match wins; filenames are unique per crate.
    """
    src = CRATES / crate / "src"
    if not src.is_dir():
        return None
    quoted = f'"{filename}"'
    for rs in src.rglob("*.rs"):
        if rs.name == filename:
            continue
        lines = rs.read_text(encoding="utf-8", errors="replace").splitlines()
        for index, line in enumerate(lines):
            if quoted not in line or PATH_ATTR not in line:
                continue
            # `mod name;` is the next non-attribute line. Stop there so a
            # later `mod tests` in the same file is not this include.
            mod_name = ""
            for follow in lines[index:index + 4]:
                stripped = follow.strip()
                if not stripped or stripped.startswith("#["):
                    continue
                match = MOD_LINE.match(follow)
                if match:
                    mod_name = match.group(1)
                break
            if not mod_name:
                continue
            rel = rs.relative_to(src).as_posix()
            filt = module_filter(crate, rel)
            owner = filt if filt is not None else ""
            # Root `mod tests` includes `lib_tests.rs` only. A different
            # filename quoted on the same line is a different module
            # (`mod behavior_eval`) and must not be treated as `mod tests`.
            if rel in {"lib.rs", "main.rs"} and mod_name == "tests" and filename not in {
                "lib_tests.rs",
                "main_tests.rs",
            }:
                continue
            return owner, mod_name
    return None


def integration_target(path: str) -> str | None:
    """`crates/<c>/tests/foo.rs` -> foo. Nested helpers are not cargo targets."""
    parts = path.split("/")
    if len(parts) == 4 and parts[0] == "crates" and parts[2] == "tests" and parts[3].endswith(".rs"):
        return parts[3][:-3]
    return None


class Plan:
    """Selected cargo invocations. A crate in `full` runs every test target.
    `lib_filters[crate]` is a set of `--lib` substrings (empty string = whole lib).
    `integration[crate]` is a set of `--test <name>` targets.
    """

    def __init__(self) -> None:
        self.full: set[str] = set()
        self.lib_filters: dict[str, set[str]] = defaultdict(set)
        self.integration: dict[str, set[str]] = defaultdict(set)
        self.workspace = False
        self.reasons: list[str] = []

    def add_full(self, crate: str, why: str) -> None:
        self.full.add(crate)
        self.reasons.append(f"{package_name(crate)} (full): {why}")

    def add_lib(self, crate: str, filt: str, why: str) -> None:
        if crate in self.full:
            return
        self.lib_filters[crate].add(filt)
        label = filt if filt else "(whole unit tests)"
        kind = "--lib" if has_lib_target(crate) else "--bin"
        self.reasons.append(f"{package_name(crate)} {kind} {label}: {why}")

    def add_integration(self, crate: str, target: str, why: str) -> None:
        if crate in self.full:
            return
        self.integration[crate].add(target)
        self.reasons.append(f"{package_name(crate)} --test {target}: {why}")


def lock_bumps_external(base_lock: str, head_lock: str) -> list[str]:
    """External packages present on both sides whose version or dependency
    list changed. Workspace members and packages that were only added or
    only removed are left to the crate manifests in the same diff."""

    def entries(text: str) -> set[tuple[str, str, tuple[str, ...]]]:
        packages = tomllib.loads(text).get("package", [])
        return {
            (p["name"], p.get("version", ""), tuple(sorted(p.get("dependencies", []))))
            for p in packages
            if not p["name"].startswith(PKG_PREFIX)
        }

    base, head = entries(base_lock), entries(head_lock)
    head_names = {name for name, _, _ in head}
    return sorted({name for name, _, _ in base - head if name in head_names})


def _lock_bumps(base: str) -> list[str] | None:
    """`lock_bumps_external` against `base`, or None when either side is unreadable."""
    if tomllib is None:
        return None
    old = subprocess.run(
        ["git", "show", f"{base}:Cargo.lock"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    try:
        new = (ROOT / "Cargo.lock").read_text(encoding="utf-8")
        if old.returncode != 0:
            return None
        return lock_bumps_external(old.stdout, new)
    except (OSError, ValueError, KeyError):
        return None


def plan_for(
    paths: list[str],
    graph: dict[str, set[str]],
    lock_bumps: list[str] | None = None,
) -> Plan:
    """`lock_bumps` is the external packages a `Cargo.lock` change bumped;
    None (unknown) keeps the lockfile a workspace root."""
    plan = Plan()
    rev = dependents_of(graph)
    if not paths:
        plan.reasons.append("no changed files; nothing to test")
        return plan

    for path in paths:
        if path == "Cargo.lock" and lock_bumps is not None and not lock_bumps:
            plan.reasons.append("Cargo.lock: no external version bumps; crate manifests decide")
            continue
        if path in WORKSPACE_ROOTS or path.startswith(".cargo/"):
            plan.workspace = True
            why = f" ({', '.join(lock_bumps[:5])})" if path == "Cargo.lock" and lock_bumps else ""
            plan.reasons.append(f"workspace: {path}{why}")
            return plan

        crate = crate_of(path)
        if crate is None:
            # Docs, scripts, landing, Formula: no Rust tests.
            continue

        target = integration_target(path)
        if target is not None:
            plan.add_integration(crate, target, path)
            continue

        rel = path.split("/")
        under = "/".join(rel[2:]) if len(rel) > 2 else ""
        if under.startswith("src/") and under.endswith(".rs"):
            src_rel = under[len("src/"):]
            filename = src_rel.rsplit("/", 1)[-1]
            owner = path_attr_owner(crate, filename)
            if owner is not None and owner[1] == "tests":
                # Included as `mod tests`: cargo names those tests under the
                # parent module. A test-only file does not change the public
                # API, so dependents stay out.
                plan.add_lib(crate, owner[0], path)
                continue
            if (is_test_surface(path) or src_rel in {"lib.rs", "main.rs"}) and filename != "mod.rs":
                plan.add_full(crate, path)
                _direct_dependents(plan, crate, rev, path)
                continue
            filt = module_filter(crate, src_rel)
            if filt is None or filename.endswith("_tests.rs"):
                plan.add_full(crate, path)
            else:
                plan.add_lib(crate, filt, path)
            # A leaf module does not get its dependents' tests. Compile
            # breakage is `cargo check`'s job. Dependents' tests run only
            # when the crate root (`lib.rs` / `main.rs` / Cargo.toml) changes,
            # which is handled above.
            continue

        # benches, build.rs, assets inside the crate: the crate's own tests.
        plan.add_full(crate, path)

    return plan


def _direct_dependents(plan: Plan, crate: str, rev: dict[str, set[str]], path: str) -> None:
    for dep in sorted(rev.get(crate, ())):
        if dep in plan.full:
            continue
        plan.add_lib(dep, "", f"depends on {crate} via {path}")


def cargo_lines(plan: Plan, *, locked: bool, features: str) -> list[str]:
    if plan.workspace:
        # `--workspace` may name any member's feature (`whycodes-storage/bundled`).
        # A `-p` line cannot: cargo requires that package to be the feature's
        # owner or a direct dependency.
        line = _cargo(locked, features, ["--workspace"])
        skips = " ".join(f"--skip {name}" for name in WORKSPACE_SKIPS)
        return [f"{line} -- {skips}"]

    lines: list[str] = []
    deps = direct_deps()
    crates = sorted(plan.full | set(plan.lib_filters) | set(plan.integration))
    for crate in crates:
        pkg = package_name(crate)
        pkg_features = features_for_crate(crate, features, deps)
        if crate in plan.full:
            lines.append(_cargo(locked, pkg_features, ["-p", pkg]))
            continue
        filters = sorted(f for f in plan.lib_filters.get(crate, set()) if f)
        whole_lib = "" in plan.lib_filters.get(crate, set())
        targets = sorted(plan.integration.get(crate, set()))
        selectors = unit_selectors(crate)
        if not selectors and (whole_lib or filters or not targets):
            # No lib and no bin: still name the package so cargo reports it.
            selectors = [[]]
        if whole_lib or (not filters and not targets):
            for selector in selectors:
                lines.append(_cargo(locked, pkg_features, ["-p", pkg, *selector]))
        elif filters:
            # Cargo accepts one TESTNAME. Extra filters must follow `--` so
            # the test harness ORs them (`cargo test --lib a:: -- b::` is
            # "unexpected argument" on rustc 1.99). A lone filter stays
            # before `--`; a trailing `--` with nothing after it is noise.
            extra = ["--", *filters[1:]] if len(filters) > 1 else []
            for selector in selectors:
                lines.append(
                    _cargo(
                        locked,
                        pkg_features,
                        ["-p", pkg, *selector, filters[0], *extra],
                    )
                )
        for target in targets:
            lines.append(_cargo(locked, pkg_features, ["-p", pkg, "--test", target]))
    return lines


def features_for_crate(crate: str, features: str, deps: dict[str, set[str]]) -> str:
    """Feature specs `cargo test -p` will accept for this crate.

    CI passes `whycodes-storage/bundled` so Linux runners without
    libsqlite3-dev still link. That spec is legal only when the selected
    package is `whycodes-storage` or depends on it directly. Other crates
    keep a direct dependency's feature that turns the spec on (their own
    alias wins: `whycodes-tools` uses `bundled-sqlite`). A crate with no
    path to the feature, such as `whycodes-config`, gets no `--features`.
    """
    if not features:
        return ""
    kept: list[str] = []
    for spec in re.split(r"[\s,]+", features.strip()):
        if not spec:
            continue
        rewritten = _accept_feature(crate, spec, deps)
        if rewritten and rewritten not in kept:
            kept.append(rewritten)
    return ",".join(kept)


def _accept_feature(crate: str, spec: str, deps: dict[str, set[str]]) -> str | None:
    own = _feature_map(crate)
    if "/" not in spec:
        return spec if spec in own else None
    pkg, feat = spec.split("/", 1)
    if package_name(crate) == pkg and feat in own:
        return spec
    dep_dir = pkg.removeprefix(PKG_PREFIX) if pkg.startswith(PKG_PREFIX) else ""
    if dep_dir and dep_dir in deps.get(crate, ()) and feat in _feature_map(dep_dir):
        return spec
    for dep in sorted(deps.get(crate, ())):
        name = _enabling_feature(dep, spec, deps)
        if name is None:
            continue
        forwarded = f"{package_name(dep)}/{name}"
        for own_name, own_enables in own.items():
            if forwarded in own_enables:
                return own_name
        return forwarded
    return None


def _enabling_feature(crate: str, spec: str, deps: dict[str, set[str]]) -> str | None:
    """This crate's own feature that turns `spec` on, through any depth of
    aliases (`whycodes-mcp` -> tools -> memory -> storage)."""
    pkg, feat = spec.split("/", 1)
    own = _feature_map(crate)
    if package_name(crate) == pkg and feat in own:
        return feat
    for name, enables in own.items():
        if spec in enables:
            return name
        for enabled in enables:
            dep_pkg, _, dep_feat = enabled.partition("/")
            dep_dir = dep_pkg.removeprefix(PKG_PREFIX)
            if (
                dep_feat
                and dep_dir in deps.get(crate, ())
                and _enabling_feature(dep_dir, spec, deps) == dep_feat
            ):
                return name
    return None


def _feature_map(crate: str) -> dict[str, tuple[str, ...]]:
    """`[features]` names in this crate's Cargo.toml, in file order."""
    text = (CRATES / crate / "Cargo.toml").read_text(encoding="utf-8")
    in_features = False
    found: dict[str, tuple[str, ...]] = {}
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            in_features = stripped == "[features]"
            continue
        if not in_features or "=" not in stripped or stripped.startswith("#"):
            continue
        name, _, rest = stripped.partition("=")
        found[name.strip()] = tuple(re.findall(r'"([^"]+)"', rest))
    return found


def _cargo(locked: bool, features: str, args: list[str]) -> str:
    cmd = ["cargo", "test"]
    if locked:
        cmd.append("--locked")
    if features:
        cmd.extend(["--features", features])
    cmd.extend(args)
    return " ".join(cmd)


def plan_crates(plan: Plan) -> list[str]:
    return sorted(plan.full | set(plan.lib_filters) | set(plan.integration))


def integration_targets(crate: str) -> list[str]:
    tests = CRATES / crate / "tests"
    return sorted(p.stem for p in tests.glob("*.rs")) if tests.is_dir() else []


FENCE = re.compile(r"^\s*//[/!]\s*```\s*([A-Za-z0-9_,\-]*)")
# Fenced blocks rustdoc does not compile as tests.
NON_RUST_FENCES = {"text", "sh", "bash", "console", "toml", "json", "yaml", "plain", "ignore"}


def has_doctests(crate: str) -> bool:
    """True when a doc comment under `src/` opens a Rust code fence."""
    src = CRATES / crate / "src"
    if not has_lib_target(crate):
        return False
    for rs in src.rglob("*.rs"):
        inside = False
        for line in rs.read_text(encoding="utf-8", errors="replace").splitlines():
            match = FENCE.match(line)
            if not match:
                continue
            if inside:
                inside = False
                continue
            inside = True
            langs = {t for t in match.group(1).split(",") if t}
            if not langs & NON_RUST_FENCES:
                return True
    return False


def bundled_build(plan: Plan, *, locked: bool, features: str) -> list[str]:
    """One `cargo test --no-run` over every selected package and target.

    Features are qualified per package (`whycodes-tools/bundled-sqlite`) so
    one invocation can carry what each `-p` line would have passed.
    """
    deps = direct_deps()
    crates = plan_crates(plan)
    specs: list[str] = []
    flags: list[str] = []
    seen: set[tuple[str, ...]] = set()

    def flag(*args: str) -> None:
        if args not in seen:
            seen.add(args)
            flags.extend(args)

    for crate in crates:
        for spec in filter(None, features_for_crate(crate, features, deps).split(",")):
            qualified = spec if "/" in spec else f"{package_name(crate)}/{spec}"
            if qualified not in specs:
                specs.append(qualified)
        unit = crate in plan.full or crate in plan.lib_filters
        if unit and has_lib_target(crate):
            flag("--lib")
        if crate in plan.full or (unit and not has_lib_target(crate)):
            for name in bin_names(crate):
                flag("--bin", name)
        tests = integration_targets(crate) if crate in plan.full else sorted(plan.integration.get(crate, ()))
        for target in tests:
            flag("--test", target)
    cmd = ["cargo", "test", "--no-run", "--message-format=json-render-diagnostics"]
    if locked:
        cmd.append("--locked")
    if specs:
        cmd.extend(["--features", ",".join(specs)])
    for crate in crates:
        cmd.extend(["-p", package_name(crate)])
    return cmd + flags


def bundled_runs(plan: Plan, artifacts: list[dict]) -> list[tuple[str, str, list[str]]]:
    """`(crate, executable, harness args)` for each planned test binary."""
    exes: dict[str, list[tuple[str, str, str]]] = defaultdict(list)
    for msg in artifacts:
        if msg.get("reason") != "compiler-artifact" or not msg.get("executable"):
            continue
        if not msg.get("profile", {}).get("test"):
            continue
        crate = Path(msg["manifest_path"]).parent.name
        kinds = msg["target"]["kind"]
        kind = "test" if "test" in kinds else "bin" if "bin" in kinds else "lib"
        exes[crate].append((kind, msg["target"]["name"], msg["executable"]))

    runs: list[tuple[str, str, list[str]]] = []
    for crate in plan_crates(plan):
        built = sorted(exes.get(crate, []))
        if crate in plan.full:
            runs.extend((crate, exe, []) for _, _, exe in built)
            continue
        if crate in plan.lib_filters:
            filters = plan.lib_filters[crate]
            args = [] if "" in filters else sorted(filters)
            unit = "lib" if has_lib_target(crate) else "bin"
            runs.extend((crate, exe, args) for kind, _, exe in built if kind == unit)
        wanted = plan.integration.get(crate, set())
        runs.extend((crate, exe, []) for kind, name, exe in built if kind == "test" and name in wanted)
    return runs


def run_bundled(plan: Plan, *, locked: bool, features: str) -> int:
    cmd = bundled_build(plan, locked=locked, features=features)
    print("build: " + " ".join(cmd), flush=True)
    built = subprocess.run(cmd, cwd=ROOT, stdout=subprocess.PIPE, text=True, check=False)
    if built.returncode != 0:
        return built.returncode
    artifacts = [json.loads(line) for line in built.stdout.splitlines() if line.startswith("{")]
    failed: list[str] = []
    for crate, exe, args in bundled_runs(plan, artifacts):
        label = f"{package_name(crate)} {Path(exe).name} {' '.join(args)}".strip()
        print(f"\n== {label}", flush=True)
        env = dict(os.environ, CARGO_MANIFEST_DIR=str(CRATES / crate))
        if subprocess.run([exe, *args], cwd=CRATES / crate, env=env, check=False).returncode:
            failed.append(label)
    deps = direct_deps()
    for crate in sorted(plan.full):
        if has_doctests(crate):
            doc = _cargo(locked, features_for_crate(crate, features, deps), ["-p", package_name(crate), "--doc"])
            print(f"\n== {doc}", flush=True)
            if subprocess.run(doc, cwd=ROOT, shell=True, check=False).returncode:
                failed.append(doc)
    if failed:
        print("\nfailed:\n  " + "\n  ".join(failed), file=sys.stderr)
        return 1
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--paths", nargs="*", help="changed files (repo-relative)")
    parser.add_argument("--stdin", action="store_true", help="read paths from stdin")
    parser.add_argument("--base", default="origin/main", help="diff base when no paths are given")
    parser.add_argument("--execute", action="store_true", help="run the selected cargo commands")
    parser.add_argument("--locked", action="store_true", help="pass --locked (CI)")
    parser.add_argument(
        "--features",
        default="",
        help=(
            "cargo --features value (CI passes whycodes-storage/bundled). "
            "On -p lines the spec is kept, rewritten to a crate alias, or "
            "dropped when that package cannot activate it"
        ),
    )
    parser.add_argument("--quiet", action="store_true", help="print commands only, not the reasons")
    args = parser.parse_args(argv)

    paths = changed_files(args)
    graph = direct_deps()
    # Only a real base can say what the lockfile bumped; --paths/--stdin and
    # a push (base HEAD) keep Cargo.lock a workspace root.
    lock_bumps = None
    if "Cargo.lock" in paths and not (args.paths or args.stdin) and args.base != "HEAD":
        lock_bumps = _lock_bumps(args.base)
    plan = plan_for(paths, graph, lock_bumps)
    lines = cargo_lines(plan, locked=args.locked, features=args.features)

    if not args.quiet:
        if paths:
            print(f"changed: {len(paths)} file(s)")
        else:
            print("changed: none")
        for reason in plan.reasons:
            print(f"  {reason}")
        if not lines:
            print("no rust tests selected")
        else:
            print("run:")

    for line in lines:
        print(line if args.quiet else f"  {line}")

    if not args.execute:
        return 0
    if not plan.workspace and len(plan_crates(plan)) > 1:
        return run_bundled(plan, locked=args.locked, features=args.features)
    for line in lines:
        result = subprocess.run(line, cwd=ROOT, shell=True)
        if result.returncode != 0:
            return result.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
