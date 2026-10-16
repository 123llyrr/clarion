import tempfile
import unittest
from pathlib import Path

from check_module_boundaries import check


class ModuleBoundaries(unittest.TestCase):
    def fixture(self, root):
        (root / "Cargo.toml").write_text('[workspace]\nmembers = ["clarion-core", "clarion-motion", "clarion-timing", "clarion-scene"]\n')
        for name in ("clarion-core", "clarion-motion", "clarion-timing", "clarion-scene"):
            path = root / name
            path.mkdir()
            (path / "Cargo.toml").write_text('[package]\nname = "' + name + '"\n[dependencies]\n')
        (root / "src/backend").mkdir(parents=True)
        (root / "src/scene").mkdir()

    def test_backend_cannot_reacquire_desktop_but_test_fixture_can(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/backend/test.rs"
            source.write_text('use crate::compositor::Clarion;\n')
            self.assertTrue(check(root))
            source.write_text('#[cfg(test)]\nmod tests { use crate::compositor::Clarion; }\n')
            self.assertEqual(check(root), [])

    def test_pure_crate_cannot_gain_device_dependency(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            path = root / "clarion-timing/Cargo.toml"
            path.write_text(path.read_text() + 'smithay = "0.4"\n')
            self.assertTrue(check(root))

    def test_scene_cannot_retain_full_owner_or_event_loop(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/scene/test.rs"
            for capability in ("Clarion", "OutputState", "LoopHandle", "Layout", "Monitor", "Workspace", "DesktopCore"):
                source.write_text('struct Scene { owner: ' + capability + ' }\n')
                self.assertTrue(check(root), capability)
            source.write_text('// Clarion is intentionally absent.\nstruct Scene { locked: bool }\nfn trace() { log!("Clarion::render"); }\nclarion_render_elements! { Elements<R> => { Monitor = MonitorRenderElement<R>, } }\n')
            self.assertEqual(check(root), [])
            source.write_text("use crate::layout::scene_source::{OutputSceneSource, WindowLookup};\nstruct Scene<'a> { sources: OutputSceneSource<'a, Mapped>, windows: &'a dyn WindowLookup<Mapped> }\n")
            self.assertEqual(check(root), [])


if __name__ == "__main__":
    unittest.main()
