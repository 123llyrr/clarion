use std::collections::HashMap;
use std::time::Duration;

use clarion::animation::Clock;
use clarion::layout::{ActivateWindow, AddWindowTarget, LayoutElement as _, Options, SizingMode};
use clarion::render_helpers::{RenderCtx, RenderTarget};
use clarion_config::{Color, OutputName, PresetSize};
use smithay::backend::renderer::element::RenderElement;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::layer_map_for_output;
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::utils::{Logical, Physical, Point, Size};

use super::{Args, TestCase};
use crate::test_window::TestWindow;

type DynStepFn = Box<dyn FnOnce(&mut Layout)>;

pub struct Layout {
    output: Output,
    windows: Vec<TestWindow>,
    clock: Clock,
    layout: clarion::layout::Layout<TestWindow>,
    start_time: Duration,
    steps: HashMap<Duration, DynStepFn>,
}

impl Layout {
    pub fn new(args: Args) -> Self {
        let Args { size, clock } = args;

        let output = Output::new(
            String::new(),
            PhysicalProperties {
                size: Size::from((size.w, size.h)),
                subpixel: Subpixel::Unknown,
                make: String::new(),
                model: String::new(),
                serial_number: String::new(),
            },
        );
        let mode = Some(Mode {
            size: size.to_physical(1),
            refresh: 60000,
        });
        output.change_current_state(mode, None, None, None);
        output.user_data().insert_if_missing(|| OutputName {
            connector: String::new(),
            make: None,
            model: None,
            serial: None,
        });

        let options = Options {
            layout: clarion_config::Layout {
                focus_ring: clarion_config::FocusRing {
                    off: true,
                    ..Default::default()
                },
                border: clarion_config::Border {
                    off: false,
                    width: 4.,
                    active_color: Color::from_rgba8_unpremul(255, 163, 72, 255),
                    inactive_color: Color::from_rgba8_unpremul(50, 50, 50, 255),
                    urgent_color: Color::from_rgba8_unpremul(155, 0, 0, 255),
                    active_gradient: None,
                    inactive_gradient: None,
                    urgent_gradient: None,
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let mut layout = clarion::layout::Layout::with_options(clock.clone(), options);
        layout.add_output(output.clone(), None);

        let start_time = clock.now_unadjusted();

        Self {
            output,
            windows: Vec::new(),
            clock,
            layout,
            start_time,
            steps: HashMap::new(),
        }
    }

    pub fn spread_search(args: Args) -> Self {
        let mut rv = Self::new(args);
        for id in 0..9 {
            rv.add_window(TestWindow::freeform(id), Some(PresetSize::Proportion(0.6)));
        }
        rv.add_step(500, |l| l.layout.toggle_window_spread());
        rv.add_step(2500, |l| {
            for c in "app4".chars() {
                l.layout
                    .spread_key(smithay::input::keyboard::Keysym::from_char(c));
            }
        });
        rv.add_step(4000, |l| {
            l.layout
                .spread_key(smithay::input::keyboard::Keysym::Return);
        });
        rv
    }

    fn spread_workspace_scene(args: Args) -> Self {
        let mut rv = Self::new(args);
        for name in ["Code", "Chat", "Notes"] {
            rv.layout
                .ensure_named_workspace(&clarion_config::Workspace {
                    name: clarion_config::WorkspaceName(name.into()),
                    open_on_output: None,
                    layout: None,
                });
        }
        for workspace in 0..3 {
            rv.layout.switch_workspace(workspace);
            for id in workspace * 2..workspace * 2 + 2 {
                rv.add_window(TestWindow::freeform(id), Some(PresetSize::Proportion(0.4)));
            }
        }
        rv.layout.activate_window(&0);
        rv.clock.set_complete_instantly(true);
        rv.layout.advance_animations();
        rv.clock.set_complete_instantly(false);
        rv
    }

    pub fn spread_transition_reversal(args: Args) -> Self {
        let mut rv = Self::spread_workspace_scene(args);
        rv.add_step(500, |l| l.layout.toggle_window_spread());
        rv.add_step(625, |l| {
            l.layout
                .spread_key(smithay::input::keyboard::Keysym::Escape);
        });
        rv.add_step(700, |l| l.layout.toggle_window_spread());
        rv.add_step(1500, |l| {
            for ch in "app4".chars() {
                l.layout
                    .spread_key(smithay::input::keyboard::Keysym::from_char(ch));
            }
        });
        rv.add_step(2500, |l| {
            l.layout
                .spread_key(smithay::input::keyboard::Keysym::Return);
        });
        rv
    }

    pub fn spread_cancel_after_navigation(args: Args) -> Self {
        let mut rv = Self::spread_workspace_scene(args);
        rv.add_step(500, |l| l.layout.toggle_window_spread());
        rv.add_step(1500, |l| {
            l.layout.spread_key(smithay::input::keyboard::Keysym::Right);
        });
        rv.add_step(2000, |l| {
            l.layout.spread_key(smithay::input::keyboard::Keysym::Down);
        });
        rv.add_step(2500, |l| {
            l.layout
                .spread_key(smithay::input::keyboard::Keysym::Escape);
        });
        rv
    }

    pub fn expo_arrange(args: Args) -> Self {
        let mut rv = Self::new(args);
        for name in ["Code", "Chat", "Notes"] {
            rv.layout
                .ensure_named_workspace(&clarion_config::Workspace {
                    name: clarion_config::WorkspaceName(name.into()),
                    open_on_output: None,
                    layout: None,
                });
        }
        for idx in 0..3 {
            rv.layout.switch_workspace(idx);
            rv.add_window(TestWindow::freeform(idx), Some(PresetSize::Proportion(0.6)));
        }
        rv.layout.switch_workspace(0);
        rv.add_step(500, |l| l.layout.toggle_expo());
        rv.add_step(1500, |l| {
            let mon = l.layout.monitors().next().unwrap();
            let cells: Vec<_> = mon.workspaces_render_geo().take(2).collect();
            let center = |geo: smithay::utils::Rectangle<f64, Logical>| {
                geo.loc + Point::from((geo.size.w / 2., geo.size.h / 2.))
            };
            let start = center(cells[0]);
            let end = center(cells[1]);
            l.layout.interactive_move_begin(0, &l.output, start);
            l.layout
                .interactive_move_update(&0, Point::from((500., 100.)), l.output.clone(), end);
        });
        rv.add_step(2500, |l| l.layout.interactive_move_end(&0));
        rv
    }

    pub fn open_in_between(args: Args) -> Self {
        let mut rv = Self::new(args);

        rv.add_window(TestWindow::freeform(0), Some(PresetSize::Proportion(0.3)));
        rv.add_window(TestWindow::freeform(1), Some(PresetSize::Proportion(0.3)));
        rv.layout.activate_window(&0);

        rv.add_step(500, |l| {
            let win = TestWindow::freeform(2);
            l.add_window(win.clone(), Some(PresetSize::Proportion(0.3)));
            l.layout.start_open_animation_for_window(win.id());
        });

        rv
    }

    pub fn open_multiple_quickly(args: Args) -> Self {
        let mut rv = Self::new(args);

        for delay in [100, 200, 300] {
            rv.add_step(delay, move |l| {
                let win = TestWindow::freeform(delay as usize);
                l.add_window(win.clone(), Some(PresetSize::Proportion(0.3)));
                l.layout.start_open_animation_for_window(win.id());
            });
        }

        rv
    }

    pub fn open_multiple_quickly_big(args: Args) -> Self {
        let mut rv = Self::new(args);

        for delay in [100, 200, 300] {
            rv.add_step(delay, move |l| {
                let win = TestWindow::freeform(delay as usize);
                l.add_window(win.clone(), Some(PresetSize::Proportion(0.5)));
                l.layout.start_open_animation_for_window(win.id());
            });
        }

        rv
    }

    pub fn open_to_the_left(args: Args) -> Self {
        let mut rv = Self::new(args);

        rv.add_window(TestWindow::freeform(0), Some(PresetSize::Proportion(0.3)));
        rv.add_window(TestWindow::freeform(1), Some(PresetSize::Proportion(0.3)));

        rv.add_step(500, |l| {
            let win = TestWindow::freeform(2);
            let right_of = l.windows[0].clone();
            l.add_window_right_of(&right_of, win.clone(), Some(PresetSize::Proportion(0.3)));
            l.layout.start_open_animation_for_window(win.id());
        });

        rv
    }

    pub fn open_to_the_left_big(args: Args) -> Self {
        let mut rv = Self::new(args);

        rv.add_window(TestWindow::freeform(0), Some(PresetSize::Proportion(0.3)));
        rv.add_window(TestWindow::freeform(1), Some(PresetSize::Proportion(0.8)));

        rv.add_step(500, |l| {
            let win = TestWindow::freeform(2);
            let right_of = l.windows[0].clone();
            l.add_window_right_of(&right_of, win.clone(), Some(PresetSize::Proportion(0.5)));
            l.layout.start_open_animation_for_window(win.id());
        });

        rv
    }

    pub fn workspace_cube(args: Args) -> Self {
        let mut rv = Self::new(args);
        rv.add_window(TestWindow::freeform(0), Some(PresetSize::Proportion(0.6)));
        rv.add_step(500, |layout| layout.layout.switch_workspace_down());
        rv
    }

    pub fn interactive_move_wobble(args: Args) -> Self {
        let mut rv = Self::new(args);
        rv.add_window(TestWindow::freeform(0), Some(PresetSize::Proportion(0.6)));
        rv.add_step(400, |layout| {
            let window = *layout.windows[0].id();
            let start = Point::<f64, Logical>::from((500., 350.));
            assert!(layout
                .layout
                .interactive_move_begin(window, &layout.output, start));
            assert!(layout.layout.interactive_move_update(
                &window,
                Point::from((300., 80.)),
                layout.output.clone(),
                Point::from((800., 430.)),
            ));
        });
        rv.add_step(600, |layout| {
            let window = *layout.windows[0].id();
            assert!(layout.layout.interactive_move_update(
                &window,
                Point::from((120., -100.)),
                layout.output.clone(),
                Point::from((920., 330.)),
            ));
        });
        rv.add_step(900, |layout| {
            let window = *layout.windows[0].id();
            layout.layout.interactive_move_end(&window);
        });
        rv
    }

    fn add_window(&mut self, mut window: TestWindow, width: Option<PresetSize>) {
        let ws = self.layout.active_workspace().unwrap();
        let min_size = window.min_size();
        let max_size = window.max_size();
        window.request_size(
            ws.new_window_size(width, None, false, window.rules(), (min_size, max_size)),
            SizingMode::Normal,
            false,
            None,
        );
        window.communicate();

        self.layout.add_window(
            window.clone(),
            AddWindowTarget::Auto,
            width,
            None,
            false,
            false,
            ActivateWindow::default(),
        );
        self.windows.push(window);
    }

    fn add_window_right_of(
        &mut self,
        right_of: &TestWindow,
        mut window: TestWindow,
        width: Option<PresetSize>,
    ) {
        let ws = self.layout.active_workspace().unwrap();
        let min_size = window.min_size();
        let max_size = window.max_size();
        window.request_size(
            ws.new_window_size(width, None, false, window.rules(), (min_size, max_size)),
            SizingMode::Normal,
            false,
            None,
        );
        window.communicate();

        self.layout.add_window(
            window.clone(),
            AddWindowTarget::NextTo(right_of.id()),
            width,
            None,
            false,
            false,
            ActivateWindow::default(),
        );
        self.windows.push(window);
    }

    fn add_step(&mut self, delay_ms: u64, f: impl FnOnce(&mut Self) + 'static) {
        self.steps
            .insert(Duration::from_millis(delay_ms), Box::new(f) as _);
    }
}

impl TestCase for Layout {
    fn resize(&mut self, width: i32, height: i32) {
        let mode = Some(Mode {
            size: Size::from((width, height)),
            refresh: 60000,
        });
        self.output.change_current_state(mode, None, None, None);
        layer_map_for_output(&self.output).arrange();
        self.layout.update_output_size(&self.output);
        for win in &self.windows {
            if win.communicate() {
                self.layout.update_window(win.id(), None);
            }
        }
    }

    fn are_animations_ongoing(&self) -> bool {
        self.layout.are_animations_ongoing(Some(&self.output)) || !self.steps.is_empty()
    }

    fn advance_animations(&mut self, _current_time: Duration) {
        let now_unadjusted = self.clock.now_unadjusted();
        let mut run = self
            .steps
            .keys()
            .copied()
            .filter(|delay| self.start_time + *delay <= now_unadjusted)
            .collect::<Vec<_>>();
        run.sort_unstable();
        for delay in &run {
            let now = self.start_time + *delay;
            self.clock.set_unadjusted(now);
            self.layout.advance_animations();

            let f = self.steps.remove(delay).unwrap();
            f(self);
        }

        self.clock.set_unadjusted(now_unadjusted);
        self.layout.advance_animations();
    }

    fn render(
        &mut self,
        renderer: &mut GlesRenderer,
        _size: Size<i32, Physical>,
    ) -> Vec<Box<dyn RenderElement<GlesRenderer>>> {
        self.layout.update_render_elements(Some(&self.output));

        let mut rv = Vec::new();
        let mut ctx = RenderCtx {
            renderer,
            target: RenderTarget::Output,
            xray: None,
        };
        self.layout
            .scene_source(&self.output)
            .render_moving(ctx.r(), &mut |elem| {
                rv.push(Box::new(elem) as _);
            });
        self.layout
            .monitor_for_output(&self.output)
            .unwrap()
            .render_workspaces(ctx, true, &mut |elem| rv.push(Box::new(elem) as _));
        rv
    }
}
