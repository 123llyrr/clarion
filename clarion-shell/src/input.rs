// SPDX-License-Identifier: Apache-2.0
// Adapted from GPUI 0.2.2 examples/input.rs (Zed Industries, Apache-2.0).
// See third_party/gpui/LICENSE-APACHE for the upstream license.
// This single-line input exercises GPUI's real text-input-v3 / IME path.

use std::ops::Range;

use gpui::prelude::*;
use gpui::{
    actions, div, fill, point, px, relative, rgb, rgba, size, App, Bounds, ClipboardItem,
    ContentMask, Context, CursorStyle, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style,
    TextRun, UTF16Selection, UnderlineStyle, Window,
};
use unicode_segmentation::UnicodeSegmentation;

actions!(
    shell_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
    ]
);

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_text_run: Option<TextRun>,
    last_bounds: Option<Bounds<Pixels>>,
    last_ime_coordinates: Option<ImeCoordinates>,
    is_selecting: bool,
    horizontal_offset: Pixels,
    compact: bool,
    light: bool,
    /// A boxless headline field: no border, no fill, larger type. The
    /// launcher sheet uses it as its title line.
    bare: bool,
}

pub struct QueryChanged(pub String);

impl gpui::EventEmitter<QueryChanged> for TextInput {}

/// Backspace arrived at the start of an empty field. The palette listens for
/// this to unwind its session (leave a tool/mode, drop an override) instead of
/// deleting a character that does not exist.
pub struct BackspaceAtStart;

impl gpui::EventEmitter<BackspaceAtStart> for TextInput {}

#[derive(PartialEq)]
struct ImeCoordinates {
    bounds: Bounds<Pixels>,
    content: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    horizontal_offset: Pixels,
    font: gpui::Font,
    font_size: Pixels,
    focused: bool,
}

impl TextInput {
    pub fn set_text(&mut self, text: &str, select_all: bool, cx: &mut Context<Self>) {
        self.content = text.to_owned().into();
        self.selected_range = if select_all {
            0..text.len()
        } else {
            text.len()..text.len()
        };
        self.selection_reversed = false;
        self.marked_range = None;
        self.is_selecting = false;
        self.horizontal_offset = px(0.);
        cx.emit(QueryChanged(text.to_owned()));
        cx.notify();
    }
    pub fn has_preedit(&self) -> bool {
        self.marked_range.is_some()
    }
    pub fn with_placeholder(mut self, placeholder: &str) -> Self {
        self.placeholder = placeholder.to_owned().into();
        self
    }
    pub fn with_compact(mut self, dark: bool) -> Self {
        self.compact = true;
        self.light = !dark;
        self
    }
    /// The boxless headline style. `dark` picks the ink: dark text on a light
    /// surface, light text on a dark one, so the line stays readable under
    /// both themes.
    pub fn with_bare(mut self, dark: bool) -> Self {
        self.bare = true;
        self.light = !dark;
        self
    }
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            placeholder: "搜索应用、窗口与最近内容".into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_text_run: None,
            last_bounds: None,
            last_ime_coordinates: None,
            is_selecting: false,
            horizontal_offset: px(0.),
            compact: false,
            light: false,
            bare: false,
        }
    }

    /// Keep editing bindings inside this field so the parent can own Escape.
    pub fn bind_keys(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, Some("TextInput")),
            KeyBinding::new("delete", Delete, Some("TextInput")),
            KeyBinding::new("left", Left, Some("TextInput")),
            KeyBinding::new("right", Right, Some("TextInput")),
            KeyBinding::new("shift-left", SelectLeft, Some("TextInput")),
            KeyBinding::new("shift-right", SelectRight, Some("TextInput")),
            KeyBinding::new("ctrl-a", SelectAll, Some("TextInput")),
            KeyBinding::new("ctrl-c", Copy, Some("TextInput")),
            KeyBinding::new("ctrl-v", Paste, Some("TextInput")),
            KeyBinding::new("ctrl-x", Cut, Some("TextInput")),
            KeyBinding::new("home", Home, Some("TextInput")),
            KeyBinding::new("end", End, Some("TextInput")),
        ]);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.content.is_empty() && self.marked_range.is_none() {
            cx.emit(BackspaceAtStart);
            return;
        }
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        self.is_selecting = true;

        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace(['\r', '\n', '\t'], " "), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }

        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if line.text != self.content {
            return self.cursor_offset();
        }
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left() + self.horizontal_offset)
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        utf16_offset(&self.content, offset)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        utf8_range(&self.content, range_utf16)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        previous_boundary(&self.content, offset)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        next_boundary(&self.content, offset)
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        self.selection_reversed = false;
        cx.emit(QueryChanged(self.content.to_string()));
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        // IME selections are UTF-16 offsets relative to the new preedit,
        // not to the entire field or to the range being replaced.
        self.selected_range =
            preedit_selection(new_text, new_selected_range_utf16.as_ref(), range.start);
        self.selection_reversed = false;

        cx.emit(QueryChanged(self.content.to_string()));
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let bounds = self.last_bounds.unwrap_or(bounds);
        // text-input-v3 asks for coordinates immediately after changing preedit,
        // before a repaint can refresh last_layout. Shape current content using
        // the field's painted font instead of returning no cursor rectangle.
        let fresh_layout = if last_layout.text != self.content {
            let mut run = self.last_text_run.clone()?;
            run.len = self.content.len();
            Some(window.text_system().shape_line(
                self.content.clone(),
                last_layout.font_size,
                &[run],
                None,
            ))
        } else {
            None
        };
        let layout = fresh_layout.as_ref().unwrap_or(last_layout);
        let horizontal_offset = visible_horizontal_offset(
            layout.width,
            layout.x_for_index(self.cursor_offset()),
            bounds.size.width,
            self.horizontal_offset,
        );
        Some(Bounds::from_corners(
            point(
                bounds.left() + layout.x_for_index(range.start) - horizontal_offset,
                bounds.top(),
            ),
            point(
                bounds.left() + layout.x_for_index(range.end) - horizontal_offset,
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;

        if self.content.is_empty() {
            return Some(0);
        }
        if last_layout.text != self.content {
            return None;
        }
        let utf8_index = last_layout.index_for_x(line_point.x + self.horizontal_offset)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

struct SpikeTextElement {
    input: Entity<TextInput>,
}

struct SpikePrepaintState {
    line: Option<ShapedLine>,
    text_run: TextRun,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    horizontal_offset: Pixels,
}

impl IntoElement for SpikeTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SpikeTextElement {
    type RequestLayoutState = ();
    type PrepaintState = SpikePrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (
                input.placeholder.clone(),
                if input.light {
                    rgba(0x24242a80).into()
                } else {
                    rgba(0xffffffb3).into()
                },
            )
        } else {
            (content, style.color)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let geometry_run = run.clone();
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let cursor_pos = line.x_for_index(cursor);
        let horizontal_offset = visible_horizontal_offset(
            line.width,
            cursor_pos,
            bounds.size.width,
            input.horizontal_offset,
        );
        let text_left = bounds.left() - horizontal_offset;
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(text_left + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    rgb(0xa8ccff),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            text_left + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            text_left + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    rgba(0x78abff55),
                )),
                None,
            )
        };
        SpikePrepaintState {
            line: Some(line),
            text_run: geometry_run,
            cursor,
            selection,
            horizontal_offset,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let line = prepaint.line.take().unwrap();
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(selection) = prepaint.selection.take() {
                window.paint_quad(selection);
            }
            let origin = point(bounds.left() - prepaint.horizontal_offset, bounds.top());
            let _ = line.paint(origin, window.line_height(), window, cx);
            if focus_handle.is_focused(window) {
                if let Some(cursor) = prepaint.cursor.take() {
                    window.paint_quad(cursor);
                }
            }
        });

        let focused = focus_handle.is_focused(window);
        let coordinates_changed = self.input.update(cx, |input, _cx| {
            let coordinates = ImeCoordinates {
                bounds,
                content: input.content.clone(),
                selected_range: input.selected_range.clone(),
                selection_reversed: input.selection_reversed,
                horizontal_offset: prepaint.horizontal_offset,
                font: prepaint.text_run.font.clone(),
                font_size: line.font_size,
                focused,
            };
            let changed = input.last_ime_coordinates.as_ref() != Some(&coordinates);
            input.last_ime_coordinates = Some(coordinates);
            input.last_layout = Some(line);
            input.last_text_run = Some(prepaint.text_run.clone());
            input.last_bounds = Some(bounds);
            input.horizontal_offset = prepaint.horizontal_offset;
            changed
        });
        if focused && coordinates_changed {
            // Also supplies the initial caret rectangle before composition starts.
            // Unchanged paints schedule nothing, so this cannot drive a frame loop.
            window.invalidate_character_coordinates();
        }
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .w_full()
            .key_context("TextInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .rounded(px(if self.compact { 6. } else { 16. }))
            .when(!self.bare, |field| {
                field
                    .border_1()
                    .border_color(rgba(0xffffff24))
                    .bg(if self.compact {
                        rgba(0x00000000)
                    } else {
                        rgba(0x080d1699)
                    })
            })
            .text_color(if self.light {
                rgb(0x24242a)
            } else {
                rgb(0xf4f6ff)
            })
            // The bare style is a headline, not a form control: it draws no
            // box and sets its own larger type (the launcher sheet's title
            // line).
            .line_height(px(if self.bare {
                38.
            } else if self.compact {
                18.
            } else {
                30.
            }))
            .text_size(px(if self.bare {
                22.
            } else if self.compact {
                13.
            } else {
                18.
            }))
            .child(
                div()
                    .h(px(if self.bare {
                        40.
                    } else if self.compact {
                        28.
                    } else {
                        54.
                    }))
                    .w_full()
                    .px(px(if self.bare {
                        0.
                    } else if self.compact {
                        8.
                    } else {
                        18.
                    }))
                    .py(px(if self.bare {
                        0.
                    } else if self.compact {
                        5.
                    } else {
                        12.
                    }))
                    .child(SpikeTextElement { input: cx.entity() }),
            )
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn visible_horizontal_offset(
    line_width: Pixels,
    cursor: Pixels,
    field_width: Pixels,
    previous_offset: Pixels,
) -> Pixels {
    let available_width = (field_width - px(3.)).max(px(0.));
    let max_offset = (line_width - available_width).max(px(0.));
    let offset = previous_offset.min(max_offset);
    if cursor < offset {
        cursor
    } else if cursor > offset + available_width {
        cursor - available_width
    } else {
        offset
    }
}

// Offsets at an invalid half-surrogate are rounded forward to a complete
// scalar; all resulting offsets are safe UTF-8 slice boundaries.
fn utf8_offset(text: &str, offset: usize) -> usize {
    let mut utf8 = 0;
    let mut utf16 = 0;
    for ch in text.chars() {
        if utf16 >= offset {
            break;
        }
        utf16 += ch.len_utf16();
        utf8 += ch.len_utf8();
    }
    utf8
}

fn utf16_offset(text: &str, offset: usize) -> usize {
    let mut utf8 = 0;
    let mut utf16 = 0;
    for ch in text.chars() {
        if utf8 >= offset {
            break;
        }
        utf8 += ch.len_utf8();
        utf16 += ch.len_utf16();
    }
    utf16
}

fn utf8_range(text: &str, range: &Range<usize>) -> Range<usize> {
    let start = utf8_offset(text, range.start);
    let end = utf8_offset(text, range.end);
    start.min(end)..start.max(end)
}

fn preedit_selection(text: &str, selection: Option<&Range<usize>>, start: usize) -> Range<usize> {
    let relative = selection
        .map(|range| utf8_range(text, range))
        .unwrap_or(text.len()..text.len());
    start + relative.start..start + relative.end
}

fn previous_boundary(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .rev()
        .find_map(|(idx, _)| (idx < offset).then_some(idx))
        .unwrap_or(0)
}

fn next_boundary(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .find_map(|(idx, _)| (idx > offset).then_some(idx))
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_ranges_preserve_cjk_and_surrogate_pairs() {
        let text = "中😀文";
        assert_eq!(utf8_range(text, &(1..3)), 3..7);
        assert_eq!(utf16_offset(text, 7), 3);
        assert_eq!(utf8_offset(text, 2), 7);
        assert_eq!(utf8_range(text, &(99..100)), text.len()..text.len());
        for (byte, _) in text.char_indices() {
            assert_eq!(utf8_offset(text, utf16_offset(text, byte)), byte);
        }
    }

    #[test]
    fn preedit_selection_is_relative_to_new_composition_after_prefix() {
        // The prefix is deliberately unrelated to the composition's length.
        let prefix = "已输入一段文字 ";
        let preedit = "你好😀";
        assert_eq!(
            preedit_selection(preedit, Some(&(1..4)), prefix.len()),
            prefix.len() + 3..prefix.len() + preedit.len(),
        );
        assert_eq!(
            preedit_selection(preedit, None, prefix.len()),
            prefix.len() + preedit.len()..prefix.len() + preedit.len(),
        );
    }

    #[test]
    fn fresh_preedit_scrolls_the_caret_into_the_same_field_as_paint() {
        assert_eq!(
            visible_horizontal_offset(px(400.), px(400.), px(100.), px(0.)),
            px(303.),
        );
        assert_eq!(
            visible_horizontal_offset(px(400.), px(20.), px(100.), px(303.)),
            px(20.),
        );
        assert_eq!(
            visible_horizontal_offset(px(40.), px(40.), px(100.), px(303.)),
            px(0.),
        );
    }

    #[test]
    fn cursor_and_backspace_skip_complete_graphemes() {
        let text = "a\u{301}👨‍👩‍👧‍👦中";
        let combining_end = "a\u{301}".len();
        let family_end = text.len() - "中".len();
        assert_eq!(next_boundary(text, 0), combining_end);
        assert_eq!(next_boundary(text, combining_end), family_end);
        assert_eq!(previous_boundary(text, family_end), combining_end);
        assert_eq!(previous_boundary(text, combining_end), 0);
    }
}
