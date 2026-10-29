Realand dependency patch, 2026-10-12

Upstream: https://github.com/Smithay/smithay
Revision: 4cf0b62028039661477d482ec4758b687d8f4392
Version: 0.7.0
License: MIT (upstream LICENSE.txt retained)

All 343 tracked files were copied from the existing pinned Cargo checkout.
The local copy lives outside cargo vendor's output directory so release
packaging cannot delete the patch. Nix sources include this directory.

The only upstream source modification is src/utils/geometry.rs; the exact
change is retained in realand-geometry-subtraction.patch. Cargo.toml in the
Realand repository applies this local dependency through [patch]. The user's
shared Cargo checkout is untouched.

Rectangle::subtract_rects_many_in_place formerly visited original slots
forwards while swap_remove and appended split fragments changed the vector.
A newly created fragment could be processed twice and an original rectangle
skipped. Subtracting 24 overlapping rectangles from the same set left a
nonempty region. OutputDamageTracker consequently reported damage for an
unchanged scene; clipping could also retain pixels intended to be excluded.
Visiting original indices backwards processes each original rectangle once.

Regression coverage is in Realand's src/render_helpers/damage_region.rs:
subtract a region from itself; compare seeded rectangle subtraction against
an independent per-pixel coverage oracle. Native GPU tests verify unchanged
24-window capture reuse, rounded backdrop pixels, alpha, privacy, scale,
rotation and immutable snapshots.

Upstream status: fixed upstream on 2026-10-18 in commit 2923f22485
("fix Rectangle::subtract_rects_many_in_place skip"). No pull request is
needed; drop the local patch when the pinned revision includes that commit.

When updating Smithay, test whether upstream has fixed the loop. Remove the
patch and local dependency only after these regressions pass without it.
