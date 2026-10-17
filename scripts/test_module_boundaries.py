import tempfile
import unittest
from pathlib import Path

from check_module_boundaries import check, NUMERIC_WINDOW_MODELS, TILE_SCENE_COMPATIBILITY


class ModuleBoundaries(unittest.TestCase):
    def fixture(self, root):
        (root / "Cargo.toml").write_text('[workspace]\nmembers = ["clarion-core", "clarion-motion", "clarion-timing", "clarion-scene"]\n')
        for name in ("clarion-core", "clarion-motion", "clarion-timing", "clarion-scene"):
            path = root / name
            path.mkdir()
            (path / "Cargo.toml").write_text('[package]\nname = "' + name + '"\n[dependencies]\n')
        (root / "src/backend").mkdir(parents=True)
        (root / "src/scene").mkdir()
        (root / "src/layout").mkdir()

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

    def test_dependency_alias_cannot_impersonate_a_permitted_pure_dependency(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            path = root / "clarion-core/Cargo.toml"
            path.write_text(path.read_text() + 'smallvec = { package = "smithay", version = "0.4" }\n')
            self.assertTrue(check(root))

    def test_pure_crate_cannot_hide_device_dependency_in_build_or_target_table(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            path = root / "clarion-timing/Cargo.toml"
            original = path.read_text()
            for table in ("build-dependencies", "target.'cfg(unix)'.dependencies",
                          "target.'cfg(unix)'.build-dependencies"):
                with self.subTest(table=table):
                    path.write_text(original + f'\n[{table}]\nsmithay = "0.4"\n')
                    self.assertTrue(check(root))
            path.write_text(original + '\n[dev-dependencies]\nsmithay = "0.4"\n')
            self.assertEqual(check(root), [])

    def test_fixture_module_cannot_hide_later_production_owner(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/scene/test.rs"
            fixture = '#[cfg(test)]\nmod tests { fn helper() { let message = "}"; let brace = \'{\'; } }\n'
            source.write_text(fixture + 'struct Scene { owner: Clarion }\n')
            self.assertTrue(check(root))
            source.write_text(fixture + 'struct Scene { locked: bool }\n')
            self.assertEqual(check(root), [])

    def test_test_only_fields_and_impls_do_not_hide_production_fields(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/scene/test.rs"
            source.write_text('struct Scene {\n#[cfg(test)]\nowner: Option<Clarion>,\n'
                              'locked: bool,\n}\n#[cfg(test)]\nimpl Scene {\n'
                              'fn new() -> Self { panic!("fixture") }\n}\n')
            self.assertEqual(check(root), [])
            source.write_text(source.read_text() + 'struct Leaked { owner: Clarion }\n')
            self.assertTrue(check(root))

    def test_quoted_or_commented_owner_names_are_not_capabilities(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/scene/test.rs"
            source.write_text('const INFO: &str = r###"Clarion { Layout } //"###;\n'
                              '// owner: DesktopCore\n/* owner: EventLoop */\n')
            self.assertEqual(check(root), [])

    def test_layout_can_borrow_resources_but_cannot_retain_the_registry(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/layout/tile.rs"
            source.write_text('struct Tile { resources: WindowResourceHandle }\n'
                              'struct Monitor { resources: ResourceHandle<MonitorResources>, scene: SceneResourceAllocator }\n'
                              'struct Workspace { resources: WindowResourceAllocator,\n'
                              '#[cfg(test)]\ntest_owner: Option<WindowResourceRegistry>,\n}\n'
                              '#[cfg(test)]\nimpl Workspace { fn new() { WindowResourceRegistry::new(); } }\n')
            self.assertEqual(check(root), [])
            source.write_text(source.read_text() + 'fn production() { WindowResourceRegistry::new(); }\n')
            self.assertTrue(check(root))
            source.write_text('struct Monitor { resources: ResourceRegistry<OutputCache> }\n')
            self.assertTrue(check(root))
            source.write_text('struct Layout { resources: SceneResources }\n')
            self.assertTrue(check(root))

    def test_external_fixture_is_exempt_only_when_module_is_test_conditional(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            parent = root / "src/layout/mod.rs"
            fixture = root / "src/layout/tests.rs"
            production = root / "src/layout/production.rs"
            parent.write_text('#[cfg(test)]\nmod tests;\nmod production;\n')
            fixture.write_text('struct Fixture { owner: SceneResources }\n')
            production.write_text('struct Layout { owner: SceneResourceAllocator }\n')
            self.assertEqual(check(root), [])
            production.write_text('struct Layout { owner: SceneResources }\n')
            self.assertTrue(check(root), 'a fixture declaration cannot exempt its later sibling')
            production.write_text('struct Layout { owner: SceneResourceAllocator }\n')
            parent.write_text('mod tests;\nmod production;\n')
            self.assertTrue(check(root), 'the name tests alone never exempts production code')

    def test_window_layout_records_cannot_retain_gpu_caches_or_strong_leases(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/layout/tile.rs"
            for record in ("Tile", "OpenAnimation", "ResizeAnimation", "AlphaAnimation", "ClosingWindow", "Monitor", "Mapped"):
                for field in ("cache: RefCell<Option<NativeOffscreenBuffer>>",
                              "image: Option<ResizeSnapshot>", "resources: WindowResourceLease",
                              "lease: ResourceLease<ClosingImages>",
                              "header: RefCell<Option<TextTextureCache>>"):
                    with self.subTest(record=record, field=field):
                        source.write_text(f'struct {record} {{ {field} }}\n')
                        self.assertTrue(check(root))
            source.write_text('struct Tile { resources: WindowResourceHandle }\n'
                              'impl Tile { fn snapshot(&self, image: ResizeSnapshot) -> ResizeSnapshot { image } }\n')
            self.assertEqual(check(root), [])

    def test_tile_cannot_retain_decorations_effect_cache_or_aliases(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/layout/tile.rs"
            source.write_text("struct Tile { presentation: WindowPresentationState, resources: WindowResourceHandle }")
            self.assertEqual(check(root), [])
            for storage in ("FocusRing", "Shadow", "SolidColorBuffer", "WindowDecorations",
                            "RoundedCornerDamage", "WobbleMesh", "WobbleForecast", "GenieCache",
                            "NativeOffscreenBuffer", "VulkanTexture", "WindowRenderResources", "ResourceLease"):
                for declaration in (
                        f"struct Tile {{ retained: {storage} }}",
                        f"use crate::scene::{storage} as Hidden; struct Tile {{ retained: Hidden }}",
                        f"type Hidden = {storage}; type Other = Hidden; struct Tile {{ retained: Other }}"):
                    with self.subTest(storage=storage, declaration=declaration):
                        source.write_text(declaration)
                        self.assertTrue(check(root))
            for field in ("border", "focus_ring", "shadow", "fullscreen_backdrop", "mesh_cache"):
                source.write_text(f"struct Tile {{ {field}: Hidden }}")
                self.assertTrue(check(root))

    def test_numeric_window_models_reject_renderers_caches_and_scene_imports(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            accepted = "struct Pose { alpha: f64, clock: Clock, border_config: clarion_config::FocusRing }"
            for relative in NUMERIC_WINDOW_MODELS:
                source = root / relative
                source.write_text(accepted)
                self.assertEqual(check(root), [])
                for leak in (
                        "use crate::render_helpers::renderer::NativeRenderer as Hidden;",
                        "use smithay::backend::renderer::utils::CommitCounter;",
                        "use crate::scene::wobble::WobbleForecast;",
                        "use ash::vk as Hidden;",
                        "struct WindowPresentationState { image: ResizeSnapshot }",
                        "struct OpenAnimation { cache: GenieCache }",
                        "struct Model { pixels: SolidColorBuffer }",
                        "struct Model { pixels: GbmBuffer }",
                        "struct Model { image: RenderSnapshot }",
                        "fn advance(gpu: &mut NativeRenderer) {}"):
                    with self.subTest(relative=relative, leak=leak):
                        source.write_text(accepted + leak)
                        self.assertTrue(check(root))
                source.write_text(accepted)

    def test_numeric_window_model_test_fixture_does_not_exempt_later_production(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            for relative in NUMERIC_WINDOW_MODELS:
                source = root / relative
                accepted = ('#[cfg(test)] mod tests { use crate::scene::genie::GenieCache; '
                            'fn draw(renderer: &mut NativeRenderer) {} }\nstruct Pose { value: f64 }')
                source.write_text(accepted)
                self.assertEqual(check(root), [])
                source.write_text(accepted + "\nuse crate::render_helpers::snapshot::RenderSnapshot;")
                self.assertTrue(check(root))
                source.write_text(accepted)

    def test_tile_compatibility_entries_only_project_or_delegate(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/layout/tile.rs"
            for method, delegate in TILE_SCENE_COMPATIBILITY.items():
                accepted = f"impl Tile {{ fn {method}(&self) {{ {delegate} }} }}"
                source.write_text(accepted)
                self.assertEqual(check(root), [], method)
                for work in ("for pass in 0..3 { renderer.draw(pass); } ",
                             "let image = render_to_encompassing_texture(renderer, elements); ",
                             "self.window.render_normal(ctx, push); "):
                    with self.subTest(method=method, work=work):
                        source.write_text(accepted.replace(delegate, work + delegate))
                        self.assertTrue(check(root))
            source.write_text("impl Tile { fn render_inner(&self) { renderer.draw(); } }")
            self.assertTrue(check(root))

    def test_window_scene_and_snapshot_cannot_reacquire_a_layout_tile(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            for relative in ("src/scene/tile.rs", "src/scene/window_snapshot.rs", "src/scene/opening_window.rs",
                             "src/scene/resize_snapshot.rs"):
                source = root / relative
                accepted = "struct Pose { progress: f64 }\nfn compose(scene: &TileScene<W>, pose: OpeningWindowPose) {}"
                source.write_text(accepted)
                self.assertEqual(check(root), [])
                for owner in ("Tile", "Monitor", "Workspace", "Layout", "DesktopCore"):
                    for leak in (f"use crate::layout::{owner} as Hidden;",
                                 f"fn capture(owner: &{owner}<W>) {{}}",
                                 f"struct Hidden {{ owner: &'a {owner}<W> }}"):
                        with self.subTest(relative=relative, leak=leak):
                            source.write_text(accepted + leak)
                            self.assertTrue(check(root))
                if relative != "src/scene/tile.rs":
                    for model in ("Animation", "Clock", "OpenAnimation", "ResizeAnimation", "AlphaAnimation", "Genie"):
                        source.write_text(accepted + f"\nfn sample(model: &{model}) {{}}")
                        self.assertTrue(check(root))
                source.write_text(accepted)

    def test_tile_scene_projection_cannot_store_a_callback(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/scene/tile.rs"
            accepted = "struct TileScene<'a, W> { window: &'a W, state: &'a WindowPresentationState, resources: WindowResourceLease }"
            source.write_text(accepted)
            self.assertEqual(check(root), [])
            for leak in ("callback: Box<dyn FnMut(Context, Sink)>",
                         "callback: &'a dyn FnMut(Context, Sink)",
                         "callback: fn(Context, Sink)", "callback: impl Fn(Context)"):
                source.write_text(accepted.replace("resources:", leak + ", resources:"))
                self.assertTrue(check(root), leak)
            source.write_text("type Callback = fn(Context, Sink); type Hidden = Callback; "
                              + accepted.replace("resources:", "callback: Hidden, resources:"))
            self.assertTrue(check(root))

    def test_tile_scene_reads_numeric_effects_and_only_mutates_its_resource_caches(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/scene/tile.rs"
            accepted = """
                impl TileScene {
                    fn render(&self) {
                        let active = self.wobble.borrow().is_active();
                        let pose = self.state.genie.borrow().as_ref().map(|genie| genie.pose());
                        self.resources.wobble_forecast.borrow_mut().sample(active);
                        self.resources.genie_mesh.borrow_mut().clear();
                        self.resources.request_effect_retirement(request);
                    }
                }
            """
            source.write_text(accepted)
            self.assertEqual(check(root), [])
            for mutation in (
                    "self.wobble.borrow_mut().cancel();",
                    "self.state.wobble.try_borrow_mut().unwrap().cancel();",
                    "self.genie.borrow_mut().take();",
                    "self.state.genie.replace(None);",
                    "self.genie.take();",
                    "self.wobble.get_mut().cancel();",
                    "RefCell::borrow_mut(&self.wobble).cancel();",
                    "let model = &self.genie; model.borrow_mut().take();"):
                with self.subTest(mutation=mutation):
                    source.write_text(accepted + f"fn corrupt(&self) {{ {mutation} }}")
                    self.assertTrue(check(root))
            source.write_text(accepted + '#[cfg(test)] mod tests { fn owner() { self.genie.borrow_mut().take(); } }')
            self.assertEqual(check(root), [])
            # The numeric simulation copy and mesh forecast are owned here;
            # this guard must not ban mutation in unrelated scene modules.
            (root / "src/scene/wobble.rs").write_text("fn forecast(&self) { self.wobble.borrow_mut().advance(); }")
            self.assertEqual(check(root), [])

    def test_captured_window_sources_retain_projection_instead_of_tile_or_callback(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/layout/scene_source.rs"
            for record, field in (("TileRenderSource", "scene"), ("MovingTileScene", "tile")):
                accepted = self.captured_scene_fixture() + f"\nstruct {record}<'a, W> {{ {field}: crate::scene::tile::TileScene<'a, W>, zoom: f64 }}"
                source.write_text(accepted)
                self.assertEqual(check(root), [])
                for capability in ("&'a Tile<W>", "&'a Monitor<W>", "&'a Workspace<W>",
                                   "Box<dyn FnMut(Context, Sink)>", "fn(Context, Sink)"):
                    changed = accepted.replace("crate::scene::tile::TileScene<'a, W>", capability)
                    self.assertNotEqual(changed, accepted)
                    source.write_text(changed)
                    self.assertTrue(check(root))
                source.write_text(accepted)

    def test_resource_registration_and_allocation_are_weak(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/scene/window_resources.rs"
            allocator = 'struct WindowResourceAllocator { owner: Weak<RegistryInner> }\n'
            handle = 'struct WindowResourceHandle { owner: Weak<RegistryInner>, key: WindowResourceKey }\n'
            source.write_text(allocator + handle)
            self.assertEqual(check(root), [])
            source.write_text(allocator + handle.replace('Weak<RegistryInner>', 'Rc<RegistryInner>'))
            self.assertTrue(check(root))
            source.write_text(allocator.replace('owner:', 'strong: Rc<WindowRenderResources>, owner:') + handle)
            self.assertTrue(check(root))

    def test_generic_resource_registration_cannot_own_its_arena(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/scene/retained.rs"
            allocator = 'struct ResourceAllocator<T> { owner: Weak<RegistryInner<T>> }\n'
            handle = 'struct ResourceHandle<T> { owner: Weak<RegistryInner<T>>, key: u64 }\n'
            source.write_text(allocator + handle)
            self.assertEqual(check(root), [])
            source.write_text(allocator + handle.replace('Weak<RegistryInner<T>>', 'Rc<RegistryInner<T>>'))
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


    def captured_scene_fixture(self):
        return """
            enum Presentation<'a, W> {
                Ordinary(Composition<'a, W>),
                Stage(StageComposition<'a, W>),
                Spread(SpreadComposition<'a, W>),
                Cube(CubeComposition<'a, W>),
            }
            struct MonitorScene<'a, W> {
                presentation: Presentation<'a, W>,
                cube_lifecycle: Option<CubeLifecycle<'a, W>>,
            }
            impl<W> MonitorScene<'_, W> {
                fn render_workspaces(&self, ctx: Context, focus_ring: bool, push: Sink) {
                    if let Some(lifecycle) = &self.cube_lifecycle { lifecycle.leave_cube(); }
                    match &self.presentation {
                        Presentation::Ordinary(composition) => composition.render(ctx, focus_ring, push),
                        Presentation::Stage(composition) => { composition.render(ctx, focus_ring, &mut |_, _, _, _, _| {}, push) }
                        Presentation::Spread(composition) => composition.render(ctx, push),
                        Presentation::Cube(composition) => composition.render(ctx, focus_ring, push),
                    }
                }
                fn render_stage(&self, ctx: Context, focus_ring: bool, layers: Layers, push: Sink) {
                    if let Presentation::Stage(stage) = &self.presentation {
                        if let Some(lifecycle) = &self.cube_lifecycle { lifecycle.leave_cube(); }
                        stage.render(ctx, focus_ring, layers, push);
                    }
                }
                fn render_shadows(&self, renderer: Renderer, push: Sink) {
                    match &self.presentation {
                        Presentation::Ordinary(composition) => composition.render_shadows(renderer, push),
                        Presentation::Spread(composition) => composition.render_shadows(renderer, push),
                        Presentation::Stage(_) | Presentation::Cube(_) => {}
                    }
                }
            }
        """

    def compatibility_fixture(self, relative):
        if relative == "src/layout/monitor.rs":
            return """
                impl Monitor {
                    fn render_workspaces(&self) {
                        if self.is_stage_visible() {
                            self.cube_lifecycle().leave_cube();
                            self.render_stage(ctx, focus_ring, push);
                        } else if self.is_spread_visible() {
                            self.cube_lifecycle().leave_cube();
                            self.render_spread(ctx, push);
                        } else if self.is_cube_scene() {
                            self.cube_composition().render(ctx, focus_ring, push);
                        } else {
                            self.cube_lifecycle().leave_cube();
                            self.standard_composition().render(ctx, focus_ring, push);
                        }
                    }
                    fn render_workspace_shadows(&self) {
                        if self.is_stage_visible() { return; }
                        if self.is_spread_visible() {
                            self.spread_composition().render_shadows(renderer, push);
                        } else if !self.is_cube_scene() {
                            self.standard_composition().render_shadows(renderer, push);
                        }
                    }
                }
            """
        if relative == "src/layout/monitor/stage.rs":
            return """
                impl Monitor {
                    fn render_stage(&self) {
                        self.stage_composition().render(ctx, focus_ring, &mut |_, _, _, _, _| {}, push);
                    }
                    fn render_stage_with_layers(&self) {
                        self.stage_composition().render(ctx, focus_ring, layers, push);
                    }
                }
            """
        if relative == "src/layout/monitor/spread.rs":
            return "impl Monitor { fn render_spread(&self) { self.spread_composition().render(ctx, push); } }"
        raise AssertionError(relative)

    def write_compatibility_fixture(self, root, relative):
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(self.compatibility_fixture(relative))
        return path

    def test_all_four_presentations_use_captured_composition_and_narrow_lifecycle(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/layout/scene_source.rs"
            accepted = self.captured_scene_fixture()
            source.write_text(accepted)
            self.assertEqual(check(root), [])
            # Formatting, comments and trailing call/arm commas are not
            # capabilities and must not make this guard depend on rustfmt style.
            source.write_text(accepted.replace('composition.render(ctx, push)', '/* Monitor { old callback } */ composition.render(ctx, push)')
                              .replace('render(ctx, push)', 'render( ctx, push, )'))
            self.assertEqual(check(root), [])

    def test_each_presentation_rejects_owner_payload_even_when_other_arms_stay_captured(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/layout/scene_source.rs"
            for variant, capability in (("Ordinary", "Composition"), ("Stage", "StageComposition"),
                                        ("Spread", "SpreadComposition"), ("Cube", "CubeComposition")):
                for payload in ("(&'a Monitor<W>)", " { owner: &'a Workspace<W> }",
                                "(Box<dyn FnMut(Context, Sink)>)"):
                    with self.subTest(variant=variant, payload=payload):
                        accepted = self.captured_scene_fixture()
                        changed = accepted.replace(f"{variant}({capability}<'a, W>)",
                                                   variant + payload, 1)
                        self.assertNotEqual(changed, accepted)
                        source.write_text(changed)
                        self.assertTrue(check(root))

    def test_each_capture_is_inline_and_only_cube_exit_lifecycle_is_separate(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/layout/scene_source.rs"
            for capability in ("Composition", "StageComposition", "SpreadComposition", "CubeComposition"):
                with self.subTest(capability=capability):
                    accepted = self.captured_scene_fixture()
                    for storage in ("Box", "Option"):
                        changed = accepted.replace(capability + "<'a, W>)",
                                                   storage + "<" + capability + "<'a, W>>)", 1)
                        self.assertNotEqual(changed, accepted)
                        source.write_text(changed)
                        self.assertTrue(check(root))
            accepted = self.captured_scene_fixture()
            source.write_text(accepted.replace("Option<CubeLifecycle", "Box<CubeLifecycle", 1))
            self.assertTrue(check(root))
            for field, capability in (("composition", "Composition"), ("stage_composition", "StageComposition"),
                                      ("spread_composition", "SpreadComposition"), ("cube_composition", "CubeComposition")):
                with self.subTest(inactive=field):
                    source.write_text(accepted.replace("cube_lifecycle:",
                                                       f"{field}: Option<{capability}<'a, W>>, cube_lifecycle:", 1))
                    self.assertTrue(check(root))

    def test_scene_projection_cannot_hide_owner_or_callback_in_helper_record_or_alias(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/layout/scene_source.rs"
            for leak in (
                    "struct Hidden<'a> { owner: &'a Monitor<W> }",
                    "struct Hidden<'a> { owner: &'a Workspace<W> }",
                    "struct Hidden<'a> { callback: &'a dyn FnMut(Context, Sink) }",
                    "struct Hidden { callback: fn(Context, Sink) }",
                    "struct Hidden<'a> { callback: &'a StageLayers<'a, R> }",
                    "type Hidden<'a> = &'a Monitor<W>;",
                    "type Hidden = Box<dyn FnMut(Context, Sink)>;",
                    "type Hidden = dyn FnMut(Context, Sink);",
                    "type Hidden = fn(Context, Sink);",
                    "type Hidden = StageLayers<'static, R>;",
                    "enum Hidden<'a> { Owner(&'a Monitor<W>), Empty }",
                    "enum Hidden { Callback(Box<dyn FnMut(Context, Sink)>), Empty }",
                    "use super::Monitor as Hidden; struct Source { leaked: Hidden<W> }",
            ):
                with self.subTest(leak=leak):
                    source.write_text(self.captured_scene_fixture() + leak)
                    self.assertTrue(check(root))

    def test_each_render_arm_cannot_call_legacy_owner_or_replace_source_with_callback(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/layout/scene_source.rs"
            for variant in ("Ordinary", "Stage", "Spread", "Cube"):
                needle = f'Presentation::{variant}(composition) => '
                if variant == "Stage":
                    needle += "{ "
                needle += "composition"
                for redirect in ("monitor", "callback", "self.presentation"):
                    with self.subTest(variant=variant, redirect=redirect):
                        accepted = self.captured_scene_fixture()
                        changed = accepted.replace(needle, needle.removesuffix("composition") + redirect, 1)
                        self.assertNotEqual(changed, accepted)
                        source.write_text(changed)
                        self.assertTrue(check(root))
            for changed in (
                    self.captured_scene_fixture().replace('stage.render(ctx, focus_ring, layers, push);',
                                                         'monitor.render_stage_with_layers(ctx, focus_ring, layers, push);'),
                    self.captured_scene_fixture().replace('stage.render(ctx, focus_ring, layers, push);',
                                                         'callback(ctx, focus_ring, layers, push);'),
                    self.captured_scene_fixture().replace('composition.render_shadows(renderer, push)',
                                                         'monitor.render_workspace_shadows(renderer, push)'),
                    self.captured_scene_fixture().replace('composition.render_shadows(renderer, push)',
                                                         'callback(renderer, push)'),
            ):
                source.write_text(changed)
                self.assertTrue(check(root))

    def test_each_presentation_cannot_restore_a_drawing_loop_before_captured_render(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/layout/scene_source.rs"
            for variant, arguments in (
                    ("Ordinary", "ctx, focus_ring, push"),
                    ("Stage", "ctx, focus_ring, &mut |_, _, _, _, _| {}, push"),
                    ("Spread", "ctx, push"),
                    ("Cube", "ctx, focus_ring, push")):
                with self.subTest(variant=variant):
                    prefix = f"Presentation::{variant}(composition) => "
                    delegate = f'composition.render({arguments})'
                    accepted = self.captured_scene_fixture()
                    old = prefix + ("{ " if variant == "Stage" else "") + delegate
                    changed = accepted.replace(old, prefix + "{ for pass in 0..4 { monitor.old_draw(pass); } " + delegate + " }", 1)
                    self.assertNotEqual(changed, accepted)
                    source.write_text(changed)
                    self.assertTrue(check(root))

    def test_every_compatibility_entry_only_delegates_to_the_captured_scene(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            for relative in ("src/layout/monitor.rs", "src/layout/monitor/stage.rs", "src/layout/monitor/spread.rs"):
                with self.subTest(relative=relative):
                    source = self.write_compatibility_fixture(root, relative)
                    self.assertEqual(check(root), [])
                    accepted = source.read_text()
                    for corruption in ("for pass in 0..4 { old_draw(); } ",
                                       "let callback = |ctx| self.old_render(ctx); callback(ctx); ",
                                       "let old = self.workspaces_with_render_geo_cull(true); "):
                        source.write_text(accepted.replace("(&self) {", "(&self) {" + corruption, 1))
                        self.assertTrue(check(root))
                    source.write_text('#[cfg(test)] mod tests { fn render_workspaces() { old_draw(); } } ' + accepted)
                    self.assertEqual(check(root), [])
                    source.write_text(accepted)

    def test_old_specialized_drawing_helpers_cannot_remain_beside_new_delegate(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            for relative, legacy in (
                    ("src/layout/monitor.rs", "render_cube_workspaces"),
                    ("src/layout/monitor.rs", "render_cube_workspaces_native"),
                    ("src/layout/monitor.rs", "update_cube_visible_workspaces"),
                    ("src/layout/monitor/stage.rs", "render_stage_border"),
                    ("src/layout/monitor/stage.rs", "render_stage_rail")):
                with self.subTest(relative=relative, legacy=legacy):
                    source = self.write_compatibility_fixture(root, relative)
                    accepted = source.read_text()
                    source.write_text(accepted + f"fn {legacy}() {{ old_draw(); }}")
                    self.assertTrue(check(root))
                    source.write_text(accepted)

    def test_layout_projection_cannot_accept_or_recover_a_renderer_or_issue_gpu_work(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            for relative, method in (
                    ("src/layout/monitor.rs", "standard_composition"),
                    ("src/layout/monitor.rs", "composition_with_sources"),
                    ("src/layout/monitor.rs", "cube_composition"),
                    ("src/layout/monitor.rs", "cube_lifecycle"),
                    ("src/layout/monitor.rs", "cube_lifecycle_for"),
                    ("src/layout/monitor/stage.rs", "stage_composition"),
                    ("src/layout/monitor/spread.rs", "spread_composition")):
                with self.subTest(relative=relative, method=method):
                    source = self.write_compatibility_fixture(root, relative)
                    accepted = source.read_text()
                    source.write_text(accepted + f"fn {method}(&self) {{ capture_sources(self); }}")
                    self.assertEqual(check(root), [])
                    for extra in (
                            f"fn {method}(&self, gpu: &mut NativeRenderer) {{ capture_sources(self); }}",
                            f"fn {method}(&self) {{ let renderer = self.renderer(); }}",
                            f"fn {method}(&self) {{ tile.render(r, p, x, true, push); }}",
                            f"fn {method}(&self) {{ gpu.submit(fence); }}",
                            f"fn {method}(&self) {{ gpu.wait_for_fence(); }}"):
                        source.write_text(accepted + extra)
                        self.assertTrue(check(root))
                    source.write_text(accepted)

    def test_output_scene_projection_also_stays_renderer_free(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            source = root / "src/layout/scene_source.rs"
            accepted = self.captured_scene_fixture()
            for method in ("scene_source", "scene_backgrounds"):
                source.write_text(accepted + f"fn {method}(&self) {{ capture_sources(self); }}")
                self.assertEqual(check(root), [])
                source.write_text(accepted + f"fn {method}(&self, ctx: RenderCtx<R>) {{ capture_sources(self); }}")
                self.assertTrue(check(root))
                source.write_text(accepted + f"fn {method}(&self) {{ source.render(r, push); }}")
                self.assertTrue(check(root))


if __name__ == "__main__":
    unittest.main()
