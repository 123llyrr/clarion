//! Minimal coordinate-kind adapter for the core's destination-local coverage.
use clarion_core::region::{self, PixelRectangle};
use smithay::utils::{Rectangle, Size};

struct Coordinates<K>(std::marker::PhantomData<K>);
impl<K> PixelRectangle for Coordinates<K> {
    type Rect = Rectangle<i32, K>;
    fn xywh(rect: Self::Rect) -> [i32; 4] {
        [rect.loc.x, rect.loc.y, rect.size.w, rect.size.h]
    }
    fn from_xywh([x, y, w, h]: [i32; 4]) -> Self::Rect {
        Rectangle::new((x, y).into(), (w, h).into())
    }
}
pub type DamageRegions<K> = region::DamageRegions<Rectangle<i32, K>>;

pub fn disjoint_damage<K>(
    damage: &[Rectangle<i32, K>],
    size: Size<i32, K>,
) -> DamageRegions<K> {
    region::disjoint_damage::<Coordinates<K>>(damage, [size.w, size.h])
}

#[cfg(test)]
fn clipped<K>(rect: Rectangle<i32, K>, size: Size<i32, K>) -> Option<Rectangle<i32, K>> {
    region::clipped::<Coordinates<K>>(rect, [size.w, size.h])
}

#[cfg(test)]
mod regressions;
#[cfg(test)]
mod tests;
