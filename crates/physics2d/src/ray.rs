//! Casting a ray at a single shape.

use crate::shape::{Shape, Transform};
use renew_fixed::{Fixed, Vec2, Wide};

/// Where a ray met a shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RayHit {
    /// Distance along the ray's direction, never negative.
    pub distance: Fixed,
    /// Where it met, in world space.
    pub point: Vec2,
    /// The surface normal at that point, pointing back along the ray.
    pub normal: Vec2,
}

/// Cast a ray at one shape.
///
/// `direction` must be unit length; `max_distance` bounds the search.
///
/// # A ray that starts inside
///
/// **Hits, at distance zero.** That is the vocabulary's rule and it is the
/// useful one: a ground check whose origin is already inside the floor should
/// report the floor, not report nothing and let a character fall through
/// something it is standing in. The normal reported there points back along
/// the ray, since there is no surface crossing to take one from.
#[must_use]
pub fn cast(
    origin: Vec2,
    direction: Vec2,
    max_distance: Fixed,
    shape: Shape,
    at: Transform,
) -> Option<RayHit> {
    match shape {
        Shape::Circle { radius } => circle(origin, direction, max_distance, radius, at.translation),
        Shape::Box { half_extents } => {
            oriented_box(origin, direction, max_distance, half_extents, at)
        }
        // Not written yet, and saying "no hit" would be a lie a caller acts on.
        Shape::Capsule { .. } => None,
    }
}

/// Ray against circle, in the form that keeps the arithmetic small.
///
/// Solving `|o + t·d − c|² = r²` directly needs a discriminant built from
/// three squared lengths. With `d` unit the quadratic's leading coefficient is
/// one, and the discriminant collapses to `m² − l² + r²` where `m` is how far
/// along the ray the centre projects and `l` is the distance to it — three
/// terms instead of a product of squares, which is what keeps this exact at
/// world scale rather than merely deterministic.
fn circle(
    origin: Vec2,
    direction: Vec2,
    max_distance: Fixed,
    radius: Fixed,
    centre: Vec2,
) -> Option<RayHit> {
    let to_centre = centre - origin;
    let along = to_centre.dot(direction);
    let discriminant =
        along.wide_mul(along) - to_centre.length_squared_wide() + radius.wide_mul(radius);
    if discriminant < Wide::ZERO {
        return None;
    }
    let root = discriminant.sqrt();
    // The near crossing. Negative means the origin is past it — inside the
    // circle, or behind it entirely.
    let near = along - root;
    let far = along + root;
    if far < Fixed::ZERO {
        return None; // wholly behind the origin
    }
    let distance = near.max(Fixed::ZERO);
    if distance > max_distance {
        return None;
    }
    let point = origin + direction * distance;
    let normal = if near < Fixed::ZERO {
        // Started inside: there is no crossing to take a normal from, so it
        // points back the way the ray came.
        -direction
    } else {
        (point - centre).normalize().unwrap_or(-direction)
    };
    Some(RayHit {
        distance,
        point,
        normal,
    })
}

/// Ray against an oriented box, by the slab test in the box's own frame where
/// it is axis-aligned by definition.
fn oriented_box(
    origin: Vec2,
    direction: Vec2,
    max_distance: Fixed,
    half: Vec2,
    at: Transform,
) -> Option<RayHit> {
    let (sin, cos) = at.rotation.sin_cos();
    let axis_x = Vec2::new(cos, sin);
    let axis_y = Vec2::new(-sin, cos);
    let offset = origin - at.translation;
    let local_origin = Vec2::new(offset.dot(axis_x), offset.dot(axis_y));
    let local_direction = Vec2::new(direction.dot(axis_x), direction.dot(axis_y));

    let mut enter = Fixed::ZERO;
    let mut exit = max_distance;
    // Which axis the entry came from, and which way it faces. Starts on +x so
    // a ray beginning inside — where no slab sets it — has a stated answer
    // rather than an accidental one.
    let mut entry_axis = 0u8;
    let mut entry_sign = Fixed::ONE;

    for axis in 0..2u8 {
        let (start, step, extent) = if axis == 0 {
            (local_origin.x, local_direction.x, half.x)
        } else {
            (local_origin.y, local_direction.y, half.y)
        };
        let Some(inverse) = Fixed::ONE.checked_div(step) else {
            // Parallel to this slab: a ray outside it never enters, and one
            // inside is unconstrained by it. A computed zero divisor is
            // ordinary geometry here, not a mistake, which is why this is a
            // checked division rather than an asserting one.
            if start < -extent || start > extent {
                return None;
            }
            continue;
        };
        let first = (-extent - start).saturating_mul(inverse);
        let second = (extent - start).saturating_mul(inverse);
        let (near, far) = if first <= second {
            (first, second)
        } else {
            (second, first)
        };
        if near > enter {
            enter = near;
            entry_axis = axis;
            entry_sign = if step < Fixed::ZERO {
                Fixed::ONE
            } else {
                Fixed::from_int(-1)
            };
        }
        exit = exit.min(far);
        // This one check rejects everything: a box behind the origin drives
        // `exit` negative while `enter` sits at zero, and a box beyond the
        // ray's reach drives `enter` past `max_distance`, which `exit` can
        // never exceed. A second test after the loop for either of those is
        // unreachable, so there is not one.
        if enter > exit {
            return None;
        }
    }

    let point = origin + direction * enter;
    let local_normal = if axis_is_x(entry_axis) {
        Vec2::new(entry_sign, Fixed::ZERO)
    } else {
        Vec2::new(Fixed::ZERO, entry_sign)
    };
    let normal = axis_x * local_normal.x + axis_y * local_normal.y;
    Some(RayHit {
        distance: enter,
        point,
        normal,
    })
}

const fn axis_is_x(axis: u8) -> bool {
    axis == 0
}
