//! Immutable output composition policy. Device buffers and surface imports are
//! supplied by the platform after the pass order and privacy route are frozen.

#[derive(Debug, Clone, Copy, Default)]
pub struct SceneInputs {
    pub pointer: bool,
    pub screen_transition: bool,
    pub locked: bool,
    pub screenshot_ui: bool,
    pub desktop_above_top: bool,
    pub stage: bool,
    pub overview_backdrop: bool,
    pub explicit_backdrop_wallpaper: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenePass {
    Pointer,
    ScreenTransition,
    ExitDialog,
    ConfigNotification,
    LockSurface,
    LockBackdrop,
    ScreenshotUi,
    HotkeyOverlay,
    WindowSwitcher,
    OverlayLayer,
    TopLayer,
    MovingWindow,
    InsertHint,
    Workspaces,
    LowerLayerPopups,
    LowerLayers,
    WorkspaceBackground,
    WorkspaceLayerPopups,
    StageWithLayers,
    WorkspaceLowerLayers,
    WorkspaceShadows,
    OverviewShade,
    BackdropLayers,
    WallpaperFallback,
    Backdrop,
}

/// Front-to-back passes; all stored inline and immutable once constructed.
#[derive(Debug, Clone)]
pub struct ScenePlan {
    passes: [ScenePass; 32],
    len: usize,
}

impl ScenePlan {
    pub fn new(inputs: SceneInputs) -> Self {
        use ScenePass::*;
        let mut plan = Self {
            passes: [Backdrop; 32],
            len: 0,
        };
        if inputs.pointer {
            plan.push(Pointer);
        }
        if inputs.screen_transition && !inputs.locked {
            plan.push(ScreenTransition);
        }
        plan.push(ExitDialog);
        plan.push(ConfigNotification);
        if inputs.locked {
            plan.push(LockSurface);
            plan.push(LockBackdrop);
            return plan;
        }
        if inputs.screenshot_ui {
            plan.push(ScreenshotUi);
            plan.push(Backdrop);
            return plan;
        }
        plan.push(HotkeyOverlay);
        plan.push(WindowSwitcher);
        plan.push(OverlayLayer);
        if inputs.desktop_above_top {
            plan.push(MovingWindow);
            plan.push(InsertHint);
            plan.push(Workspaces);
            plan.push(TopLayer);
            plan.push(LowerLayerPopups);
            plan.push(LowerLayers);
            plan.push(WorkspaceBackground);
        } else {
            plan.push(TopLayer);
            plan.push(MovingWindow);
            plan.push(InsertHint);
            if inputs.stage {
                plan.push(StageWithLayers);
            } else {
                plan.push(WorkspaceLayerPopups);
                plan.push(Workspaces);
                plan.push(WorkspaceLowerLayers);
            }
        }
        if !inputs.stage {
            plan.push(WorkspaceShadows);
        }
        if inputs.overview_backdrop {
            plan.push(OverviewShade);
        }
        plan.push(BackdropLayers);
        if inputs.overview_backdrop && !inputs.explicit_backdrop_wallpaper {
            plan.push(WallpaperFallback);
        }
        plan.push(Backdrop);
        plan
    }

    fn push(&mut self, pass: ScenePass) {
        self.passes[self.len] = pass;
        self.len += 1;
    }

    pub fn passes(&self) -> &[ScenePass] {
        &self.passes[..self.len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_route_never_contains_desktop_or_previous_screen_content() {
        for flags in 0..256 {
            let plan = ScenePlan::new(SceneInputs {
                locked: true,
                pointer: flags & 1 != 0,
                screen_transition: flags & 2 != 0,
                screenshot_ui: flags & 4 != 0,
                desktop_above_top: flags & 8 != 0,
                stage: flags & 16 != 0,
                overview_backdrop: flags & 32 != 0,
                explicit_backdrop_wallpaper: flags & 64 != 0,
            });
            assert!(plan.passes().iter().all(|p| matches!(
                p,
                ScenePass::Pointer
                    | ScenePass::ExitDialog
                    | ScenePass::ConfigNotification
                    | ScenePass::LockSurface
                    | ScenePass::LockBackdrop
            )));
            assert_eq!(plan.passes().last(), Some(&ScenePass::LockBackdrop));
        }
    }

    #[test]
    fn fullscreen_and_stage_have_one_ordered_workspace_route() {
        use ScenePass::*;
        for above in [false, true] {
            for stage in [false, true] {
                let plan = ScenePlan::new(SceneInputs {
                    desktop_above_top: above,
                    stage,
                    ..Default::default()
                });
                let index = |p| plan.passes().iter().position(|x| *x == p).unwrap();
                assert_eq!(
                    plan.passes()
                        .iter()
                        .filter(|p| matches!(p, Workspaces | StageWithLayers))
                        .count(),
                    1
                );
                if above {
                    assert!(index(Workspaces) < index(TopLayer));
                } else {
                    assert!(index(TopLayer) < index(MovingWindow));
                }
                assert_eq!(plan.passes().contains(&WorkspaceShadows), !stage);
                assert_eq!(plan.passes().last(), Some(&Backdrop));
            }
        }
    }

    #[test]
    fn capture_ui_and_wallpaper_routes_do_not_duplicate_backdrop_elements() {
        use ScenePass::*;
        let ui = ScenePlan::new(SceneInputs {
            screenshot_ui: true,
            stage: true,
            ..Default::default()
        });
        assert_eq!(
            ui.passes(),
            &[ExitDialog, ConfigNotification, ScreenshotUi, Backdrop]
        );
        for explicit in [true, false] {
            let plan = ScenePlan::new(SceneInputs {
                overview_backdrop: true,
                explicit_backdrop_wallpaper: explicit,
                ..Default::default()
            });
            assert_eq!(
                plan.passes()
                    .iter()
                    .filter(|p| **p == BackdropLayers)
                    .count(),
                1
            );
            assert_eq!(plan.passes().contains(&WallpaperFallback), !explicit);
        }
    }
}
