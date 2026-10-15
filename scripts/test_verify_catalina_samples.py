import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "verify_catalina_samples", Path(__file__).with_name("verify-catalina-samples.py")
)
verify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)


class SampleIntegrityTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.sample = self.root / "Dock"
        self.sample.write_bytes(b"recovered sample")
        self.entry = {
            "id": "dock",
            "path": "Dock",
            "size": self.sample.stat().st_size,
            "sha256": hashlib.sha256(self.sample.read_bytes()).hexdigest(),
        }
        self.manifest = self.root / "samples.json"
        self.write_manifest([self.entry])

    def write_manifest(self, samples):
        self.manifest.write_text(json.dumps({"version": 1, "samples": samples}))

    def test_relative_sample_identity_and_same_size_corruption(self):
        self.assertTrue(verify.verify_manifest(self.manifest)["verified"])
        self.sample.write_bytes(b"Recovered sample")
        report = verify.verify_manifest(self.manifest)
        self.assertFalse(report["verified"])
        self.assertEqual(report["samples"][0]["status"], "mismatch")

    def test_missing_sample_is_an_explicit_failure(self):
        self.sample.unlink()
        report = verify.verify_manifest(self.manifest)
        self.assertFalse(report["verified"])
        self.assertEqual(report["samples"][0]["status"], "unavailable")

    def test_empty_or_duplicate_manifest_cannot_pass(self):
        for samples in ([], [self.entry, self.entry]):
            self.write_manifest(samples)
            with self.assertRaises(ValueError):
                verify.verify_manifest(self.manifest)

    def test_output_cannot_overwrite_an_input(self):
        original = self.sample.read_bytes()
        self.assertEqual(
            verify.main([str(self.manifest), "--output", str(self.sample)]), 2
        )
        self.assertEqual(self.sample.read_bytes(), original)
        self.assertEqual(
            verify.main([str(self.manifest), "--output", str(self.manifest)]), 2
        )
        self.assertTrue(verify.verify_manifest(self.manifest)["verified"])

    def test_hard_link_output_cannot_overwrite_an_input(self):
        for source in (self.sample, self.manifest):
            alias = self.root / "report.json"
            alias.hardlink_to(source)
            original = source.read_bytes()
            self.assertEqual(
                verify.main([str(self.manifest), "--output", str(alias)]), 2
            )
            self.assertEqual(source.read_bytes(), original)
            alias.unlink()

    def test_sample_path_replacement_during_hashing_fails(self):
        original_digest = hashlib.sha256
        replacement = self.root / "different-Dock"
        replacement.write_bytes(b"Recovered sample")
        link = self.root / "sample-link"
        link.symlink_to(self.sample)
        self.entry["path"] = link.name
        self.write_manifest([self.entry])

        class SwappingDigest:
            def __init__(inner):
                inner.digest = original_digest()
            def update(inner, chunk):
                inner.digest.update(chunk)
                link.unlink()
                link.symlink_to(replacement)
            def hexdigest(inner):
                return inner.digest.hexdigest()

        with patch.object(verify.hashlib, "sha256", SwappingDigest):
            report = verify.verify_manifest(self.manifest)
        self.assertFalse(report["verified"])
        self.assertEqual(report["samples"][0]["status"], "unavailable")
        self.assertIn("path changed", report["samples"][0]["error"])

    def test_report_is_saved_without_modifying_samples(self):
        output = self.root / "report.json"
        with patch("sys.stdout"):
            self.assertEqual(
                verify.main([str(self.manifest), "--output", str(output)]), 0
            )
        self.assertTrue(json.loads(output.read_text())["verified"])
        self.assertTrue(verify.verify_manifest(self.manifest)["verified"])


if __name__ == "__main__":
    unittest.main()
