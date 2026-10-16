// Your shader must contain one function (see the bottom of this file).
//
// It should not contain any uniform definitions or anything else, as clarion
// provides them for you.
//
// All symbols defined by clarion will have a clarion_ prefix, so don't use it for
// your own variables and functions.

// The function that you must define looks like this:
vec4 resize_color(vec3 coords_curr_geo, vec3 size_curr_geo) {
    vec4 color = /* ...compute the color... */;
    return color;
}

// It takes as input:
//
// * coords_curr_geo: coordinates of the current pixel relative to the current
// window geometry.
//
// These are homogeneous (the Z component is equal to 1) and scaled in such a
// way that the 0 to 1 coordinates lie within the current window geometry (in
// the middle of a resize). Pixels outside the window geometry will have
// coordinates below 0 or above 1.
//
// The window geometry is its "visible bounds" from the user's perspective.
//
// The shader runs over an area of unspecified size and location, so you must
// expect and handle coordinates outside the [0, 1] range. The area will be
// large enough to accommodate a crossfade effect.
//
// * size_curr_geo: size of the current window geometry in logical pixels.
//
// It is homogeneous (the Z component is equal to 1).
//
// The function must return the color of the pixel (with premultiplied alpha).
// The pixel color will be further processed by clarion (for example, to apply the
// final opacity from window rules).

// Now let's go over the uniforms that clarion defines.
//
// You should only rely on the uniforms documented here. Any other uniforms can
// change or be removed without notice.

// Previous (before resize) window texture.
uniform sampler2D clarion_tex_prev;

// Matrix that converts geometry coordinates into the previous window texture
// coordinates.
//
// The window texture can and will go outside the geometry (for client-side
// decoration shadows for example), which is why this matrix is necessary.
uniform mat3 clarion_geo_to_tex_prev;

// Next (after resize) window texture.
uniform sampler2D clarion_tex_next;

// Matrix that converts geometry coordinates into the next window texture
// coordinates.
uniform mat3 clarion_geo_to_tex_next;


// Matrix that converts coords_curr_geo into coordinates inside the previous
// (before resize) window geometry.
uniform mat3 clarion_curr_geo_to_prev_geo;

// Matrix that converts coords_curr_geo into coordinates inside the next
// (after resize) window geometry.
uniform mat3 clarion_curr_geo_to_next_geo;


// Unclamped progress of the animation.
//
// Goes from 0 to 1 but may overshoot and oscillate.
uniform float clarion_progress;

// Clamped progress of the animation.
//
// Goes from 0 to 1, but will stop at 1 as soon as it first reaches 1. Will not
// overshoot or oscillate.
uniform float clarion_clamped_progress;

// Now let's look at some examples. You can copy everything below this line
// into your custom-shader to experiment.

// Example: fill the current geometry with a solid vertical gradient.
vec4 solid_gradient(vec3 coords_curr_geo, vec3 size_curr_geo) {
    vec3 coords = coords_curr_geo;
    vec4 color = vec4(0.0);

    // Paint only the area inside the current geometry.
    if (0.0 <= coords.x && coords.x <= 1.0
            && 0.0 <= coords.y && coords.y <= 1.0)
    {
        vec4 from = vec4(1.0, 0.0, 0.0, 1.0);
        vec4 to = vec4(0.0, 1.0, 0.0, 1.0);
        color = mix(from, to, coords.y);
    }

    return color;
}

// Example: crossfade between previous and next texture, stretched to the
// current geometry.
vec4 crossfade(vec3 coords_curr_geo, vec3 size_curr_geo) {
    // Convert coordinates into the texture space for sampling.
    vec3 coords_tex_prev = clarion_geo_to_tex_prev * coords_curr_geo;
    vec4 color_prev = texture2D(clarion_tex_prev, coords_tex_prev.st);

    // Convert coordinates into the texture space for sampling.
    vec3 coords_tex_next = clarion_geo_to_tex_next * coords_curr_geo;
    vec4 color_next = texture2D(clarion_tex_next, coords_tex_next.st);

    vec4 color = mix(color_prev, color_next, clarion_clamped_progress);
    return color;
}

// Example: next texture, stretched to the current geometry.
vec4 stretch_next(vec3 coords_curr_geo, vec3 size_curr_geo) {
    vec3 coords_tex_next = clarion_geo_to_tex_next * coords_curr_geo;
    vec4 color = texture2D(clarion_tex_next, coords_tex_next.st);
    return color;
}

// Example: next texture, stretched to the current geometry if smaller, and
// cropped if bigger.
vec4 stretch_or_crop_next(vec3 coords_curr_geo, vec3 size_curr_geo) {
    vec3 coords_next_geo = clarion_curr_geo_to_next_geo * coords_curr_geo;

    vec3 coords_stretch = clarion_geo_to_tex_next * coords_curr_geo;
    vec3 coords_crop = clarion_geo_to_tex_next * coords_next_geo;

    // We can crop if the current window size is smaller than the next window
    // size. One way to tell is by comparing to 1.0 the X and Y scaling
    // coefficients in the current-to-next transformation matrix.
    bool can_crop_by_x = clarion_curr_geo_to_next_geo[0][0] <= 1.0;
    bool can_crop_by_y = clarion_curr_geo_to_next_geo[1][1] <= 1.0;

    vec3 coords = coords_stretch;
    if (can_crop_by_x)
        coords.x = coords_crop.x;
    if (can_crop_by_y)
        coords.y = coords_crop.y;

    vec4 color = texture2D(clarion_tex_next, coords.st);

    // However, when we crop, we also want to crop out anything outside the
    // current geometry. This is because the area of the shader is unspecified
    // and usually bigger than the current geometry, so if we don't fill pixels
    // outside with transparency, the texture will leak out.
    //
    // When stretching, this is not an issue because the area outside will
    // correspond to client-side decoration shadows, which are already supposed
    // to be outside.
    if (can_crop_by_x && (coords_curr_geo.x < 0.0 || 1.0 < coords_curr_geo.x))
        color = vec4(0.0);
    if (can_crop_by_y && (coords_curr_geo.y < 0.0 || 1.0 < coords_curr_geo.y))
        color = vec4(0.0);

    return color;
}

// Example: cropped next texture if it's bigger than the current geometry, and
// crossfade between previous and next texture otherwise.
vec4 crossfade_or_crop_next(vec3 coords_curr_geo, vec3 size_curr_geo) {
    vec3 coords_next_geo = clarion_curr_geo_to_next_geo * coords_curr_geo;
    vec3 coords_prev_geo = clarion_curr_geo_to_prev_geo * coords_curr_geo;

    vec3 coords_crop = clarion_geo_to_tex_next * coords_next_geo;
    vec3 coords_stretch = clarion_geo_to_tex_next * coords_curr_geo;
    vec3 coords_stretch_prev = clarion_geo_to_tex_prev * coords_curr_geo;

    // We can crop if the current window size is smaller than the next window
    // size. One way to tell is by comparing to 1.0 the X and Y scaling
    // coefficients in the current-to-next transformation matrix.
    bool can_crop_by_x = clarion_curr_geo_to_next_geo[0][0] <= 1.0;
    bool can_crop_by_y = clarion_curr_geo_to_next_geo[1][1] <= 1.0;
    bool crop = can_crop_by_x && can_crop_by_y;

    vec4 color;

    if (crop) {
        // However, when we crop, we also want to crop out anything outside the
        // current geometry. This is because the area of the shader is unspecified
        // and usually bigger than the current geometry, so if we don't fill pixels
        // outside with transparency, the texture will leak out.
        //
        // When crossfading, this is not an issue because the area outside will
        // correspond to client-side decoration shadows, which are already supposed
        // to be outside.
        if (coords_curr_geo.x < 0.0 || 1.0 < coords_curr_geo.x ||
                coords_curr_geo.y < 0.0 || 1.0 < coords_curr_geo.y) {
            color = vec4(0.0);
        } else {
            color = texture2D(clarion_tex_next, coords_crop.st);
        }
    } else {
        // If we can't crop, then crossfade.
        color = texture2D(clarion_tex_next, coords_stretch.st);
        vec4 color_prev = texture2D(clarion_tex_prev, coords_stretch_prev.st);
        color = mix(color_prev, color, clarion_clamped_progress);
    }

    return color;
}

// This is the function that you must define.
vec4 resize_color(vec3 coords_curr_geo, vec3 size_curr_geo) {
    // You can pick one of the example functions or write your own.
    return stretch_or_crop_next(coords_curr_geo, size_curr_geo);
}

