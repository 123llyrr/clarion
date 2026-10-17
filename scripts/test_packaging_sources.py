"""Check the actual Nix source filter against Cargo's local build inputs.

This does not evaluate Nix or claim a successful Nix build. It catches missing
workspace crates and embedded resources before an isolated build loses them.
"""
from pathlib import Path
import re
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[1]


class NixSourceClosureTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
        source = (ROOT / "flake.nix").read_text()
        fileset = re.search(r"fileset\s*=\s*lib\.fileset\.unions\s*\[(.*?)\];", source, re.S)
        if fileset is None:
            raise AssertionError("Update source-closure checks for the new Nix source filter")
        literals = re.sub(r"#[^\n]*", "", fileset[1]).split()
        if not all(re.fullmatch(r"\./[\w./-]+", literal) for literal in literals):
            raise AssertionError("Nix source-closure check requires explicit source paths")
        cls.sources = [(ROOT / literal).resolve() for literal in literals]
        cls.members = [ROOT]
        for pattern in cls.manifest["workspace"]["members"]:
            members = list(ROOT.glob(pattern))
            if not members:
                raise AssertionError(f"Workspace member does not exist: {pattern}")
            cls.members.extend(members)

    def assert_in_source(self, path):
        path = path.resolve()
        self.assertTrue(
            any(path == source or path.is_relative_to(source) for source in self.sources),
            f"Nix source filter omits {path.relative_to(ROOT)}",
        )

    def test_nix_source_contains_every_workspace_crate(self):
        for member in self.members:
            with self.subTest(member=member.relative_to(ROOT)):
                self.assertTrue((member / "Cargo.toml").is_file())
                self.assert_in_source(member / "Cargo.toml")
                self.assert_in_source(member / "src")

    def test_nix_source_contains_local_path_dependencies_and_patches(self):
        def local_paths(value, in_dependencies=False):
            if isinstance(value, dict):
                for key, child in value.items():
                    if in_dependencies and key == "path" and isinstance(child, str):
                        yield child
                    else:
                        dependency_table = key in {
                            "dependencies", "dev-dependencies", "build-dependencies",
                            "patch", "replace",
                        }
                        yield from local_paths(child, in_dependencies or dependency_table)

        for member in self.members:
            manifest = tomllib.loads((member / "Cargo.toml").read_text())
            for relative in local_paths(manifest):
                dependency = (member / relative).resolve()
                with self.subTest(member=member.relative_to(ROOT), dependency=relative):
                    self.assertTrue((dependency / "Cargo.toml").is_file())
                    self.assert_in_source(dependency / "Cargo.toml")
                    self.assert_in_source(dependency / "src")

    def test_nix_source_preserves_literal_embedded_resources(self):
        for member in self.members:
            sources = list((member / "src").rglob("*.rs"))
            if (member / "build.rs").is_file():
                sources.append(member / "build.rs")
            for source in sources:
                for relative in re.findall(r'include_(?:bytes|str)!\s*\(\s*"([^"\n]+)"', source.read_text()):
                    with self.subTest(source=source.relative_to(ROOT), resource=relative):
                        resource = source.parent / relative
                        self.assertTrue(resource.is_file(), str(resource))
                        self.assert_in_source(resource)


if __name__ == "__main__":
    unittest.main()
