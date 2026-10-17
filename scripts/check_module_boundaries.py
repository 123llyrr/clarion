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

WINDOW_LAYOUT_RECORDS = {
    "Tile", "WindowPresentationState", "OpenAnimation", "ResizeAnimation", "AlphaAnimation",
    "ClosingWindow", "Monitor", "Mapped", "Workspace",
}
WINDOW_GPU_STORAGE = re.compile(
    r"\b(?:OffscreenBuffer|NativeOffscreenBuffer|GlesTexture|VulkanTexture|TextureBuffer|"
    r"RenderSnapshot|LayoutElementRenderSnapshot|TileRenderSnapshot|NativeTileSnapshot|"
    r"ResizeSnapshot|CapturedResizeTexture|ClosingBuffer|AnimationBuffers|ResizeImages|"
    r"WindowRenderResources|WindowResourceLease|ResourceLease|MonitorResources|ClosingResources|"
    r"MappedResources|MruTexture|TextTextureCache|CubeFaceRenderElement)\b"
)

NUMERIC_WINDOW_MODELS = (
    "src/layout/window_presentation.rs", "src/layout/opening_window.rs",
    "src/layout/minimized.rs", "src/layout/wobble.rs",
)
WINDOW_DRAW_STORAGE = re.compile(
    r"\b(?:FocusRing|Shadow|SolidColorBuffer|WindowDecorations|RoundedCornerDamage|"
    r"WobbleMesh|WobbleForecast|WobbleCache|GenieMesh|GenieCache|MeshRenderElement|"
    r"ExtraDamage|AnimationBuffers|ResizeImages|WindowResourceLease|ResourceLease)\b"
)
NUMERIC_WINDOW_RENDERER = re.compile(
    r"\b(?:crate::(?:render_helpers|scene)|smithay::backend::renderer|ash|vulkan|gles)::|"
    r"\b(?:\w*Renderer|\w*Texture|\w*Shader|\w*Cache|\w*RenderElement|\w*Buffer|\w*RenderSnapshot|"
    r"RenderCtx|RenderTarget|WindowRenderResources|WindowResourceLease|ResourceLease|"
    r"WobbleMesh|WobbleForecast|GenieMesh|AnimationBuffers|ResizeImages)\b"
)


def mask_literals(source):
    return re.sub(
        r'r(?P<hashes>#+)".*?"(?P=hashes)|r"[^"]*"|"(?:\\.|[^"\\])*"|'
        r"'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'|/\*.*?\*/|//[^\n]*",
        lambda match: re.sub(r"[^\n]", " ", match[0]),
        source,
        flags=re.S,
    )


def production(source):
    # Mask comments and strings before counting braces, including raw shader
    # strings. Fixtures may appear before production items; exclude only their
    # annotated item or field, never the rest of the file.
    source = mask_literals(source)
    test_only = re.compile(r"#\[\s*cfg\s*\(\s*test\s*\)\s*\]")
    while match := test_only.search(source):
        end = match.end()
        parentheses = brackets = angles = 0
        while end < len(source):
            char = source[end]
            if char == "(":
                parentheses += 1
            elif char == ")":
                parentheses -= 1
            elif char == "[":
                brackets += 1
            elif char == "]":
                brackets -= 1
            elif char == "<":
                angles += 1
            elif char == ">" and angles:
                angles -= 1
            elif not parentheses and not brackets and not angles:
                if char in ";,":
                    end += 1
                    break
                if char == "{":
                    depth = 1
                    end += 1
                    while end < len(source) and depth:
                        depth += (source[end] == "{") - (source[end] == "}")
                        end += 1
                    break
                end += 1
                continue
            end += 1
        source = source[:match.start()] + " " + source[end:]
    return source


def test_only_source_files(root):
    """Resolve explicit test-only external modules, never filenames alone."""
    files = set()
    declaration = re.compile(
        r"#\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*"
        r"(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;"
    )
    for parent in (root / "src").rglob("*.rs"):
        source = mask_literals(parent.read_text())
        base = parent.parent if parent.name in ("mod.rs", "lib.rs", "main.rs") else parent.with_suffix("")
        for match in declaration.finditer(source):
            module = base / match[1]
            if module.with_suffix(".rs").is_file():
                files.add(module.with_suffix(".rs"))
            if module.is_dir():
                files.update(module.rglob("*.rs"))
    return files


def production_dependencies(manifest):
    tables = [manifest]
    tables.extend(manifest.get("target", {}).values())
    return {
        dependency.get("package", name) if isinstance(dependency, dict) else name
        for table in tables
        for kind in ("dependencies", "build-dependencies")
        for name, dependency in table.get(kind, {}).items()
    }


def struct_fields(source):
    """Yield named record bodies, not implementation arguments or local leases."""
    for match in re.finditer(r"\bstruct\s+(\w+)\b[^;{]*\{", source):
        end = match.end()
        depth = 1
        while end < len(source) and depth:
            depth += (source[end] == "{") - (source[end] == "}")
            end += 1
        yield match[1], source[match.end():end - 1]


def rust_body(source, pattern):
    """Find one named Rust item/method body in already masked production text."""
    match = re.search(pattern + r"[^;{]*\{", source)
    if match is None:
        return None
    end, depth = match.end(), 1
    while end < len(source) and depth:
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    return source[match.end():end - 1] if depth == 0 else None


def compact_rust(source):
    """Normalize formatting and legal trailing commas in a small Rust contract."""
    source = re.sub(r"\s+", "", source)
    return re.sub(r",(?=[)\]}])", "", source)


def aliases_referenced_by_record(source, fields, forbidden):
    """Follow simple local aliases so a field rename cannot hide a capability."""
    aliases = {match[2]: match[1]
               for match in re.finditer(r"\b(\w+)\s+as\s+(\w+)\b", source)}
    aliases.update({match[1]: match[2]
                    for match in re.finditer(r"\btype\s+(\w+)\b[^;=]*=([^;]*);", source)})
    pending = [name for name in aliases if re.search(r"\b" + name + r"\b", fields)]
    visited = set()
    while pending:
        name = pending.pop()
        if name in visited:
            continue
        visited.add(name)
        target = aliases[name]
        if forbidden.search(target):
            return True
        pending.extend(alias for alias in aliases if re.search(r"\b" + alias + r"\b", target))
    return False


# These deliberately small compatibility/dispatch functions are the boundary:
# a permitted lifecycle operation may retire scene resources, but no renderer
# callback may recover a desktop owner. Keep this contract explicit rather than
# attempting to recognize arbitrary Rust expressions with a permissive regex.
CAPTURED_DISPATCH = {
    "render_workspaces": """
        if let Some(lifecycle) = &self.cube_lifecycle { lifecycle.leave_cube(); }
        match &self.presentation {
            Presentation::Ordinary(composition) => composition.render(ctx, focus_ring, push),
            Presentation::Stage(composition) => { composition.render(ctx, focus_ring, &mut |_, _, _, _, _| {}, push) }
            Presentation::Spread(composition) => composition.render(ctx, push),
            Presentation::Cube(composition) => composition.render(ctx, focus_ring, push),
        }
    """,
    "render_stage": """
        if let Presentation::Stage(stage) = &self.presentation {
            if let Some(lifecycle) = &self.cube_lifecycle { lifecycle.leave_cube(); }
            stage.render(ctx, focus_ring, layers, push);
        }
    """,
    "render_shadows": """
        match &self.presentation {
            Presentation::Ordinary(composition) => composition.render_shadows(renderer, push),
            Presentation::Spread(composition) => composition.render_shadows(renderer, push),
            Presentation::Stage(_) | Presentation::Cube(_) => {}
        }
    """,
}

LAYOUT_COMPATIBILITY = {
    "src/layout/monitor.rs": {
        "render_workspaces": """
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
        """,
        "render_workspace_shadows": """
            if self.is_stage_visible() { return; }
            if self.is_spread_visible() {
                self.spread_composition().render_shadows(renderer, push);
            } else if !self.is_cube_scene() {
                self.standard_composition().render_shadows(renderer, push);
            }
        """,
    },
    "src/layout/monitor/stage.rs": {
        "render_stage": "self.stage_composition().render(ctx, focus_ring, &mut |_, _, _, _, _| {}, push);",
        "render_stage_with_layers": "self.stage_composition().render(ctx, focus_ring, layers, push);",
    },
    "src/layout/monitor/spread.rs": {
        "render_spread": "self.spread_composition().render(ctx, push);",
    },
}

PROJECTION_METHODS = {
    "src/layout/scene_source.rs": ("scene_source", "scene_backgrounds"),
    "src/layout/monitor.rs": (
        "standard_composition", "composition_with_sources", "cube_composition",
        "cube_lifecycle", "cube_lifecycle_for",
    ),
    "src/layout/monitor/stage.rs": ("stage_composition",),
    "src/layout/monitor/spread.rs": ("spread_composition",),
}


def captured_scene_boundaries(root):
    """Tripwires for all four captured presentations and renderer-free projection.

    These check named boundary records and their small forwarding functions, not
    arbitrary formatting or source line numbers. General scene-owner and weak
    resource-registry checks below remain independently enforced.
    """
    errors = []
    path = root / "src/layout/scene_source.rs"
    if path.is_file():
        source = production(path.read_text())
        records = dict(struct_fields(source))
        fields = records.get("MonitorScene", "")
        presentation = rust_body(source, r"\benum\s+Presentation\b")
        expected_presentation = """
            Ordinary(Composition<'a, W>),
            Stage(StageComposition<'a, W>),
            Spread(SpreadComposition<'a, W>),
            Cube(CubeComposition<'a, W>),
        """
        if (compact_rust(presentation or "").rstrip(",") != compact_rust(expected_presentation).rstrip(",")
                or not re.search(r"\bpresentation\s*:\s*Presentation\s*<\s*'a\s*,\s*W\s*>\s*,", fields)):
            errors.append("src/layout/scene_source.rs: Presentation must hold exactly one inline captured composition")
        if re.search(r"\b(?:composition|stage_composition|spread_composition|cube_composition)\s*:", fields):
            errors.append("src/layout/scene_source.rs: inactive presentation compositions must not be stored alongside the active payload")
        if not re.search(r"\bcube_lifecycle\s*:\s*Option\s*<\s*CubeLifecycle\b", fields):
            errors.append("src/layout/scene_source.rs: MonitorScene must retain the narrow Cube exit lifecycle separately")
        owner = re.compile(r"\b(?:Tile|Layout|Monitor|Workspace|DesktopCore|Clarion|OutputState|EventLoop|LoopHandle)\b")
        callback_storage = re.compile(r"\bBox\b|\bdyn\s+Fn(?:Mut|Once)?\b|\bimpl\s+Fn(?:Mut|Once)?\b|\bfn\s*\(|\bStageLayers\b")
        # Check helper records too: a wrapper or type alias must not turn the
        # old monitor adapter into an apparently narrow field in MonitorScene.
        for name, record in records.items():
            if owner.search(record) or callback_storage.search(record):
                errors.append(f"src/layout/scene_source.rs: {name} retains an owner, per-frame box or callback instead of narrow sources")
        for declaration in re.finditer(r"\benum\s+(\w+)\b", source):
            record = rust_body(source[declaration.start():], r"\benum\s+" + declaration[1] + r"\b") or ""
            if owner.search(record) or callback_storage.search(record):
                errors.append(f"src/layout/scene_source.rs: {declaration[1]} wraps an unrestricted owner or callback")
        for alias in re.finditer(r"\btype\s+(\w+)\b[^;=]*=([^;]*);", source):
            if owner.search(alias[2]) or re.search(r"\bBox\b", alias[2]) or (
                alias[1] != "StageLayers" and callback_storage.search(alias[2])
            ):
                errors.append(f"src/layout/scene_source.rs: {alias[1]} aliases an unrestricted owner or callback")
        for alias in re.finditer(owner.pattern + r"\s+as\s+(\w+)", source):
            if any(re.search(r"\b" + alias[1] + r"\b", record) for record in records.values()):
                errors.append(f"src/layout/scene_source.rs: {alias[1]} renames an owner retained by a source record")
        for method, expected in CAPTURED_DISPATCH.items():
            body = rust_body(source, r"\bfn\s+" + method + r"\b")
            if body is None or compact_rust(body) != compact_rust(expected):
                errors.append(f"src/layout/scene_source.rs: {method} must use only captured compositions and the narrow Cube lifecycle")

    for relative, methods in LAYOUT_COMPATIBILITY.items():
        path = root / relative
        if not path.is_file():
            continue
        source = production(path.read_text())
        for method, expected in methods.items():
            body = rust_body(source, r"\bfn\s+" + method + r"\b")
            if body is None or compact_rust(body) != compact_rust(expected):
                errors.append(f"{relative}: {method} compatibility entry must only delegate to the single captured scene implementation")
        if re.search(r"\bfn\s+(?:render_cube_workspaces(?:_native)?|render_stage_border|render_stage_rail|update_cube_visible_workspaces)\b", source):
            errors.append(f"{relative}: legacy specialized drawing or GPU lifecycle body belongs in the scene")

    renderer = re.compile(r"\b(?:\w*Renderer|RenderCtx|renderer|ctx)\b")
    drawing = re.compile(r"\.\s*(?:render|render_workspaces|render_scrolling|render_floating|render_shadow|render_stage|render_cube_workspaces(?:_native)?|draw|submit|wait_for_[a-z_]+)\s*\(")
    for relative, methods in PROJECTION_METHODS.items():
        path = root / relative
        if not path.is_file():
            continue
        source = production(path.read_text())
        for method in methods:
            signature = re.search(r"\bfn\s+" + method + r"\b[^;{]*\{", source)
            if signature is None:
                continue
            body = rust_body(source, r"\bfn\s+" + method + r"\b") or ""
            if renderer.search(signature[0] + body) or drawing.search(body):
                errors.append(f"{relative}: {method} projection must be renderer-free and cannot draw, submit or wait")
    return errors


TILE_SCENE_COMPATIBILITY = {
    "render": "self.scene().render(ctx, location, xray_pos, focus_ring, push);",
    "render_genie": "self.scene().render_genie(ctx, xray_pos, push)",
    "complete_genie": """
        self.presentation.genie.borrow_mut().take();
        self.resources.lease().clear_genie_retirement();
        self.scene().clear_genie_pixels();
    """,
    "clear_wobble_render_cache": """
        self.resources.lease().clear_wobble_retirement();
        self.scene().clear_wobble_pixels();
    """,
    "handle_native_effect_error": "self.scene().handle_native_effect_error(effect, error)",
    "scene": """crate::scene::tile::TileScene {
        window: &self.window, state: &self.presentation, resources: self.resources.lease(),
    }""",
    "snapshot_native": "crate::scene::window_snapshot::snapshot_native(&self.scene(), &self.resources, renderer)",
    "take_native_close_source": "crate::scene::window_snapshot::take_native_close_source(&self.window, &self.resources)",
    "store_unmap_snapshot_native_if_empty": "crate::scene::window_snapshot::store_native_unmap_if_empty(&self.scene(), &self.resources, renderer, xray, blocked, pos)",
    "store_unmap_snapshot_if_empty": "crate::scene::window_snapshot::store_gles_unmap_if_empty(&self.scene(), &self.resources, renderer, xray, blocked, pos)",
    "take_unmap_snapshot": "crate::scene::window_snapshot::take_unmap_snapshot(&self.resources)",
    "capture_resize_retarget": """
        let resize = self.presentation.resize_animation.as_ref().unwrap();
        crate::scene::resize_snapshot::capture_resize_retarget(
            renderer, &self.resources, next,
            crate::scene::resize_snapshot::ResizeRetargetPose {
                size: resize.size_value(next.size(), false),
                progress: resize.anim.value() as f32,
                clamped: resize.anim.clamped_value().clamp(0., 1.) as f32,
                scale: self.presentation.scale,
                custom_shader: self.presentation.options.animations.window_resize.custom_shader.as_deref(),
            },
        )
    """,
}


def window_presentation_boundaries(root):
    """Keep model trajectories numeric and draw/capture capabilities one way.

    These are narrow contracts for the migrated window paths. They do not
    exempt the rest of layout from the independent GPU ownership checks.
    """
    errors = []
    test_files = test_only_source_files(root)
    for relative in NUMERIC_WINDOW_MODELS:
        path = root / relative
        if not path.is_file() or path in test_files:
            continue
        source = production(path.read_text())
        if NUMERIC_WINDOW_RENDERER.search(source):
            errors.append(f"{relative}: numeric window model imports or accesses scene/GPU rendering")
        # Config FocusRing/Shadow are immutable style values, not the retained
        # decoration implementations that carry render buffers and identities.
        fields_source = re.sub(r"\bclarion_config::(?:FocusRing|Shadow)\b", "NumericStyle", source)
        for name, fields in struct_fields(fields_source):
            if WINDOW_GPU_STORAGE.search(fields) or WINDOW_DRAW_STORAGE.search(fields):
                errors.append(f"{relative}: {name} retains drawing state inside a numeric window model")

    tile = root / "src/layout/tile.rs"
    if tile.is_file() and tile not in test_files:
        source = production(tile.read_text())
        fields = dict(struct_fields(source)).get("Tile", "")
        tile_storage = re.compile(WINDOW_GPU_STORAGE.pattern + "|" + WINDOW_DRAW_STORAGE.pattern)
        if (tile_storage.search(fields)
                or aliases_referenced_by_record(source, fields, tile_storage)
                or re.search(
                r"\b(?:border|focus_ring|shadow|fullscreen_backdrop|rounded_corner_damage|"
                r"wobble_cache|genie_cache|mesh_cache)\s*:", fields)):
            errors.append("src/layout/tile.rs: Tile retains scene decorations or effect caches")
        for method, expected in TILE_SCENE_COMPATIBILITY.items():
            body = rust_body(source, r"\bfn\s+" + method + r"\b")
            if body is not None and compact_rust(body) != compact_rust(expected):
                errors.append(f"src/layout/tile.rs: {method} must only project or delegate to scene-owned drawing/capture")
        if re.search(r"\bfn\s+(?:render_inner|render_wobble|render_resize|render_opening_window|capture_native|capture_gles)\b", source):
            errors.append("src/layout/tile.rs: migrated window drawing/capture body returned to layout")

    workspace = root / "src/layout/workspace.rs"
    if workspace.is_file() and workspace not in test_files:
        source = production(workspace.read_text())
        fields = dict(struct_fields(source)).get("Workspace", "")
        fields_clean = re.sub(r"\bResource(?:Handle|Allocator)\s*<[^>]*>", "", fields)
        ws_storage = re.compile(WINDOW_GPU_STORAGE.pattern + "|" + WINDOW_DRAW_STORAGE.pattern)
        if (ws_storage.search(fields_clean)
                or aliases_referenced_by_record(source, fields_clean, ws_storage)
                or re.search(r"\b(?:shadow|background_buffer)\s*:", fields)):
            errors.append("src/layout/workspace.rs: Workspace retains scene decorations or render buffers")

    monitor = root / "src/layout/monitor.rs"
    if monitor.is_file() and monitor not in test_files:
        source = production(monitor.read_text())
        fields = dict(struct_fields(source)).get("Monitor", "")
        fields_clean = re.sub(r"\bResource(?:Handle|Allocator)\s*<[^>]*>", "", fields)
        mon_storage = re.compile(WINDOW_GPU_STORAGE.pattern + "|" + WINDOW_DRAW_STORAGE.pattern)
        if (mon_storage.search(fields_clean)
                or aliases_referenced_by_record(source, fields_clean, mon_storage)
                or re.search(r"\b(?:expo_borders|stage_borders)\s*:", fields)):
            errors.append("src/layout/monitor.rs: Monitor retains scene decorations or render buffers")

    owner = re.compile(r"\b(?:Tile|Layout|Monitor|Workspace|DesktopCore|Clarion|OutputState|LoopHandle|EventLoop)\b")
    callback = re.compile(r"\bBox\b|\bdyn\s+Fn(?:Mut|Once)?\b|\bimpl\s+Fn(?:Mut|Once)?\b|\bfn\s*\(")
    for relative in ("src/scene/tile.rs", "src/scene/window_snapshot.rs", "src/scene/opening_window.rs",
                     "src/scene/resize_snapshot.rs"):
        path = root / relative
        if not path.is_file() or path in test_files:
            continue
        source = production(path.read_text())
        if owner.search(source):
            errors.append(f"{relative}: window composition/capture reacquires a layout owner")
        if relative == "src/scene/tile.rs":
            # TileScene dereferences the event-owned numeric presentation, but
            # owns its resource caches. Read the two model RefCells only through
            # immutable borrows; even handing out the RefCell could let a helper
            # cancel an owner trajectory while composing another output.
            model = re.compile(r"\bself\s*\.\s*(?:state\s*\.\s*)?(?:wobble|genie)\b")
            if any(not re.match(r"\s*\.\s*borrow\s*\(\s*\)", source[match.end():])
                   for match in model.finditer(source)):
                errors.append("src/scene/tile.rs: composition must only read numeric wobble/genie; retirement belongs to the layout owner")
        for name, fields in struct_fields(source):
            if name == "TileScene" and (owner.search(fields) or callback.search(fields)
                                        or aliases_referenced_by_record(source, fields, callback)):
                errors.append(f"{relative}: TileScene retains an owner or drawing callback")
        if relative != "src/scene/tile.rs" and re.search(
                r"\bcrate::animation\b|\b(?:Animation|Clock|OpenAnimation|ResizeAnimation|AlphaAnimation|Genie)\b", source):
            errors.append(f"{relative}: snapshot/opening drawing must consume frozen poses instead of animation models or clocks")

    path = root / "src/layout/scene_source.rs"
    if path.is_file():
        records = dict(struct_fields(production(path.read_text())))
        for record, field in (("TileRenderSource", "scene"), ("MovingTileScene", "tile")):
            if record not in records:
                continue
            fields = records[record]
            if (owner.search(fields) or callback.search(fields)
                    or not re.search(r"\b" + field + r"\s*:\s*(?:\w+::)*TileScene\s*<", fields)):
                errors.append(f"src/layout/scene_source.rs: {record} must retain only the projected TileScene and numeric placement")
    return errors


def check(root):
    errors = captured_scene_boundaries(root) + window_presentation_boundaries(root)
    test_files = test_only_source_files(root)
    workspace = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["members"]
    for name, permitted in PURE_DEPENDENCIES.items():
        if name not in workspace:
            errors.append(f"{name}: absent from the production workspace")
        manifest = tomllib.loads((root / name / "Cargo.toml").read_text())
        dependencies = production_dependencies(manifest)
        if dependencies - permitted:
            errors.append(f"{name}: forbidden dependencies {sorted(dependencies - permitted)}")
    forbidden = re.compile(r"\bcrate::(?:compositor|layout|input)\b")
    for path in sorted((root / "src/backend").rglob("*.rs")):
        if path in test_files:
            continue
        if forbidden.search(production(path.read_text())):
            errors.append(f"{path.relative_to(root)}: device backend accesses desktop state")
    full_owner = re.compile(r"\b(?:crate::compositor|Clarion|OutputState|LoopHandle|EventLoop)\b|\b(?:Layout|Monitor|Workspace|DesktopCore)\b(?!\s*=)")
    for path in sorted((root / "src/scene").rglob("*.rs")):
        if path in test_files:
            continue
        if full_owner.search(production(path.read_text())):
            errors.append(f"{path.relative_to(root)}: scene owns an unrestricted platform capability")
    adapters = list((root / "src/layout").rglob("*.rs"))
    if (root / "src/window/mapped.rs").is_file():
        adapters.append(root / "src/window/mapped.rs")
    for path in sorted(adapters):
        if path in test_files:
            continue
        source = production(path.read_text())
        if re.search(r"\b(?:WindowResourceRegistry|ResourceRegistry|SceneResources)\b", source):
            errors.append(f"{path.relative_to(root)}: layout reacquires the strong scene resource owner")
        for name, fields in struct_fields(source):
            # A generic registration names its payload without retaining it.
            fields = re.sub(r"\bResource(?:Handle|Allocator)\s*<[^>]*>", "", fields)
            if name in WINDOW_LAYOUT_RECORDS and WINDOW_GPU_STORAGE.search(fields):
                errors.append(f"{path.relative_to(root)}: {name} retains window GPU storage instead of a weak resource handle")

    for relative, names in (
        ("src/scene/window_resources.rs", ("WindowResourceAllocator", "WindowResourceHandle")),
        ("src/scene/retained.rs", ("ResourceAllocator", "ResourceHandle")),
    ):
        registry = root / relative
        if not registry.is_file():
            continue
        records = dict(struct_fields(production(registry.read_text())))
        for name in names:
            fields = records.get(name, "")
            if not re.search(r"\bowner\s*:\s*Weak\s*<\s*RegistryInner\b", fields):
                errors.append(f"{registry.relative_to(root)}: {name} must borrow its registry through Weak")
            if re.search(r"\b(?:Rc|Arc|WindowResourceRegistry|ResourceRegistry|SceneResources|WindowRenderResources|WindowResourceLease|ResourceLease)\b", fields):
                errors.append(f"{registry.relative_to(root)}: {name} retains strong scene resource ownership")
    return errors


if __name__ == "__main__":
    issues = check(Path(__file__).resolve().parents[1])
    for issue in issues:
        print(issue, file=sys.stderr)
    if issues:
        raise SystemExit(1)
    print("Clarion module dependency boundaries passed.")
