use realand_config::layer_rule::{LayerRule, Match};
use realand_config::utils::MergeWith as _;
use realand_config::{
    BackgroundEffect, BlockOutFrom, CornerRadius, ResolvedPopupsRules, ShadowRule,
};
use smithay::desktop::LayerSurface;
use smithay::wayland::shell::wlr_layer::Layer;

pub mod mapped;
pub use mapped::MappedLayer;

/// Rules fully resolved for a layer-shell surface.
#[derive(Debug, Default, PartialEq)]
pub struct ResolvedLayerRules {
    /// Extra opacity to draw this layer surface with.
    pub opacity: Option<f32>,

    /// Whether to block out this layer surface from certain render targets.
    pub block_out_from: Option<BlockOutFrom>,

    /// Shadow overrides.
    pub shadow: ShadowRule,

    /// Corner radius to assume this layer surface has.
    pub geometry_corner_radius: Option<CornerRadius>,

    /// Whether to place this layer surface within the overview backdrop.
    pub place_within_backdrop: bool,

    /// Whether to bob this window up and down.
    pub baba_is_float: bool,

    /// Background effect configuration.
    pub background_effect: BackgroundEffect,

    /// Rules for this layer surface's popups.
    pub popups: ResolvedPopupsRules,
}

impl ResolvedLayerRules {
    pub fn compute(rules: &[LayerRule], surface: &LayerSurface, is_at_startup: bool) -> Self {
        let _span = realand_tracy_span!("ResolvedLayerRules::compute");

        let mut resolved = ResolvedLayerRules::default();

        // Integrated Shell materials also get their default when an existing
        // user config predates liquid glass. Only surfaces with a nonempty
        // protocol blur mask render it; Material and Launchpad stay unchanged.
        // User layer rules below can override this with liquid-glass false.
        if matches!(surface.namespace(), "realand-shell" | "realand-shell-dock") {
            resolved.background_effect.liquid_glass = Some(true);
        }

        for rule in rules {
            let matches = |m: &Match| {
                if let Some(at_startup) = m.at_startup {
                    if at_startup != is_at_startup {
                        return false;
                    }
                }

                surface_matches(surface, m)
            };

            if !(rule.matches.is_empty() || rule.matches.iter().any(matches)) {
                continue;
            }

            if rule.excludes.iter().any(matches) {
                continue;
            }

            if let Some(x) = rule.opacity {
                resolved.opacity = Some(x);
            }
            if let Some(x) = rule.block_out_from {
                resolved.block_out_from = Some(x);
            }
            if let Some(x) = rule.geometry_corner_radius {
                resolved.geometry_corner_radius = Some(x);
            }
            if let Some(x) = rule.place_within_backdrop {
                resolved.place_within_backdrop = x;
            }
            if let Some(x) = rule.baba_is_float {
                resolved.baba_is_float = x;
            }

            resolved.shadow.merge_with(&rule.shadow);

            resolved
                .background_effect
                .merge_with(&rule.background_effect);

            resolved.popups.merge_with(&rule.popups);
        }

        resolved
    }
}

fn surface_matches(surface: &LayerSurface, m: &Match) -> bool {
    if let Some(namespace_re) = &m.namespace {
        if !namespace_re.0.is_match(surface.namespace()) {
            return false;
        }
    }

    if let Some(layer) = m.layer {
        let surface_layer = match surface.layer() {
            Layer::Background => realand_ipc::Layer::Background,
            Layer::Bottom => realand_ipc::Layer::Bottom,
            Layer::Top => realand_ipc::Layer::Top,
            Layer::Overlay => realand_ipc::Layer::Overlay,
        };
        if layer != surface_layer {
            return false;
        }
    }

    true
}
