#!/usr/bin/env python3
"""Check that every Rust integration test file belongs to exactly one test binary.

rs/read/Cargo.toml sets `autotests = false`, so Cargo builds only the [[test]]
targets it lists. A test file that no target names is silently never compiled.
Each file in rs/read/tests/*.rs must be exactly one of:

- a group file (group_*.rs) that is itself a [[test]] target;
- a module of exactly one group file, declared as
  `#[path = "<name>.rs"] pub mod <name>;` so runners can select it with `<name>::`;
- its own [[test]] target.

Helper modules in subdirectories (support/, phase4_races/) are not test files.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PACKAGE = ROOT / "rs" / "read"
TESTS = PACKAGE / "tests"
GROUP_PREFIX = "group_"

# One [[test]] table: its name and an optional explicit path.
_TEST_TABLE = re.compile(r"^\[\[test\]\]\n((?:[a-z-]+ = [^\n]*\n)*)", re.MULTILINE)
_TABLE_KEY = re.compile(r'^([a-z-]+) = "([^"]*)"$', re.MULTILINE)
_AUTOTESTS_OFF = re.compile(r"^autotests = false$", re.MULTILINE)
# A group module: the path attribute, optional attributes, then the module.
_GROUP_MODULE = re.compile(
    r'^#\[path = "([^"]+)"\]\n(?:#\[[^\n]*\]\n)*pub mod ([a-z0-9_]+);$', re.MULTILINE
)
_ANY_MODULE = re.compile(r"^\s*(?:pub )?mod [a-z0-9_]+;", re.MULTILINE)


def target_files(manifest: str) -> dict[str, list[str]]:
    """Map each test file name to the [[test]] targets that build it."""
    owners: dict[str, list[str]] = {}
    for table in _TEST_TABLE.finditer(manifest):
        keys = dict(_TABLE_KEY.findall(table.group(1)))
        name = keys["name"]
        path = keys.get("path", f"tests/{name}.rs")
        if not path.startswith("tests/") or "/" in path[len("tests/") :]:
            raise SystemExit(f"[[test]] {name}: path {path} is not a file in tests/")
        owners.setdefault(path[len("tests/") :], []).append(f"target {name}")
    return owners


def group_modules(group: Path, problems: list[str]) -> list[str]:
    """Return the test files that one group file includes as modules."""
    text = group.read_text()
    included = []
    for path, module in _GROUP_MODULE.findall(text):
        if path != f"{module}.rs":
            problems.append(f"{group.name}: module {module} must load {module}.rs, not {path}")
        included.append(path)
    if len(_ANY_MODULE.findall(text)) != len(included):
        problems.append(f"{group.name}: every module needs #[path = \"<name>.rs\"] pub mod")
    return included


def main() -> int:
    manifest = (PACKAGE / "Cargo.toml").read_text()
    problems: list[str] = []
    if not _AUTOTESTS_OFF.search(manifest):
        problems.append("rs/read/Cargo.toml must set autotests = false")

    owners = target_files(manifest)
    test_files = sorted(path.name for path in TESTS.glob("*.rs"))
    groups = [name for name in test_files if name.startswith(GROUP_PREFIX)]
    for group in groups:
        if f"target {group[:-3]}" not in owners.get(group, []):
            problems.append(f"{group} is not a [[test]] target with the same name")
        for included in group_modules(TESTS / group, problems):
            owners.setdefault(included, []).append(f"group {group[:-3]}")

    for name in sorted(owners):
        if name not in test_files:
            problems.append(f"{name} is named by {', '.join(owners[name])} but does not exist")
    for name in test_files:
        found = owners.get(name, [])
        if not found:
            problems.append(f"{name} is in no test binary; add it to a group or a [[test]] target")
        elif len(found) > 1:
            problems.append(f"{name} is in more than one test binary: {', '.join(found)}")

    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        return 1
    separate = len(test_files) - len(groups) - sum(
        1 for found in owners.values() if found[0].startswith("group ")
    )
    print(
        f"{len(test_files)} test files: {len(groups)} group binaries, "
        f"{separate} files in their own binary"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
