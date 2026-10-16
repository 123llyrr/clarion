#!/usr/bin/env python3
"""Reject dependency edges that would merge Clarion's independent modules."""
from pathlib import Path
import re
import sys
import tomllib

PURE_DEPENDENCIES = {
    "clarion-core": {"smallvec", "clarion-motion"},
    "clarion-motion": set(),
    "clarion-timing": set(),
    "clarion-scene": set(),
}


def production(source):
    # Backend fixture modules live at the end of each source file. Their State
    # access is deliberate integration-test setup, not a production capability.
    source = re.split(r"(?m)^#\[cfg\(test\)\]\s*\nmod\s+", source, maxsplit=1)[0]
    source = re.sub(r"/\*.*?\*/", "", source, flags=re.S)
    source = re.sub(r"//[^\n]*", "", source)
    return re.sub(r'"(?:\\.|[^"\\])*"', '""', source)


def check(root):
    errors = []
    workspace = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["members"]
    for name, permitted in PURE_DEPENDENCIES.items():
        if name not in workspace:
            errors.append(f"{name}: absent from the production workspace")
        manifest = tomllib.loads((root / name / "Cargo.toml").read_text())
        dependencies = set(manifest.get("dependencies", {}))
        if dependencies - permitted:
            errors.append(f"{name}: forbidden dependencies {sorted(dependencies - permitted)}")
    forbidden = re.compile(r"\bcrate::(?:compositor|layout|input)\b")
    for path in sorted((root / "src/backend").rglob("*.rs")):
        if forbidden.search(production(path.read_text())):
            errors.append(f"{path.relative_to(root)}: device backend accesses desktop state")
    full_owner = re.compile(r"\b(?:crate::compositor|Clarion|OutputState|LoopHandle|EventLoop)\b|\b(?:Layout|Monitor|Workspace|DesktopCore)\b(?!\s*=)")
    for path in sorted((root / "src/scene").rglob("*.rs")):
        if full_owner.search(production(path.read_text())):
            errors.append(f"{path.relative_to(root)}: scene owns an unrestricted platform capability")
    return errors


if __name__ == "__main__":
    issues = check(Path(__file__).resolve().parents[1])
    for issue in issues:
        print(issue, file=sys.stderr)
    if issues:
        raise SystemExit(1)
    print("Clarion module dependency boundaries passed.")
