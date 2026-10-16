//! Constraint-aware column sizing without protocol or decoration objects.

#[derive(Debug, Clone, Copy)]
pub enum Height {
    Automatic { weight: f64 },
    Fixed(f64),
}

#[derive(Debug, Clone, Copy)]
pub struct HeightConstraint {
    pub height: Height,
    pub minimum: f64,
    /// Zero is unconstrained. Exact minimum/maximum always has fixed height.
    pub maximum: f64,
    /// Tile height minus client height, resolved by the platform decoration adapter.
    pub decoration: f64,
}

/// Reusable working storage for dirty column updates. Device adapters can retain
/// this alongside their constraint and result buffers instead of allocating per resize.
#[derive(Debug, Default)]
pub struct HeightScratch {
    heights: Vec<Option<f64>>,
    weights: Vec<f64>,
    remaining_weights: Vec<f64>,
}

fn rounded(height: f64, decoration: f64) -> f64 {
    (height - decoration).round().max(1.) + decoration
}

fn minimum(window: &HeightConstraint) -> f64 {
    crate::geometry::nonnegative(window.minimum).max(1.)
}

fn decoration(window: &HeightConstraint) -> f64 {
    crate::geometry::finite(window.decoration, 0.)
}

impl HeightScratch {
    fn rebuild_remaining_weights(&mut self) {
        self.remaining_weights.resize(self.weights.len() + 1, 0.);
        let mut total = 0.;
        self.remaining_weights[self.weights.len()] = 0.;
        for index in (0..self.weights.len()).rev() {
            if self.heights[index].is_none() {
                total += self.weights[index];
            }
            self.remaining_weights[index] = total;
        }
    }
}

// Preserve the established sequential rounding for ordinary weights. A suffix
// sum is the recovery denominator when subtraction loses significant digits;
// changing every denominator would move exact half-pixel rounding boundaries.
fn remaining_weight(legacy: f64, suffix: f64, count: usize) -> f64 {
    let tolerance = suffix * (8. * f64::EPSILON * count.max(1) as f64);
    if legacy.is_finite() && legacy > 0. && (legacy - suffix).abs() <= tolerance {
        legacy
    } else {
        suffix
    }
}

/// Preserve configured fixed sizes, then distribute the remainder by weight.
/// Minimum violations become fixed allocations before any final rounding, so
/// later windows do not snap because an earlier provisional size was too small.
/// Automatic maximum hints are used only for exact-size clients, preserving
/// Clarion's established scrolling semantics.
pub fn resolve_heights(
    total_height: f64,
    gap: f64,
    tabbed: bool,
    tabbed_limit: f64,
    windows: &[HeightConstraint],
) -> Vec<f64> {
    let mut result = Vec::new();
    resolve_heights_into(
        total_height,
        gap,
        tabbed,
        tabbed_limit,
        windows,
        &mut result,
        &mut HeightScratch::default(),
    );
    result
}

/// Resolve into retained buffers. After capacity has been established, updates
/// with the same or fewer windows allocate no heap storage.
pub fn resolve_heights_into(
    total_height: f64,
    gap: f64,
    tabbed: bool,
    tabbed_limit: f64,
    windows: &[HeightConstraint],
    result: &mut Vec<f64>,
    scratch: &mut HeightScratch,
) {
    result.clear();
    scratch.heights.clear();
    scratch.weights.clear();
    if windows.is_empty() {
        return;
    }
    let tab_height = tabbed.then(|| {
        let preferred = windows
            .iter()
            .find_map(|window| match window.height {
                Height::Fixed(height) => Some(height),
                _ => None,
            })
            .unwrap_or(tabbed_limit);
        let largest_minimum = windows.iter().map(minimum).fold(0., f64::max);
        preferred.max(largest_minimum.min(tabbed_limit))
    });
    scratch.heights.extend(windows.iter().map(|window| {
        let min = minimum(window);
        let max = crate::geometry::nonnegative(window.maximum);
        let fixed = if min == max {
            Some(min)
        } else {
            tab_height.or(match window.height {
                Height::Fixed(height) => Some(height),
                _ => None,
            })
        };
        fixed.map(|height| {
            let height = crate::geometry::finite(height, min);
            if max > 0. {
                height.min(max).max(min)
            } else {
                height.max(min)
            }
        })
    }));
    scratch
        .weights
        .extend(windows.iter().map(|window| match window.height {
            Height::Automatic { weight } if weight.is_finite() && weight > 0. => weight,
            _ => 1.,
        }));
    let mut total_weight: f64 = scratch
        .heights
        .iter()
        .zip(&scratch.weights)
        .filter_map(|(height, weight)| height.is_none().then_some(weight))
        .sum();
    if !total_weight.is_finite() {
        // Preserve ordinary weights and rounding. Rescale only when their sum
        // overflows, and keep tiny weights positive so minimum allocation still
        // sees them even when their original ratio would underflow to zero.
        let largest = scratch
            .heights
            .iter()
            .zip(&scratch.weights)
            .filter_map(|(height, weight)| height.is_none().then_some(*weight))
            .fold(0., f64::max);
        for (height, weight) in scratch.heights.iter().zip(&mut scratch.weights) {
            if height.is_none() {
                *weight = (*weight / largest).max(f64::MIN_POSITIVE);
            }
        }
        total_weight = scratch
            .heights
            .iter()
            .zip(&scratch.weights)
            .filter_map(|(height, weight)| height.is_none().then_some(weight))
            .sum();
    }
    let mut remaining = crate::geometry::finite(total_height, 0.)
        - crate::geometry::nonnegative(gap) * (windows.len() + 1) as f64
        - scratch.heights.iter().flatten().sum::<f64>();
    loop {
        if !scratch.heights.iter().any(Option::is_none) {
            break;
        }
        scratch.rebuild_remaining_weights();
        total_weight = remaining_weight(total_weight, scratch.remaining_weights[0], windows.len());
        let mut available = remaining;
        let mut provisional_weight = total_weight;
        let mut violation = None;
        for (index, height) in scratch.heights.iter().enumerate() {
            if height.is_some() {
                continue;
            }
            // Suffix sums recover a small last weight after a much larger first
            // allocation, without changing ordinary sequential client rounding.
            provisional_weight = remaining_weight(
                provisional_weight,
                scratch.remaining_weights[index],
                windows.len() - index,
            );
            let desired = available * (scratch.weights[index] / provisional_weight);
            let min = minimum(&windows[index]);
            if min > desired {
                violation = Some((index, min));
                break;
            }
            available -= rounded(desired, decoration(&windows[index]));
            provisional_weight -= scratch.weights[index];
        }
        if let Some((index, min)) = violation {
            scratch.heights[index] = Some(min);
            remaining -= min;
            total_weight -= scratch.weights[index];
            continue;
        }
        for (index, height) in scratch.heights.iter_mut().enumerate() {
            if height.is_some() {
                continue;
            }
            total_weight = remaining_weight(
                total_weight,
                scratch.remaining_weights[index],
                windows.len() - index,
            );
            let value = rounded(
                remaining * (scratch.weights[index] / total_weight),
                decoration(&windows[index]),
            );
            *height = Some(value);
            remaining -= value;
            total_weight -= scratch.weights[index];
        }
        break;
    }
    result.extend(scratch.heights.iter().map(|height| height.unwrap()));
}

#[cfg(test)]
mod tests {
    use super::*;
    fn auto(weight: f64, minimum: f64, decoration: f64) -> HeightConstraint {
        HeightConstraint {
            height: Height::Automatic { weight },
            minimum,
            maximum: 0.,
            decoration,
        }
    }

    #[test]
    fn minimum_allocation_precedes_weighted_integer_rounding() {
        assert_eq!(
            resolve_heights(
                120.,
                5.,
                false,
                0.,
                &[auto(1., 60., 0.), auto(1., 1., 0.), auto(1., 1., 0.),]
            ),
            vec![60., 20., 20.]
        );
    }

    #[test]
    fn decimal_weights_preserve_established_half_pixel_rounding() {
        assert_eq!(
            resolve_heights(500., 0., false, 0., &[auto(0.1, 1., 0.); 3]),
            vec![167., 166., 167.]
        );
    }

    #[test]
    fn decoration_preserves_ordinary_client_rounding_and_total_remainder() {
        let heights = resolve_heights(
            500.,
            3.,
            false,
            0.,
            &[auto(1., 1., 2.5), auto(2., 1., 4.5), auto(1., 1., 0.)],
        );
        assert_eq!(heights, vec![122.5, 243.5, 122.]);
        assert_eq!(heights.iter().sum::<f64>(), 488.);
    }

    #[test]
    fn tabbed_exact_and_fixed_constraints_remain_individual() {
        let windows = [
            auto(1., 1., 0.),
            HeightConstraint {
                height: Height::Fixed(80.),
                minimum: 100.,
                maximum: 100.,
                decoration: 0.,
            },
        ];
        assert_eq!(
            resolve_heights(300., 5., true, 250., &windows),
            vec![100., 100.]
        );
    }

    #[test]
    fn very_large_leading_weight_cannot_erase_later_client_minimum() {
        assert_eq!(
            resolve_heights(
                500.,
                0.,
                false,
                0.,
                &[auto(1e20, 1., 0.), auto(1., 1., 0.),]
            ),
            vec![499., 1.]
        );
        assert_eq!(
            resolve_heights(
                500.,
                0.,
                false,
                0.,
                &[
                    auto(1e20, 1., 0.),
                    auto(1., 1., 0.),
                    auto(1., 1., 0.),
                    auto(1., 1., 0.),
                ]
            ),
            vec![497., 1., 1., 1.]
        );
    }

    #[test]
    fn finite_weights_whose_sum_overflows_still_share_space_symmetrically() {
        assert_eq!(
            resolve_heights(
                500.,
                0.,
                false,
                0.,
                &[auto(1e308, 1., 0.), auto(1e308, 1., 0.),]
            ),
            vec![250., 250.]
        );
        assert_eq!(
            resolve_heights(
                500.,
                0.,
                false,
                0.,
                &[
                    auto(1e308, 1., 0.),
                    auto(1e308, 1., 0.),
                    auto(1e-300, 1., 0.),
                ]
            ),
            vec![250., 249., 1.]
        );
    }

    #[test]
    fn tiny_positive_weights_retain_normal_proportions() {
        assert_eq!(
            resolve_heights(
                500.,
                0.,
                false,
                0.,
                &[auto(1e-300, 1., 0.), auto(1e-300, 1., 0.),]
            ),
            vec![250., 250.]
        );
    }

    #[test]
    fn client_minimums_larger_than_the_output_are_not_silently_violated() {
        assert_eq!(
            resolve_heights(
                100.,
                0.,
                false,
                0.,
                &[auto(1e20, 60., 0.), auto(1., 60., 0.),]
            ),
            vec![60., 60.]
        );
    }

    #[test]
    fn reused_buffers_do_not_retain_sizes_or_modes_from_a_previous_column() {
        let mut scratch = HeightScratch::default();
        let mut result = Vec::new();
        resolve_heights_into(
            500.,
            0.,
            false,
            0.,
            &[auto(1e20, 1., 0.), auto(1., 1., 0.)],
            &mut result,
            &mut scratch,
        );
        assert_eq!(result, vec![499., 1.]);
        resolve_heights_into(
            300.,
            5.,
            true,
            250.,
            &[auto(1., 1., 0.)],
            &mut result,
            &mut scratch,
        );
        assert_eq!(result, vec![250.]);
        resolve_heights_into(300., 5., false, 0., &[], &mut result, &mut scratch);
        assert!(result.is_empty());
        resolve_heights_into(
            100.,
            0.,
            false,
            0.,
            &[auto(1., 1., 0.), auto(1., 1., 0.)],
            &mut result,
            &mut scratch,
        );
        assert_eq!(result, vec![50., 50.]);
    }
}
