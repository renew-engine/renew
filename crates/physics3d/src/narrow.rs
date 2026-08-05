//! Whether two shapes actually touch, and where.

use crate::shape::{Shape, Transform};
use renew_fixed::{Fixed, Vec3};

/// The direction taken when two shapes are exactly coincident and no
/// separating direction exists.
///
/// Arbitrary, and that is the point: two implementations must choose the same
/// arbitrary thing, and a reader must be able to predict which.
const COINCIDENT_FALLBACK: Vec3 = Vec3::new(Fixed::ONE, Fixed::ZERO, Fixed::ZERO);

/// A touch between two shapes.
///
/// **One point rather than a manifold, and that is a real limitation.** Two
/// axis-aligned boxes meeting face to face touch across a rectangle, and the
/// full contact is its corners — up to four of them. A single representative
/// point is how a box resting on a floor starts to rock.
///
/// It is enough for a voxel world, where a body meets a face at a time and the
/// caller resolves by axis, and it is not enough for a solver. Named here so
/// the gap is visible rather than discovered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contact {
    /// From the first shape toward the second — the direction the first would
    /// move to separate.
    pub normal: Vec3,
    /// Where, in world space.
    pub point: Vec3,
    /// How far they interpenetrate along the normal. Never negative.
    pub depth: Fixed,
}

/// Do these two shapes touch, and if so how?
///
/// `None` means apart; a touch at exactly zero separation is a contact with
/// depth zero rather than a miss, because a caller cannot act on a contact it
/// never receives.
///
/// # Antisymmetry, and the one case that breaks it
///
/// Swapping the arguments negates the normal — except when the two centres
/// coincide exactly, where the direction carries no information and both
/// orders take the stated fallback. Inherent rather than a defect: with
/// identical inputs there is nothing antisymmetric left to derive a direction
/// from. The pipeline never depends on it, because the broadphase emits every
/// pair with its lower collider first.
#[must_use]
pub fn collide(a: Shape, a_at: Transform, b: Shape, b_at: Transform) -> Option<Contact> {
    match (a, b) {
        (Shape::Sphere { radius: ra }, Shape::Sphere { radius: rb }) => {
            sphere_sphere(a_at.translation, ra, b_at.translation, rb)
        }
        (Shape::Sphere { radius }, Shape::Box { half_extents }) => {
            sphere_box(a_at.translation, radius, half_extents, b_at.translation)
        }
        (Shape::Box { half_extents }, Shape::Sphere { radius }) => {
            // Solved the other way and reversed, so there is one
            // implementation of this geometry rather than two that can drift.
            sphere_box(b_at.translation, radius, half_extents, a_at.translation).map(reverse)
        }
        (Shape::Box { half_extents: ha }, Shape::Box { half_extents: hb }) => {
            box_box(a_at.translation, ha, b_at.translation, hb)
        }
    }
}

fn reverse(contact: Contact) -> Contact {
    Contact {
        normal: -contact.normal,
        point: contact.point,
        depth: contact.depth,
    }
}

/// Two spheres: exact, and the only case with no ambiguity anywhere.
fn sphere_sphere(a: Vec3, ra: Fixed, b: Vec3, rb: Fixed) -> Option<Contact> {
    let delta = b - a;
    let reach = ra + rb;
    // Compared wide, so the test is exact at every scale a world reaches.
    if delta.length_squared_wide() > reach.wide_mul(reach) {
        return None;
    }
    let distance = delta.length();
    let normal = delta.normalize().unwrap_or(COINCIDENT_FALLBACK);
    let depth = (reach - distance).max(Fixed::ZERO);
    // The middle of the overlapped span, so the point does not jump between
    // the two surfaces as the depth changes.
    let point = a + normal * (ra - Fixed::from_bits(depth.to_bits() / 2));
    Some(Contact {
        normal,
        point,
        depth,
    })
}

/// A sphere against an axis-aligned box: clamp the centre into the box, and
/// the rest follows.
fn sphere_box(centre: Vec3, radius: Fixed, half: Vec3, box_at: Vec3) -> Option<Contact> {
    let local = centre - box_at;
    let clamped = Vec3::new(
        local.x.clamp(-half.x, half.x),
        local.y.clamp(-half.y, half.y),
        local.z.clamp(-half.z, half.z),
    );
    let surface = box_at + clamped;

    if clamped == local {
        // The centre is inside, so the shortest way out is through the nearest
        // face — and all six are candidates, which is why the choice compares
        // distances rather than testing a sign.
        let reaches = [
            (half.x - local.x.abs(), 0u8),
            (half.y - local.y.abs(), 1),
            (half.z - local.z.abs(), 2),
        ];
        let mut nearest = reaches[0];
        for candidate in reaches {
            // Strictly nearer, so a tie leaves the earlier axis and two
            // machines choose the same one.
            if candidate.0 < nearest.0 {
                nearest = candidate;
            }
        }
        let (to_face, axis) = nearest;
        let sign = |value: Fixed| {
            if value < Fixed::ZERO {
                Fixed::from_int(-1)
            } else {
                Fixed::ONE
            }
        };
        let out = match axis {
            0 => Vec3::new(sign(local.x), Fixed::ZERO, Fixed::ZERO),
            1 => Vec3::new(Fixed::ZERO, sign(local.y), Fixed::ZERO),
            _ => Vec3::new(Fixed::ZERO, Fixed::ZERO, sign(local.z)),
        };
        return Some(Contact {
            // From the sphere toward the box.
            normal: -out,
            point: surface,
            depth: to_face + radius,
        });
    }

    let delta = local - clamped;
    if delta.length_squared_wide() > radius.wide_mul(radius) {
        return None;
    }
    let distance = delta.length();
    let out = delta.normalize().unwrap_or(COINCIDENT_FALLBACK);
    Some(Contact {
        normal: -out,
        point: surface,
        depth: (radius - distance).max(Fixed::ZERO),
    })
}

/// Two axis-aligned boxes: the overlap on each axis, and the shallowest wins.
///
/// With no rotation this is the whole separating-axis test — the three world
/// axes are the only candidates, and one with no overlap is a proof of no
/// contact.
fn box_box(a: Vec3, ha: Vec3, b: Vec3, hb: Vec3) -> Option<Contact> {
    let delta = b - a;
    let reach = ha + hb;
    let overlaps = [
        (reach.x - delta.x.abs(), 0u8),
        (reach.y - delta.y.abs(), 1),
        (reach.z - delta.z.abs(), 2),
    ];
    for (overlap, _) in overlaps {
        if overlap < Fixed::ZERO {
            return None;
        }
    }

    // Shallowest wins; a tie leaves the earlier axis, so x beats y beats z and
    // two machines agree on a symmetric overlap.
    let mut best = overlaps[0];
    for candidate in overlaps {
        if candidate.0 < best.0 {
            best = candidate;
        }
    }
    let (depth, axis) = best;
    let along = match axis {
        0 => delta.x,
        1 => delta.y,
        _ => delta.z,
    };
    let sign = if along < Fixed::ZERO {
        Fixed::from_int(-1)
    } else {
        Fixed::ONE
    };
    let normal = match axis {
        0 => Vec3::new(sign, Fixed::ZERO, Fixed::ZERO),
        1 => Vec3::new(Fixed::ZERO, sign, Fixed::ZERO),
        _ => Vec3::new(Fixed::ZERO, Fixed::ZERO, sign),
    };
    // The middle of the overlapped region on the winning axis, clamped to the
    // shared extent on the other two.
    let point = Vec3::new(
        midpoint(a.x, ha.x, b.x, hb.x),
        midpoint(a.y, ha.y, b.y, hb.y),
        midpoint(a.z, ha.z, b.z, hb.z),
    );
    Some(Contact {
        normal,
        point,
        depth,
    })
}

/// The centre of the shared span on one axis.
fn midpoint(a: Fixed, ha: Fixed, b: Fixed, hb: Fixed) -> Fixed {
    let low = (a - ha).max(b - hb);
    let high = (a + ha).min(b + hb);
    Fixed::from_bits(low.to_bits().midpoint(high.to_bits()))
}

/// How far apart two shapes are, and along which direction.
///
/// Positive means separated by that distance; zero or negative means touching
/// or overlapping. The direction points **from `a` toward `b`**.
///
/// A sweep needs this and a contact test cannot give it: "do they touch" says
/// nothing about how far a body may travel before they do.
#[must_use]
pub fn separation(a: Shape, a_at: Transform, b: Shape, b_at: Transform) -> (Fixed, Vec3) {
    match (a, b) {
        (Shape::Sphere { radius: ra }, Shape::Sphere { radius: rb }) => {
            let delta = b_at.translation - a_at.translation;
            (
                delta.length() - ra - rb,
                delta.normalize().unwrap_or(COINCIDENT_FALLBACK),
            )
        }
        (Shape::Sphere { radius }, Shape::Box { half_extents }) => {
            sphere_box_separation(a_at.translation, radius, half_extents, b_at.translation)
        }
        (Shape::Box { half_extents }, Shape::Sphere { radius }) => {
            let (distance, direction) =
                sphere_box_separation(b_at.translation, radius, half_extents, a_at.translation);
            (distance, -direction)
        }
        (Shape::Box { half_extents: ha }, Shape::Box { half_extents: hb }) => {
            box_box_separation(a_at.translation, ha, b_at.translation, hb)
        }
    }
}

fn sphere_box_separation(centre: Vec3, radius: Fixed, half: Vec3, box_at: Vec3) -> (Fixed, Vec3) {
    let local = centre - box_at;
    let clamped = Vec3::new(
        local.x.clamp(-half.x, half.x),
        local.y.clamp(-half.y, half.y),
        local.z.clamp(-half.z, half.z),
    );
    let delta = clamped - local;
    (
        delta.length() - radius,
        delta.normalize().unwrap_or(COINCIDENT_FALLBACK),
    )
}

/// The widest gap over the three axes — a lower bound on the true distance,
/// which is what conservative advancement is built on. It under-reports a
/// diagonal gap, and a lower bound is exactly what is wanted.
fn box_box_separation(a: Vec3, ha: Vec3, b: Vec3, hb: Vec3) -> (Fixed, Vec3) {
    let delta = b - a;
    let reach = ha + hb;
    let gaps = [
        (delta.x.abs() - reach.x, delta.x, 0u8),
        (delta.y.abs() - reach.y, delta.y, 1),
        (delta.z.abs() - reach.z, delta.z, 2),
    ];
    let mut best = gaps[0];
    for candidate in gaps {
        if candidate.0 > best.0 {
            best = candidate;
        }
    }
    let (gap, along, axis) = best;
    let sign = if along < Fixed::ZERO {
        Fixed::from_int(-1)
    } else {
        Fixed::ONE
    };
    let direction = match axis {
        0 => Vec3::new(sign, Fixed::ZERO, Fixed::ZERO),
        1 => Vec3::new(Fixed::ZERO, sign, Fixed::ZERO),
        _ => Vec3::new(Fixed::ZERO, Fixed::ZERO, sign),
    };
    (gap, direction)
}
