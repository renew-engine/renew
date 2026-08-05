//! Casting a ray at a single shape.

use crate::shape::{Shape, Transform};
use renew_fixed::{Fixed, Vec3, Wide};

/// Where a ray met a shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RayHit {
    /// Distance along the ray's direction, never negative.
    pub distance: Fixed,
    /// Where it met, in world space.
    pub point: Vec3,
    /// The surface normal at that point, pointing back along the ray.
    pub normal: Vec3,
}

/// Cast a ray at one shape.
///
/// `direction` must be unit length; `max_distance` bounds the search.
///
/// # A ray that starts inside
///
/// **Hits, at distance zero.** A ground check whose origin is already inside
/// the floor should report the floor, not report nothing and let a character
/// fall through what it is standing in. The normal there points back along the
/// ray, since there is no surface crossing to take one from.
#[must_use]
pub fn cast(
    origin: Vec3,
    direction: Vec3,
    max_distance: Fixed,
    shape: Shape,
    at: Transform,
) -> Option<RayHit> {
    match shape {
        Shape::Sphere { radius } => sphere(origin, direction, max_distance, radius, at.translation),
        Shape::Box { half_extents } => axis_aligned_box(
            origin,
            direction,
            max_distance,
            half_extents,
            at.translation,
        ),
    }
}

/// Ray against sphere, in the form that keeps the arithmetic small.
///
/// With `direction` unit the quadratic's leading coefficient is one and the
/// discriminant collapses to `m² − l² + r²`, where `m` is how far along the ray
/// the centre projects. Three terms instead of a product of squares is what
/// keeps this exact at world scale rather than merely deterministic.
fn sphere(
    origin: Vec3,
    direction: Vec3,
    max_distance: Fixed,
    radius: Fixed,
    centre: Vec3,
) -> Option<RayHit> {
    let to_centre = centre - origin;
    let along = to_centre.dot(direction);
    let discriminant =
        along.wide_mul(along) - to_centre.length_squared_wide() + radius.wide_mul(radius);
    if discriminant < Wide::ZERO {
        return None;
    }
    let root = discriminant.sqrt();
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
        // Started inside: no crossing to take a normal from.
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

/// Ray against an axis-aligned box, by the slab test over three axes.
fn axis_aligned_box(
    origin: Vec3,
    direction: Vec3,
    max_distance: Fixed,
    half: Vec3,
    centre: Vec3,
) -> Option<RayHit> {
    let local = origin - centre;
    let mut enter = Fixed::ZERO;
    let mut exit = max_distance;
    // Which axis the entry came from, and which way it faces. Starts on +x so
    // a ray beginning inside — where no slab sets it — has a stated answer
    // rather than an accidental one.
    let mut entry_axis = 0u8;
    let mut entry_sign = Fixed::ONE;

    for axis in 0..3u8 {
        let (start, step, extent) = match axis {
            0 => (local.x, direction.x, half.x),
            1 => (local.y, direction.y, half.y),
            _ => (local.z, direction.z, half.z),
        };
        let Some(inverse) = Fixed::ONE.checked_div(step) else {
            // Parallel to this slab: a ray outside it never enters, and one
            // inside is unconstrained by it. A computed zero divisor is
            // ordinary geometry, which is why the division is checked.
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
        // One check rejects everything: a box behind the origin drives `exit`
        // negative while `enter` sits at zero, and a box beyond the ray's
        // reach drives `enter` past `max_distance`, which `exit` can never
        // exceed. A second test after the loop would be unreachable.
        if enter > exit {
            return None;
        }
    }

    let normal = match entry_axis {
        0 => Vec3::new(entry_sign, Fixed::ZERO, Fixed::ZERO),
        1 => Vec3::new(Fixed::ZERO, entry_sign, Fixed::ZERO),
        _ => Vec3::new(Fixed::ZERO, Fixed::ZERO, entry_sign),
    };
    Some(RayHit {
        distance: enter,
        point: origin + direction * enter,
        normal,
    })
}
