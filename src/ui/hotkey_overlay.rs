use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::{self, Write as _};
use std::rc::Rc;

use clarion_config::{Action, Bind, Config, Key, ModKey, Modifiers, Trigger};
use pangocairo::cairo::{self, ImageSurface};
use pangocairo::pango::{
    AttrColor, AttrInt, AttrList, AttrString, EllipsizeMode, FontDescription, Weight, WrapMode,
};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::{ContextId, ImportMem, Texture};
use smithay::input::keyboard::xkb::keysym_get_name;
use smithay::input::keyboard::Keysym;
use smithay::output::{Output, WeakOutput};
use smithay::reexports::gbm::Format as Fourcc;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Scale, Transform};

use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::ClarionRenderer;
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::utils::{output_size, to_physical_precise_round};

const PADDING: i32 = 8;
const MARGIN: i32 = PADDING * 2;
const COLUMN_GAP: i32 = PADDING * 3;
const MIN_COLUMN_WIDTH: i32 = 420;
use crate::ui::theme::{FONT, ON_SURFACE, PRIMARY, SURFACE};
const COMPACT_FONT: &str = "sans 12px";
const BORDER: i32 = 4;
const LINE_INTERVAL: i32 = 2;
const TITLE: &str = "Important Hotkeys";

pub struct HotkeyOverlay {
    is_open: bool,
    config: Rc<RefCell<Config>>,
    mod_key: ModKey,
    buffers: UserDataMap,
    page: usize,
    page_counts: RefCell<HashMap<WeakOutput, usize>>,
}

struct RenderedOverlay<T: Texture> {
    buffer: Option<TextureBuffer<T>>,
    target_size: (i32, i32),
    page: usize,
    page_count: usize,
}

impl HotkeyOverlay {
    pub fn new(config: Rc<RefCell<Config>>, mod_key: ModKey) -> Self {
        Self {
            is_open: false,
            config,
            mod_key,
            buffers: UserDataMap::new(),
            page: 0,
            page_counts: RefCell::new(HashMap::new()),
        }
    }

    pub fn show(&mut self) -> bool {
        let changed = !self.is_open || self.page != 0;
        self.is_open = true;
        if changed {
            self.page = 0;
            self.page_counts.get_mut().clear();
            self.buffers = UserDataMap::new();
        }
        changed
    }

    /// Consume only unmodified navigation keys; other shortcuts keep working normally.
    pub fn handle_key(&mut self, keysym: Keysym, modifiers: Modifiers) -> bool {
        if !self.is_open || !modifiers.is_empty() {
            return false;
        }

        let pages = self
            .page_counts
            .get_mut()
            .iter()
            .filter(|(output, _)| output.is_alive())
            .map(|(_, pages)| *pages)
            .max()
            .unwrap_or(1);
        let last = pages.saturating_sub(1);
        let current = self.page.min(last);
        let page = match keysym {
            Keysym::Prior => current.saturating_sub(1),
            Keysym::Next => current.saturating_add(1).min(last),
            Keysym::Home => 0,
            Keysym::End => last,
            Keysym::Escape => {
                self.hide();
                return true;
            }
            _ => return false,
        };

        if self.page != page {
            self.page = page;
            self.buffers = UserDataMap::new();
        }
        true
    }

    pub fn hide(&mut self) -> bool {
        if self.is_open {
            self.is_open = false;
            true
        } else {
            false
        }
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn on_hotkey_config_updated(&mut self, mod_key: ModKey) {
        self.mod_key = mod_key;
        self.page = 0;
        self.page_counts.get_mut().clear();
        self.buffers = UserDataMap::new();
    }

    pub(crate) fn on_native_renderer_replaced(&self) {
        if let Some(cache) = self
            .buffers
            .get::<OverlayTextures<crate::render_helpers::vulkan::VulkanTexture>>()
        {
            cache.0.borrow_mut().clear();
        }
    }

    pub fn render<R: ClarionRenderer>(
        &self,
        renderer: &mut R,
        output: &Output,
    ) -> Option<PrimaryGpuTextureRenderElement> {
        if let Some(gles) = renderer.as_gles_renderer() {
            self.render_with(gles, output)
                .map(PrimaryGpuTextureRenderElement::Gles)
        } else if let Some(native) = renderer.as_native_renderer() {
            self.render_with(native, output)
                .map(PrimaryGpuTextureRenderElement::Vulkan)
        } else {
            None
        }
    }

    /// Draw using textures owned by the selected renderer.
    pub fn render_with<R>(
        &self,
        renderer: &mut R,
        output: &Output,
    ) -> Option<TextureRenderElement<R::TextureId>>
    where
        R: ImportMem,
        R::TextureId: Clone + 'static,
        R::Error: std::error::Error + Send + Sync + 'static,
    {
        if !self.is_open {
            return None;
        }

        let scale = output.current_scale().fractional_scale();
        let output_size = output_size(output);
        let physical_size = output_size.to_physical_precise_round(scale);
        let target_size = (physical_size.w, physical_size.h);

        let cache = self
            .buffers
            .get_or_insert(|| OverlayTextures::<R::TextureId>(RefCell::new(HashMap::new())));
        let mut buffers = cache.0.borrow_mut();
        buffers.retain(|(_, output), _| output.is_alive());

        let weak = (renderer.context_id(), output.downgrade());
        if buffers.get(&weak).is_some_and(|rendered| {
            rendered.target_size != target_size
                || rendered.page != self.page
                || rendered
                    .buffer
                    .as_ref()
                    .is_some_and(|buffer| buffer.texture_scale() != Scale::from(scale))
        }) {
            buffers.remove(&weak);
        }

        let rendered = buffers.entry(weak).or_insert_with(|| {
            render(
                renderer,
                &self.config.borrow(),
                self.mod_key,
                scale,
                target_size,
                self.page,
            )
            .unwrap_or_else(|err| {
                warn!("error rendering hotkey overlay: {err}");
                RenderedOverlay {
                    buffer: None,
                    target_size,
                    page: self.page,
                    page_count: 1,
                }
            })
        });
        let mut page_counts = self.page_counts.borrow_mut();
        page_counts.retain(|output, _| output.is_alive());
        page_counts.insert(output.downgrade(), rendered.page_count);
        let buffer = rendered.buffer.as_ref()?;

        let size = buffer.logical_size();
        let location = (output_size.to_f64().to_point() - size.to_point()).downscale(2.);
        let mut location = location.to_physical_precise_round(scale).to_logical(scale);
        location.x = f64::max(0., location.x);
        location.y = f64::max(0., location.y);

        let elem = TextureRenderElement::from_texture_buffer(
            buffer.clone(),
            location,
            0.9,
            None,
            None,
            Kind::Unspecified,
        );

        Some(elem)
    }

    pub fn a11y_text(&self) -> String {
        let config = self.config.borrow();
        let actions = collect_actions(&config);

        let mut buf = String::new();
        writeln!(&mut buf, "{TITLE}").unwrap();

        for action in actions {
            let Some((key, action)) = format_bind(&config.binds.0, action) else {
                continue;
            };

            let key = key.map(|key| key_name(true, self.mod_key, &key));
            let key = key.as_deref().unwrap_or("not bound");

            let action = match pango::parse_markup(&action, '\0') {
                Ok((_attrs, text, _accel)) => text,
                Err(_) => action.into(),
            };

            writeln!(&mut buf, "{key} {action}").unwrap();
        }

        buf
    }
}

fn format_bind(binds: &[Bind], action: &Action) -> Option<(Option<Key>, String)> {
    let mut bind_with_non_null = None;
    let mut bind_with_custom_title = None;
    let mut found_null_title = false;

    for bind in binds {
        if bind.action != *action {
            continue;
        }

        match &bind.hotkey_overlay_title {
            Some(Some(_)) => {
                bind_with_custom_title.get_or_insert(bind);
            }
            Some(None) => {
                found_null_title = true;
            }
            None => {
                bind_with_non_null.get_or_insert(bind);
            }
        }
    }

    if bind_with_custom_title.is_none() && found_null_title {
        return None;
    }

    let mut title = None;
    let key = if let Some(bind) = bind_with_custom_title.or(bind_with_non_null) {
        if let Some(Some(custom)) = &bind.hotkey_overlay_title {
            title = Some(custom.clone());
        }

        Some(bind.key)
    } else {
        None
    };
    let title = title.unwrap_or_else(|| action_name(action));

    Some((key, title))
}

fn collect_actions(config: &Config) -> Vec<&Action> {
    let binds = &config.binds.0;

    // Collect actions that we want to show.
    let mut actions = vec![&Action::ShowHotkeyOverlay];

    // Prefer Quit(false) if found, otherwise try Quit(true), and if there's neither, fall back to
    // Quit(false).
    if binds.iter().any(|bind| bind.action == Action::Quit(false)) {
        actions.push(&Action::Quit(false));
    } else if binds.iter().any(|bind| bind.action == Action::Quit(true)) {
        actions.push(&Action::Quit(true));
    } else {
        actions.push(&Action::Quit(false));
    }

    actions.extend(&[
        &Action::CloseWindow,
        &Action::FocusColumnLeft,
        &Action::FocusColumnRight,
        &Action::MoveColumnLeft,
        &Action::MoveColumnRight,
        &Action::FocusWorkspaceDown,
        &Action::FocusWorkspaceUp,
    ]);

    // Prefer move-column-to-workspace-down, but fall back to move-window-to-workspace-down.
    if let Some(bind) = binds
        .iter()
        .find(|bind| matches!(bind.action, Action::MoveColumnToWorkspaceDown(_)))
    {
        actions.push(&bind.action);
    } else if let Some(bind) = binds
        .iter()
        .find(|bind| matches!(bind.action, Action::MoveWindowToWorkspaceDown(_)))
    {
        actions.push(&bind.action);
    } else {
        actions.push(&Action::MoveColumnToWorkspaceDown(true));
    }

    // Same for -up.
    if let Some(bind) = binds
        .iter()
        .find(|bind| matches!(bind.action, Action::MoveColumnToWorkspaceUp(_)))
    {
        actions.push(&bind.action);
    } else if let Some(bind) = binds
        .iter()
        .find(|bind| matches!(bind.action, Action::MoveWindowToWorkspaceUp(_)))
    {
        actions.push(&bind.action);
    } else {
        actions.push(&Action::MoveColumnToWorkspaceUp(true));
    }

    actions.extend(&[
        &Action::SwitchPresetColumnWidth,
        &Action::MaximizeColumn,
        &Action::ConsumeOrExpelWindowLeft,
        &Action::ConsumeOrExpelWindowRight,
        &Action::ToggleWindowFloating,
        &Action::SwitchFocusBetweenFloatingAndTiling,
        &Action::ToggleOverview,
    ]);

    // Screenshot is not as important, can omit if not bound.
    if let Some(bind) = binds
        .iter()
        .find(|bind| matches!(bind.action, Action::Screenshot(_, _)))
    {
        actions.push(&bind.action);
    }

    // Add actions with a custom hotkey-overlay-title.
    for bind in binds {
        if matches!(bind.hotkey_overlay_title, Some(Some(_))) {
            // Avoid duplicate actions.
            if !actions.contains(&&bind.action) {
                actions.push(&bind.action);
            }
        }
    }

    // Discover all configured compositor shortcuts, including newly added actions.
    // Media keys and pointer gestures stay opt-in through a custom title.
    for bind in binds.iter().filter(|bind| {
        (bind.key.modifiers.contains(Modifiers::COMPOSITOR)
            || bind.key.modifiers.contains(Modifiers::SUPER))
            // Also filter out wheel and touchpad scroll binds.
            && matches!(bind.key.trigger, Trigger::Keysym(_))
    }) {
        let action = &bind.action;

        // Show one preferred binding for each distinct action.
        if !actions.contains(&action) {
            actions.push(action);
        }
    }

    if config.hotkey_overlay.hide_not_bound {
        // Only keep actions that have been bound
        actions.retain(|&action| binds.iter().any(|bind| bind.action == *action))
    }

    actions
}

type OverlayTextureMap<T> = HashMap<(ContextId<T>, WeakOutput), RenderedOverlay<T>>;
struct OverlayTextures<T: Texture>(RefCell<OverlayTextureMap<T>>);

fn render<R>(
    renderer: &mut R,
    config: &Config,
    mod_key: ModKey,
    scale: f64,
    target_size: (i32, i32),
    page: usize,
) -> anyhow::Result<RenderedOverlay<R::TextureId>>
where
    R: ImportMem,
    R::Error: std::error::Error + Send + Sync + 'static,
{
    let _span = clarion_tracy_span!("hotkey_overlay::render");
    let rendered = render_surface(config, mod_key, scale, target_size, page)?;
    let size = (rendered.surface.width(), rendered.surface.height());
    let data = rendered.surface.take_data()?;
    let buffer = TextureBuffer::from_memory(
        renderer,
        &data,
        Fourcc::Argb8888,
        size,
        false,
        scale,
        Transform::Normal,
        Vec::new(),
    )?;

    Ok(RenderedOverlay {
        buffer: Some(buffer),
        target_size,
        page,
        page_count: rendered.page_count,
    })
}

struct OverlaySurface {
    surface: ImageSurface,
    page_count: usize,
    // The visible range is shared by drawing and CPU regression tests.
    #[cfg(test)]
    visible: std::ops::Range<usize>,
    #[cfg(test)]
    footer: OverlayFooter,
}

struct OverlayRow {
    key: String,
    action: String,
    attributes: Option<AttrList>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NavigationKey {
    Previous,
    Next,
    First,
    Last,
    Close,
}

impl NavigationKey {
    fn label(self) -> &'static str {
        match self {
            Self::Previous => " PgUp ",
            Self::Next => " PgDn ",
            Self::First => " Home ",
            Self::Last => " End ",
            Self::Close => " Esc close ",
        }
    }

    fn is_enabled(self, page: usize, page_count: usize) -> bool {
        match self {
            Self::Previous | Self::First => page > 0,
            Self::Next | Self::Last => page + 1 < page_count,
            Self::Close => true,
        }
    }
}

struct NavigationCell {
    key: NavigationKey,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

struct NavigationLayout {
    cells: Vec<NavigationCell>,
    height: i32,
}

struct OverlayFooter {
    status: String,
    navigation: NavigationLayout,
    top: i32,
    navigation_top: i32,
    width: i32,
}

/// Pack whole navigation labels, so a narrow output never truncates the escape key.
fn layout_navigation(
    layout: &pango::Layout,
    width: i32,
    gap: i32,
    line_interval: i32,
) -> anyhow::Result<NavigationLayout> {
    layout.set_width(-1);
    let mut rows = vec![Vec::new()];
    let mut row_width = 0;
    let mut row_height = 0;
    for key in [
        NavigationKey::Previous,
        NavigationKey::Next,
        NavigationKey::First,
        NavigationKey::Last,
        NavigationKey::Close,
    ] {
        layout.set_text(key.label());
        let (w, h) = layout.pixel_size();
        anyhow::ensure!(w <= width, "output is too narrow for hotkey navigation");
        row_height = row_height.max(h);
        if row_width > 0 && row_width + gap + w > width {
            rows.push(Vec::new());
            row_width = 0;
        }
        rows.last_mut().unwrap().push((key, w));
        row_width += if row_width == 0 { w } else { gap + w };
    }

    let height = rows.len() as i32 * (row_height + line_interval) - line_interval;
    let mut cells = Vec::new();
    for (row, keys) in rows.into_iter().enumerate() {
        let occupied =
            keys.iter().map(|(_, w)| w).sum::<i32>() + gap * keys.len().saturating_sub(1) as i32;
        let mut x = (width - occupied) / 2;
        for (key, width) in keys {
            cells.push(NavigationCell {
                key,
                x,
                y: row as i32 * (row_height + line_interval),
                width,
                height: row_height,
            });
            x += width + gap;
        }
    }
    Ok(NavigationLayout { cells, height })
}

fn render_surface(
    config: &Config,
    mod_key: ModKey,
    scale: f64,
    target_size: (i32, i32),
    page: usize,
) -> anyhow::Result<OverlaySurface> {
    render_surface_with_font(config, mod_key, scale, target_size, page, FONT, PADDING).or_else(
        |_| {
            // Reserve enough space for the complete navigation on short, narrow
            // outputs. Do not shrink text on outputs that already fit the normal size.
            render_surface_with_font(
                config,
                mod_key,
                scale,
                target_size,
                page,
                COMPACT_FONT,
                PADDING / 2,
            )
        },
    )
}

fn render_surface_with_font(
    config: &Config,
    mod_key: ModKey,
    scale: f64,
    target_size: (i32, i32),
    page: usize,
    font_name: &str,
    logical_padding: i32,
) -> anyhow::Result<OverlaySurface> {
    anyhow::ensure!(target_size.0 > 0 && target_size.1 > 0);
    let margin = to_physical_precise_round::<i32>(scale, MARGIN)
        .min(target_size.0 / 8)
        .min(target_size.1 / 8);
    let max_width = target_size.0 - margin * 2;
    let max_height = target_size.1 - margin * 2;
    let padding = to_physical_precise_round::<i32>(scale, logical_padding).max(1);
    let line_interval = to_physical_precise_round::<i32>(scale, LINE_INTERVAL).max(1);
    let column_gap = to_physical_precise_round::<i32>(scale, COLUMN_GAP).max(1);

    let rows = collect_actions(config)
        .into_iter()
        .filter_map(|action| format_bind(&config.binds.0, action))
        .map(|(key, action)| {
            let key = key.map(|key| key_name(false, mod_key, &key));
            let key = format!(" {} ", key.as_deref().unwrap_or("(not bound)"));
            let (attributes, action) = parse_action_markup(&action);
            OverlayRow {
                key,
                action,
                attributes,
            }
        })
        .collect::<Vec<_>>();

    let mut font = FontDescription::from_string(font_name);
    font.set_absolute_size(to_physical_precise_round(scale, font.size()));
    let measure_surface = ImageSurface::create(cairo::Format::ARgb32, 0, 0)?;
    let cr = cairo::Context::new(&measure_surface)?;
    let layout = pangocairo::functions::create_layout(&cr);
    layout.context().set_round_glyph_positions(false);
    layout.set_font_description(Some(&font));
    layout.set_single_paragraph_mode(true);
    layout.set_text("Ag");
    let line_height = layout.pixel_size().1.max(1);

    let bold = AttrList::new();
    bold.insert(AttrInt::new_weight(Weight::Bold));
    layout.set_attributes(Some(&bold));
    layout.set_text(TITLE);
    let title_size = layout.pixel_size();

    let key_attrs = AttrList::new();
    key_attrs.insert(AttrString::new_family("Monospace"));
    key_attrs.insert(AttrColor::new_background(12000, 12000, 12000));
    let mut key_width = 0;
    let mut action_width = 0;
    let mut row_height = line_height;
    for row in &rows {
        layout.set_attributes(Some(&key_attrs));
        layout.set_text(&row.key);
        let (w, h) = layout.pixel_size();
        key_width = key_width.max(w);
        row_height = row_height.max(h);
        layout.set_attributes(row.attributes.as_ref());
        layout.set_text(&row.action);
        let (w, h) = layout.pixel_size();
        action_width = action_width.max(w);
        row_height = row_height.max(h);
    }
    // Keep oversized custom markup inside its cell rather than letting it grow the texture.
    row_height = row_height.min(line_height * 2);

    let inner_max_width = max_width - padding * 2;
    let columns = if inner_max_width
        >= to_physical_precise_round::<i32>(scale, MIN_COLUMN_WIDTH) * 2 + column_gap
    {
        2
    } else {
        1
    };
    let desired_column_width = key_width + padding + action_width;
    let desired_width = (desired_column_width * columns + column_gap * (columns - 1))
        .max(title_size.0)
        .max(to_physical_precise_round::<i32>(scale, 350))
        + padding * 2;
    let width = desired_width.min(max_width);
    let column_width = (width - padding * 2 - column_gap * (columns - 1)) / columns;
    let key_width = key_width.max(1).min((column_width - padding) / 2);
    let action_width = column_width - key_width - padding;
    // Wrap key combinations before truncating so narrow displays retain all modifiers.
    layout.set_width(key_width.saturating_mul(pango::SCALE));
    layout.set_wrap(WrapMode::WordChar);
    layout.set_attributes(Some(&key_attrs));
    for row in &rows {
        layout.set_text(&row.key);
        row_height = row_height.max(layout.pixel_size().1.min(line_height * 2));
    }
    layout.set_attributes(Some(&key_attrs));
    let navigation = layout_navigation(&layout, width - padding * 2, padding, line_interval)?;
    let fixed_height = padding * 4 + title_size.1 + line_height + line_interval + navigation.height;
    anyhow::ensure!(
        key_width > 0 && action_width > 0 && max_height >= fixed_height + row_height,
        "hotkey help needs at least {} px height in {}x{} at scale {scale} ({font_name}), has {max_height}",
        fixed_height + row_height, target_size.0, target_size.1,
    );
    let rows_per_column =
        ((max_height - fixed_height + line_interval) / (row_height + line_interval)) as usize;
    let page_capacity = rows_per_column * columns as usize;
    let page_count = rows.len().div_ceil(page_capacity).max(1);
    let page = page.min(page_count - 1);
    let start = page * page_capacity;
    let end = (start + page_capacity).min(rows.len());
    let visible = start..end;
    // Reserve the same body on every page. A short last page must not move the
    // centered panel, the rows already being read, or the navigation keys.
    let occupied_rows_per_column = rows.len().min(rows_per_column).max(1);
    let body_height =
        occupied_rows_per_column as i32 * (row_height + line_interval) - line_interval;
    let height = fixed_height + body_height;
    let footer_top = padding * 3 + title_size.1 + body_height;
    let range = if rows.is_empty() {
        String::from("0 shortcuts")
    } else {
        format!("{}–{} of {}", start + 1, end, rows.len())
    };
    layout.set_attributes(None);
    layout.set_width(-1);
    let mut status = format!("Page {}/{} · {range}", page + 1, page_count);
    layout.set_text(&status);
    if layout.pixel_size().0 > width - padding * 2 {
        status = format!("Page {}/{}", page + 1, page_count);
        layout.set_text(&status);
        if layout.pixel_size().0 > width - padding * 2 {
            status = format!("{}/{}", page + 1, page_count);
            layout.set_text(&status);
        }
    }
    anyhow::ensure!(layout.pixel_size().0 <= width - padding * 2);
    let footer = OverlayFooter {
        status,
        navigation,
        top: footer_top,
        navigation_top: footer_top + line_height + line_interval,
        width: width - padding * 2,
    };
    let surface = ImageSurface::create(cairo::Format::ARgb32, width, height)?;
    let cr = cairo::Context::new(&surface)?;
    cr.set_source_rgb(SURFACE.0, SURFACE.1, SURFACE.2);
    cr.paint()?;
    let layout = pangocairo::functions::create_layout(&cr);
    layout.context().set_round_glyph_positions(false);
    layout.set_font_description(Some(&font));
    layout.set_single_paragraph_mode(true);
    layout.set_ellipsize(EllipsizeMode::End);
    cr.set_source_rgb(ON_SURFACE.0, ON_SURFACE.1, ON_SURFACE.2);

    let draw_cell = |text: &str, attrs: Option<&AttrList>, x: i32, y: i32, w: i32, h: i32| {
        cr.save()?;
        cr.rectangle(x.into(), y.into(), w.into(), h.into());
        cr.clip();
        layout.set_width(w.saturating_mul(pango::SCALE));
        layout.set_attributes(attrs);
        layout.set_text(text);
        cr.move_to(x.into(), y.into());
        pangocairo::functions::show_layout(&cr, &layout);
        cr.restore()
    };
    let title_width = title_size.0.min(width - padding * 2);
    draw_cell(
        TITLE,
        Some(&bold),
        (width - title_width) / 2,
        padding,
        title_width,
        title_size.1,
    )?;

    if rows.is_empty() {
        draw_cell(
            "No configured shortcuts",
            None,
            padding,
            padding * 2 + title_size.1,
            width - padding * 2,
            row_height,
        )?;
    }
    for (index, row) in rows[visible.clone()].iter().enumerate() {
        let column = index / rows_per_column;
        let row_index = index % rows_per_column;
        let x = padding + column as i32 * (column_width + column_gap);
        let y = padding * 2 + title_size.1 + row_index as i32 * (row_height + line_interval);
        layout.set_height(-2);
        layout.set_wrap(WrapMode::WordChar);
        layout.set_ellipsize(EllipsizeMode::Middle);
        draw_cell(&row.key, Some(&key_attrs), x, y, key_width, row_height)?;
        layout.set_height(-1);
        layout.set_ellipsize(EllipsizeMode::End);
        draw_cell(
            &row.action,
            row.attributes.as_ref(),
            x + key_width + padding,
            y,
            action_width,
            row_height,
        )?;
    }
    draw_cell(
        &footer.status,
        None,
        padding,
        footer.top,
        footer.width,
        line_height,
    )?;
    let disabled_key_attrs = AttrList::new();
    disabled_key_attrs.insert(AttrString::new_family("Monospace"));
    disabled_key_attrs.insert(AttrColor::new_background(12000, 12000, 12000));
    disabled_key_attrs.insert(AttrColor::new_foreground(30000, 30000, 30000));
    for cell in &footer.navigation.cells {
        let attrs = if cell.key.is_enabled(page, page_count) {
            &key_attrs
        } else {
            &disabled_key_attrs
        };
        draw_cell(
            cell.key.label(),
            Some(attrs),
            padding + cell.x,
            footer.navigation_top + cell.y,
            cell.width,
            cell.height,
        )?;
    }

    cr.rectangle(0., 0., width.into(), height.into());
    cr.set_source_rgb(PRIMARY.0, PRIMARY.1, PRIMARY.2);
    // Keep the border width even to avoid blurry edges.
    cr.set_line_width((f64::from(BORDER) / 2. * scale).round() * 2.);
    cr.stroke()?;
    drop(layout);
    drop(cr);

    Ok(OverlaySurface {
        surface,
        page_count,
        #[cfg(test)]
        visible,
        #[cfg(test)]
        footer,
    })
}

/// Use the same parsed text and attributes for measurement and painting.
fn parse_action_markup(action: &str) -> (Option<AttrList>, String) {
    match pango::parse_markup(action, '\0') {
        Ok((attrs, text, _)) => (Some(attrs), text.into()),
        Err(_) => (None, action.into()),
    }
}

fn action_name(action: &Action) -> String {
    match action {
        Action::Quit(_) => String::from("Exit clarion"),
        Action::ShowHotkeyOverlay => String::from("Show Important Hotkeys"),
        Action::CloseWindow => String::from("Close Focused Window"),
        Action::FocusColumnLeft => String::from("Focus Column to the Left"),
        Action::FocusColumnRight => String::from("Focus Column to the Right"),
        Action::MoveColumnLeft => String::from("Move Column Left"),
        Action::MoveColumnRight => String::from("Move Column Right"),
        Action::FocusWorkspaceDown => String::from("Switch Workspace Down"),
        Action::FocusWorkspaceUp => String::from("Switch Workspace Up"),
        Action::MoveColumnToWorkspaceDown(_) => String::from("Move Column to Workspace Down"),
        Action::MoveColumnToWorkspaceUp(_) => String::from("Move Column to Workspace Up"),
        Action::MoveWindowToWorkspaceDown(_) => String::from("Move Window to Workspace Down"),
        Action::MoveWindowToWorkspaceUp(_) => String::from("Move Window to Workspace Up"),
        Action::SwitchPresetColumnWidth => String::from("Switch Preset Column Widths"),
        Action::MaximizeColumn => String::from("Maximize Column"),
        Action::ConsumeOrExpelWindowLeft => String::from("Consume or Expel Window Left"),
        Action::ConsumeOrExpelWindowRight => String::from("Consume or Expel Window Right"),
        Action::ToggleWindowFloating => String::from("Move Window Between Floating and Tiling"),
        Action::SwitchFocusBetweenFloatingAndTiling => {
            String::from("Switch Focus Between Floating and Tiling")
        }
        Action::ToggleOverview => String::from("Open the Overview"),
        Action::ToggleExpo => String::from("Toggle Expo 2D Grid Wall"),
        Action::CloseExpo => String::from("Close Expo Grid Wall"),
        Action::ExpoConfirm => String::from("Confirm Expo Workspace Selection"),
        Action::ExpoNavigateLeft => String::from("Expo Move Left"),
        Action::ExpoNavigateRight => String::from("Expo Move Right"),
        Action::ExpoNavigateUp => String::from("Expo Move Up"),
        Action::ExpoNavigateDown => String::from("Expo Move Down"),
        Action::ToggleScratchpad => String::from("Toggle Scratchpad Workspace"),
        Action::MoveWindowToScratchpad => String::from("Move Window to/from Scratchpad"),
        Action::Screenshot(_, _) => String::from("Take a Screenshot"),
        Action::Spawn(args) => format!(
            "Spawn <span face='monospace' bgcolor='#000000'>{}</span>",
            pango::glib::markup_escape_text(args.first().map(String::as_str).unwrap_or(""))
        ),
        Action::SpawnSh(command) => format!(
            "Spawn <span face='monospace' bgcolor='#000000'>{}</span>",
            // Fairly crude but should get the job done in most cases.
            pango::glib::markup_escape_text(command.split_ascii_whitespace().next().unwrap_or(""))
        ),
        _ => default_action_name(action),
    }
}

/// Derive a readable fallback from the enum's variant name, without formatting its payload.
/// This automatically covers new actions and never exposes command/session parameters.
fn default_action_name(action: &Action) -> String {
    struct VariantName(String);
    impl fmt::Write for VariantName {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            let end = text.find(['(', '{', ' ']).unwrap_or(text.len());
            self.0.push_str(&text[..end]);
            if end < text.len() {
                Err(fmt::Error)
            } else {
                Ok(())
            }
        }
    }

    let mut variant = VariantName(String::new());
    let _ = fmt::write(&mut variant, format_args!("{action:?}"));
    let chars = variant.0.chars().collect::<Vec<_>>();
    let mut name = String::new();
    for (index, ch) in chars.iter().copied().enumerate() {
        if index > 0
            && ch.is_uppercase()
            && (!chars[index - 1].is_uppercase()
                || chars.get(index + 1).is_some_and(|next| next.is_lowercase()))
        {
            name.push(' ');
        }
        name.push(ch);
    }
    name
}

fn key_name(screen_reader: bool, mod_key: ModKey, key: &Key) -> String {
    let mut name = String::new();

    let has_comp_mod = key.modifiers.contains(Modifiers::COMPOSITOR);

    // Compositor mod goes first.
    if has_comp_mod {
        match mod_key {
            ModKey::Super => {
                name.push_str("Super + ");
            }
            ModKey::Alt => {
                name.push_str("Alt + ");
            }
            ModKey::Shift => {
                name.push_str("Shift + ");
            }
            ModKey::Ctrl => {
                name.push_str("Ctrl + ");
            }
            ModKey::IsoLevel3Shift => {
                name.push_str("Mod5 + ");
            }
            ModKey::IsoLevel5Shift => {
                name.push_str("Mod3 + ");
            }
        }
    }

    if key.modifiers.contains(Modifiers::SUPER) && !(has_comp_mod && mod_key == ModKey::Super) {
        name.push_str("Super + ");
    }
    if key.modifiers.contains(Modifiers::CTRL) && !(has_comp_mod && mod_key == ModKey::Ctrl) {
        name.push_str("Ctrl + ");
    }
    if key.modifiers.contains(Modifiers::SHIFT) && !(has_comp_mod && mod_key == ModKey::Shift) {
        name.push_str("Shift + ");
    }
    if key.modifiers.contains(Modifiers::ALT) && !(has_comp_mod && mod_key == ModKey::Alt) {
        name.push_str("Alt + ");
    }
    if key.modifiers.contains(Modifiers::ISO_LEVEL3_SHIFT)
        && !(has_comp_mod && mod_key == ModKey::IsoLevel3Shift)
    {
        name.push_str("Mod5 + ");
    }
    if key.modifiers.contains(Modifiers::ISO_LEVEL5_SHIFT)
        && !(has_comp_mod && mod_key == ModKey::IsoLevel5Shift)
    {
        name.push_str("Mod3 + ");
    }

    let pretty = match key.trigger {
        Trigger::Keysym(keysym) => prettify_keysym_name(screen_reader, &keysym_get_name(keysym)),
        Trigger::MouseLeft => String::from("Mouse Left"),
        Trigger::MouseRight => String::from("Mouse Right"),
        Trigger::MouseMiddle => String::from("Mouse Middle"),
        Trigger::MouseBack => String::from("Mouse Back"),
        Trigger::MouseForward => String::from("Mouse Forward"),
        Trigger::WheelScrollDown => String::from("Wheel Scroll Down"),
        Trigger::WheelScrollUp => String::from("Wheel Scroll Up"),
        Trigger::WheelScrollLeft => String::from("Wheel Scroll Left"),
        Trigger::WheelScrollRight => String::from("Wheel Scroll Right"),
        Trigger::TouchpadScrollDown => String::from("Touchpad Scroll Down"),
        Trigger::TouchpadScrollUp => String::from("Touchpad Scroll Up"),
        Trigger::TouchpadScrollLeft => String::from("Touchpad Scroll Left"),
        Trigger::TouchpadScrollRight => String::from("Touchpad Scroll Right"),
        Trigger::TabletStylusButton1 => String::from("Tablet Stylus Button 1"),
        Trigger::TabletStylusButton2 => String::from("Tablet Stylus Button 2"),
        Trigger::TabletStylusButton3 => String::from("Tablet Stylus Button 3"),
    };
    name.push_str(&pretty);

    name
}

fn prettify_keysym_name(screen_reader: bool, name: &str) -> String {
    let name = if screen_reader {
        name
    } else {
        match name {
            "slash" => "/",
            "comma" => ",",
            "period" => ".",
            "minus" => "-",
            "equal" => "=",
            "grave" => "`",
            "bracketleft" => "[",
            "bracketright" => "]",
            "adiaeresis" => "Ä",
            "ediaeresis" => "Ë",
            "idiaeresis" => "Ï",
            "odiaeresis" => "Ö",
            "udiaeresis" => "Ü",
            "ydiaeresis" => "Ÿ",
            "wdiaeresis" => "Ẅ",
            _ => name,
        }
    };

    let name = match name {
        "Next" => "Page Down",
        "Prior" => "Page Up",
        "Print" => "PrtSc",
        "Return" => "Enter",
        "space" => "Space",
        _ => name,
    };

    if name.len() == 1 && name.is_ascii() {
        name.to_ascii_uppercase()
    } else {
        name.into()
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::*;

    #[track_caller]
    fn check(config: &str, action: Action) -> String {
        let config = Config::parse_mem(config).unwrap();
        if let Some((key, title)) = format_bind(&config.binds.0, &action) {
            let key = key.map(|key| key_name(false, ModKey::Super, &key));
            let key = key.as_deref().unwrap_or("(not bound)");
            format!(" {key} : {title}")
        } else {
            String::from("None")
        }
    }

    #[test]
    fn fallback_names_cover_new_actions_without_payloads() {
        assert_eq!(action_name(&Action::ToggleCanvas), "Toggle Canvas");
        assert_eq!(action_name(&Action::PanCanvas(-200, 100)), "Pan Canvas");
        assert_eq!(
            action_name(&Action::ToggleWindowSpread),
            "Toggle Window Spread"
        );
        assert_eq!(action_name(&Action::UndoLayout), "Undo Layout");
        assert_eq!(action_name(&Action::RedoLayout), "Redo Layout");
        assert_eq!(
            action_name(&Action::ToggleWorkspaceCube),
            "Toggle Workspace Cube"
        );
        assert_eq!(
            action_name(&Action::ToggleDwindleSplit),
            "Toggle Dwindle Split"
        );
        // Struct, string, vector and numeric payloads must not reach the label.
        assert_eq!(
            action_name(&Action::RestoreSessionLayout {
                json: "private-session-data".repeat(10_000),
            }),
            "Restore Session Layout"
        );
        assert_eq!(
            action_name(&Action::SetWindowGroup {
                name: "private-group".into(),
                windows: vec![42, 43],
            }),
            "Set Window Group"
        );
        assert_eq!(
            action_name(&Action::FocusMonitor("private-monitor".into())),
            "Focus Monitor"
        );
        assert_eq!(
            action_name(&Action::CloseWindowById(123)),
            "Close Window By Id"
        );
    }

    #[test]
    fn command_labels_escape_markup_and_keep_only_program() {
        let executable = "app<&\"'>";
        for action in [
            Action::Spawn(vec![executable.into(), "secret-argument".into()]),
            Action::SpawnSh(format!("{executable} secret-argument")),
        ] {
            let label = action_name(&action);
            let (attributes, text) = parse_action_markup(&label);
            assert!(
                attributes.is_some(),
                "generated markup should be valid: {label}"
            );
            assert_eq!(text, format!("Spawn {executable}"));
            assert!(!label.contains("secret-argument"));
        }
    }

    #[test]
    fn configured_actions_are_discoverable_and_opt_out_still_wins() {
        let config = Config::parse_mem(
            r#"hotkey-overlay { hide-not-bound; }
            binds {
                Mod+C { toggle-canvas; }
                Mod+R { reset-canvas; }
                Mod+Shift+C { toggle-canvas; }
                Mod+S hotkey-overlay-title=null { toggle-window-spread; }
                F10 { redo-layout; }
                Mod+WheelScrollDown { undo-layout; }
                Mod+MouseLeft hotkey-overlay-title="My undo" { undo-layout; }
                Mod+Shift+Down { move-window-to-workspace-down focus=false; }
            }"#,
        )
        .unwrap();
        let actions = collect_actions(&config);
        assert_eq!(
            actions
                .iter()
                .filter(|&&a| *a == Action::ToggleCanvas)
                .count(),
            1
        );
        assert!(actions.contains(&&Action::ResetCanvas));
        assert!(actions.contains(&&Action::MoveWindowToWorkspaceDown(false)));
        assert!(!actions.contains(&&Action::RedoLayout));
        let visible = actions
            .into_iter()
            .filter_map(|action| format_bind(&config.binds.0, action))
            .collect::<Vec<_>>();
        assert!(visible.iter().any(|(_, title)| title == "My undo"));
        assert!(visible
            .iter()
            .all(|(_, title)| title != "Toggle Window Spread"));
        assert!(visible.iter().all(|(key, _)| key.is_some()));
    }

    fn long_config() -> Config {
        let mut config = Config::parse_mem(r#"hotkey-overlay { hide-not-bound; }"#).unwrap();
        for index in 0..100 {
            config.binds.0.push(Bind {
                key: Key {
                    trigger: Trigger::Keysym(Keysym::F1),
                    modifiers: Modifiers::COMPOSITOR | Modifiers::CTRL | Modifiers::SHIFT,
                },
                action: Action::Spawn(vec![format!("app-{index}")]),
                repeat: true,
                cooldown: None,
                allow_when_locked: false,
                allow_inhibiting: true,
                hotkey_overlay_title: Some(Some(match index % 3 {
                    0 => format!("<b>Action {index}</b> with a very long title that should stay within the output"),
                    1 => format!("Action {index} <broken & markup"),
                    _ => format!("Action {index} · 中文快捷键 · ").repeat(8),
                })),
            });
        }
        config
    }

    #[test]
    fn pagination_keeps_panel_position_on_the_last_page() {
        let mut config = long_config();
        config.binds.0.truncate(97);
        for (size, scale) in [((320, 240), 1.), ((640, 480), 2.), ((360, 640), 1.)] {
            let first = render_surface(&config, ModKey::Super, scale, size, 0).unwrap();
            let last =
                render_surface(&config, ModKey::Super, scale, size, first.page_count - 1).unwrap();
            assert!(last.visible.len() < first.visible.len());
            assert_eq!(last.surface.width(), first.surface.width());
            assert_eq!(
                last.surface.height(),
                first.surface.height(),
                "the panel and its footer must not move on the last page: {size:?}, scale={scale}"
            );
            assert_eq!(last.footer.top, first.footer.top);
            save_preview(&first.surface, size, scale, 0, "position");
            save_preview(&last.surface, size, scale, last.page_count - 1, "position");
        }
    }

    #[test]
    fn footer_keeps_page_position_and_complete_navigation_inside_small_outputs() {
        let config = long_config();
        for (size, scale) in [
            ((240, 320), 1.),
            ((320, 240), 1.),
            ((320, 240), 1.5),
            ((480, 360), 1.5),
            ((640, 480), 2.),
            ((720, 1280), 2.),
            ((1920, 1080), 1.25),
        ] {
            let first = render_surface(&config, ModKey::Super, scale, size, 0).unwrap();
            for page in [0, first.page_count / 2, first.page_count - 1] {
                let rendered = render_surface(&config, ModKey::Super, scale, size, page).unwrap();
                assert!(rendered.footer.status.contains(&format!(
                    "{}/{}",
                    page + 1,
                    first.page_count
                )));
                assert_eq!(rendered.footer.top, first.footer.top);
                assert_eq!(rendered.surface.height(), first.surface.height());
                assert!(rendered.surface.height() <= size.1);
                assert_eq!(rendered.footer.navigation.cells.len(), 5);
                let enabled = rendered
                    .footer
                    .navigation
                    .cells
                    .iter()
                    .map(|cell| cell.key.is_enabled(page, first.page_count))
                    .collect::<Vec<_>>();
                assert_eq!(
                    enabled,
                    if page == 0 {
                        [false, true, false, true, true]
                    } else if page + 1 == first.page_count {
                        [true, false, true, false, true]
                    } else {
                        [true; 5]
                    }
                );
                let cells = &rendered.footer.navigation.cells;
                for cell in cells {
                    assert!(cell.x >= 0 && cell.y >= 0);
                    assert!(cell.x + cell.width <= rendered.footer.width);
                    assert!(cell.y + cell.height <= rendered.footer.navigation.height);
                    assert!(
                        rendered.footer.navigation_top + cell.y + cell.height
                            < rendered.surface.height()
                    );
                }
                for (index, cell) in cells.iter().enumerate() {
                    for other in &cells[index + 1..] {
                        assert!(
                            cell.x + cell.width <= other.x
                                || other.x + other.width <= cell.x
                                || cell.y + cell.height <= other.y
                                || other.y + other.height <= cell.y,
                            "navigation labels overlap: {size:?}, scale={scale}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn long_lists_fit_small_fractional_and_wide_outputs_without_losing_rows() {
        let config = long_config();
        for (target_size, scale) in [
            ((240, 320), 1.),
            ((320, 240), 1.),
            ((320, 240), 1.5),
            ((480, 360), 1.5),
            ((640, 480), 2.),
            ((1920, 1080), 1.25),
            ((1200, 1920), 1.5),
        ] {
            let first = render_surface(&config, ModKey::Super, scale, target_size, 0).unwrap();
            assert!(first.page_count > 1);
            let mut next_row = 0;
            for page in 0..first.page_count {
                let rendered =
                    render_surface(&config, ModKey::Super, scale, target_size, page).unwrap();
                assert!(rendered.surface.width() <= target_size.0);
                assert!(rendered.surface.height() <= target_size.1);
                assert_eq!(rendered.visible.start, next_row);
                assert!(!rendered.visible.is_empty());
                next_row = rendered.visible.end;
                if page == 0 || page + 1 == first.page_count {
                    save_preview(&rendered.surface, target_size, scale, page, "long");
                }
            }
            assert_eq!(next_row, 100, "all configured rows must remain reachable");
            let clamped =
                render_surface(&config, ModKey::Super, scale, target_size, usize::MAX).unwrap();
            assert_eq!(clamped.visible.end, 100);
        }
    }

    fn save_preview(surface: &ImageSurface, size: (i32, i32), scale: f64, page: usize, kind: &str) {
        let Some(directory) = std::env::var_os("CLARION_HOTKEY_TEST_PREVIEW") else {
            return;
        };
        let mut rgba = Vec::new();
        surface
            .with_data(|data| {
                for row in data
                    .chunks_exact(surface.stride() as usize)
                    .take(surface.height() as usize)
                {
                    for pixel in row[..surface.width() as usize * 4].as_chunks::<4>().0 {
                        rgba.extend([pixel[2], pixel[1], pixel[0], pixel[3]]);
                    }
                }
            })
            .unwrap();
        let path = std::path::Path::new(&directory).join(format!(
            "supreme-hotkeys-{kind}-{}x{}-{scale}-page-{page}.png",
            size.0, size.1
        ));
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = png::Encoder::new(file, surface.width() as u32, surface.height() as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&rgba)
            .unwrap();
    }

    #[test]
    #[ignore = "requires real DRM render node, Vulkan and GLES reference"]
    fn native_paginated_hotkeys_match_gles_after_navigation_and_resize() {
        use smithay::backend::renderer::element::{Element, Id};
        use smithay::backend::renderer::gles::GlesRenderer;
        use smithay::output::{Mode, PhysicalProperties, Scale, Subpixel};

        use crate::render_helpers::render_to_vec;
        use crate::render_helpers::vulkan::hardware_tests::setup;
        use crate::render_helpers::vulkan::native::NativeRenderer;

        fn compare(
            hotkeys: &HotkeyOverlay,
            native: &mut NativeRenderer,
            gles: &mut GlesRenderer,
            output: &Output,
            size: (i32, i32),
            scale: f64,
        ) -> (Id, Vec<u8>) {
            let actual = hotkeys.render_with(native, output).unwrap();
            let cached = hotkeys.render_with(native, output).unwrap();
            assert_eq!(
                actual.id(),
                cached.id(),
                "unchanged page reuses its texture"
            );
            let geometry = actual.geometry(scale.into());
            assert!(geometry.loc.x >= 0 && geometry.loc.y >= 0);
            assert!(geometry.loc.x + geometry.size.w <= size.0);
            assert!(geometry.loc.y + geometry.size.h <= size.1);
            let expected = hotkeys.render_with(gles, output).unwrap();
            let actual_pixels = render_to_vec(
                native,
                size.into(),
                scale.into(),
                Transform::Normal,
                Fourcc::Abgr8888,
                [&actual].into_iter(),
            )
            .unwrap();
            let expected_pixels = render_to_vec(
                gles,
                size.into(),
                scale.into(),
                Transform::Normal,
                Fourcc::Abgr8888,
                [&expected].into_iter(),
            )
            .unwrap();
            assert!(actual_pixels
                .iter()
                .zip(&expected_pixels)
                .all(|(a, b)| a.abs_diff(*b) <= 2));
            assert!(actual_pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] > 100));
            if let Some(directory) = std::env::var_os("CLARION_HOTKEY_TEST_PREVIEW") {
                let path = std::path::Path::new(&directory).join(format!(
                    "supreme-hotkeys-gpu-{}x{}-{scale}-page-{}.png",
                    size.0, size.1, hotkeys.page,
                ));
                let file = std::fs::File::create(path).unwrap();
                let mut encoder = png::Encoder::new(file, size.0 as u32, size.1 as u32);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                encoder
                    .write_header()
                    .unwrap()
                    .write_image_data(&actual_pixels)
                    .unwrap();
            }
            (actual.id().clone(), actual_pixels)
        }

        let (vk, mut gles, _) = setup();
        let mut native = NativeRenderer::new(vk).unwrap();
        let config = Rc::new(RefCell::new(long_config()));
        let mut hotkeys = HotkeyOverlay::new(config, ModKey::Super);
        let output = Output::new(
            "paginated-hotkeys".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "test".into(),
                model: "test".into(),
                serial_number: "test".into(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (480, 360).into(),
                refresh: 60_000,
            }),
            Some(Transform::Normal),
            Some(Scale::Fractional(1.5)),
            None,
        );
        hotkeys.show();
        let (first_id, first) = compare(&hotkeys, &mut native, &mut gles, &output, (480, 360), 1.5);
        assert!(hotkeys.handle_key(Keysym::Next, Modifiers::empty()));
        let (next_id, next) = compare(&hotkeys, &mut native, &mut gles, &output, (480, 360), 1.5);
        assert_ne!(first_id, next_id, "navigation invalidates cached textures");
        assert_ne!(first, next, "navigation paints different action rows");
        hotkeys.handle_key(Keysym::End, Modifiers::empty());
        compare(&hotkeys, &mut native, &mut gles, &output, (480, 360), 1.5);
        // Same output and renderer identities: mode/scale changes must replace the texture.
        output.change_current_state(
            Some(Mode {
                size: (1600, 1000).into(),
                refresh: 60_000,
            }),
            Some(Transform::Normal),
            Some(Scale::Fractional(1.)),
            None,
        );
        let (resized_id, _) = compare(&hotkeys, &mut native, &mut gles, &output, (1600, 1000), 1.);
        assert_ne!(next_id, resized_id);
        assert_eq!(hotkeys.page_counts.borrow()[&output.downgrade()], 2);
        // Rotated output bounds, with a fractional scale and a stale larger page index.
        output.change_current_state(
            Some(Mode {
                size: (360, 480).into(),
                refresh: 60_000,
            }),
            Some(Transform::_90),
            Some(Scale::Fractional(1.25)),
            None,
        );
        compare(&hotkeys, &mut native, &mut gles, &output, (480, 360), 1.25);
        assert!(hotkeys.show());
        assert_eq!(hotkeys.page, 0);
        compare(&hotkeys, &mut native, &mut gles, &output, (480, 360), 1.25);
        // Keep the whole centered panel stationary, including a partial final
        // page, on narrow, short high-DPI and portrait outputs.
        for (size, scale) in [((240, 320), 1.), ((320, 240), 1.5), ((720, 1280), 2.)] {
            output.change_current_state(
                Some(Mode {
                    size: size.into(),
                    refresh: 60_000,
                }),
                Some(Transform::Normal),
                Some(Scale::Fractional(scale)),
                None,
            );
            hotkeys.show();
            let (_, first) = compare(&hotkeys, &mut native, &mut gles, &output, size, scale);
            let first_geometry = hotkeys
                .render_with(&mut native, &output)
                .unwrap()
                .geometry(scale.into());
            hotkeys.handle_key(Keysym::End, Modifiers::empty());
            let (_, last) = compare(&hotkeys, &mut native, &mut gles, &output, size, scale);
            let last_geometry = hotkeys
                .render_with(&mut native, &output)
                .unwrap()
                .geometry(scale.into());
            assert_eq!(
                first_geometry, last_geometry,
                "page layout moves: {size:?}, scale={scale}"
            );
            assert_ne!(first, last);
        }
        hotkeys.hide();
        assert!(hotkeys.render_with(&mut native, &output).is_none());
        assert!(hotkeys.render_with(&mut gles, &output).is_none());
        eprintln!("PASS: hotkey pagination, small outputs, resize/rotation/scale, cache and GLES/Vulkan parity");
    }

    #[test]
    fn empty_and_invalid_markup_remain_renderable() {
        let empty = Config::parse_mem(r#"hotkey-overlay { hide-not-bound; }"#).unwrap();
        let rendered = render_surface(&empty, ModKey::Super, 1.5, (480, 360), 0).unwrap();
        assert_eq!(rendered.page_count, 1);
        assert!(rendered.visible.is_empty());
        assert!(rendered.surface.width() <= 480 && rendered.surface.height() <= 360);
        assert_eq!(parse_action_markup("<b>Styled</b>").1, "Styled");
        let invalid = "<b>broken & text";
        let (attrs, text) = parse_action_markup(invalid);
        assert!(attrs.is_none());
        assert_eq!(text, invalid);
        assert!(render_surface(&empty, ModKey::Super, 1., (1, 1), 0).is_err());
    }

    #[test]
    fn page_navigation_reset_hide_and_configuration_changes() {
        use smithay::output::{PhysicalProperties, Subpixel};
        let output = Output::new(
            "pages".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "test".into(),
                model: "test".into(),
                serial_number: "test".into(),
            },
        );
        let mut overlay = HotkeyOverlay::new(Default::default(), ModKey::Super);
        assert!(!overlay.handle_key(Keysym::Next, Modifiers::empty()));
        overlay.show();
        overlay.page_counts.get_mut().insert(output.downgrade(), 4);
        assert!(overlay.handle_key(Keysym::Next, Modifiers::empty()));
        assert_eq!(overlay.page, 1);
        assert!(overlay.handle_key(Keysym::End, Modifiers::empty()));
        assert_eq!(overlay.page, 3);
        assert!(overlay.handle_key(Keysym::Next, Modifiers::empty()));
        assert_eq!(overlay.page, 3);
        // A resize can reduce the number of pages; the next input reclamps it.
        overlay.page_counts.get_mut().insert(output.downgrade(), 2);
        assert!(overlay.handle_key(Keysym::Prior, Modifiers::empty()));
        assert_eq!(overlay.page, 0);
        assert!(overlay.handle_key(Keysym::Next, Modifiers::empty()));
        assert!(overlay.show(), "repeated show starts at the first page");
        assert_eq!(overlay.page, 0);
        overlay.page_counts.get_mut().insert(output.downgrade(), 4);
        overlay.handle_key(Keysym::End, Modifiers::empty());
        overlay.on_hotkey_config_updated(ModKey::Alt);
        assert_eq!(overlay.page, 0);
        assert!(overlay.handle_key(Keysym::Escape, Modifiers::empty()));
        assert!(!overlay.is_open());
    }

    #[test]
    fn test_format_bind() {
        // Not bound.
        assert_snapshot!(check("", Action::Screenshot(true, None)), @" (not bound) : Take a Screenshot");

        // Bound with a default title.
        assert_snapshot!(
            check(
                r#"binds {
                    Mod+P { screenshot; }
                }"#,
                Action::Screenshot(true, None),
            ),
            @" Super + P : Take a Screenshot"
        );

        // Custom title.
        assert_snapshot!(
            check(
                r#"binds {
                    Mod+P hotkey-overlay-title="Hello" { screenshot; }
                }"#,
                Action::Screenshot(true, None),
            ),
            @" Super + P : Hello"
        );

        // Prefer first bind.
        assert_snapshot!(
            check(
                r#"binds {
                    Mod+P { screenshot; }
                    Print { screenshot; }
                }"#,
                Action::Screenshot(true, None),
            ),
            @" Super + P : Take a Screenshot"
        );

        // Prefer bind with custom title.
        assert_snapshot!(
            check(
                r#"binds {
                    Mod+P { screenshot; }
                    Print hotkey-overlay-title="My Cool Bind" { screenshot; }
                }"#,
                Action::Screenshot(true, None),
            ),
            @" PrtSc : My Cool Bind"
        );

        // Prefer first bind with custom title.
        assert_snapshot!(
            check(
                r#"binds {
                    Mod+P hotkey-overlay-title="First" { screenshot; }
                    Print hotkey-overlay-title="My Cool Bind" { screenshot; }
                }"#,
                Action::Screenshot(true, None),
            ),
            @" Super + P : First"
        );

        // Any bind with null title hides it.
        assert_snapshot!(
            check(
                r#"binds {
                    Mod+P { screenshot; }
                    Print hotkey-overlay-title=null { screenshot; }
                }"#,
                Action::Screenshot(true, None),
            ),
            @"None"
        );

        // Custom title takes preference over null.
        assert_snapshot!(
            check(
                r#"binds {
                    Mod+P hotkey-overlay-title="Hello" { screenshot; }
                    Print hotkey-overlay-title=null { screenshot; }
                }"#,
                Action::Screenshot(true, None),
            ),
            @" Super + P : Hello"
        );
    }
}
