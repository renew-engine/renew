//! Whether two shapes actually touch, and where.

use crate::contact::{ContactPoint, MAX_MANIFOLD_POINTS, Manifold};
use crate::shape::{Shape, Transform};
use renew_fixed::{Fixed, Vec2};

/// The direction taken when two shapes are exactly coincident and no
/// separating direction exists.
///
/// Arbitrary, and that is the point: two implementations must choose the same
/// arbitrary thing, and a reader must be able to predict which.
const COINCIDENT_FALLBACK: Vec2 = Vec2::new(Fixed::ONE, Fixed::ZERO);

/// A box's two world-space axis directions, unit length.
fn box_axes(transform: Transform) -> (Vec2, Vec2) {
    let (sin, cos) = transform.rotation.sin_cos();
    (Vec2::new(cos, sin), Vec2::new(-sin, cos))
}

/// How far a box reaches from its centre along `direction`, which must be
/// unit length.
fn box_reach(half: Vec2, axes: (Vec2, Vec2), direction: Vec2) -> Fixed {
    let along_x = half.x.saturating_mul(axes.0.dot(direction).abs());
    let along_y = half.y.saturating_mul(axes.1.dot(direction).abs());
    along_x + along_y
}

/// Do these two shapes touch, and if so how?
///
/// The normal points **from `a` toward `b`** — the direction `a` would move to
/// separate. `None` means they are apart; a touch at exactly zero separation
/// is a contact with depth zero rather than a miss, because the vocabulary
/// defines depth zero as *touching or within the contact tolerance* and a
/// caller cannot act on a contact it never receives.
///
/// # Antisymmetry, and the one case that breaks it
///
/// Swapping the arguments negates the normal and leaves the depth alone —
/// **except when the two shapes' centres coincide exactly.** There the
/// direction carries no information: both orders see the same distance in
/// every direction, so both return [`COINCIDENT_FALLBACK`], and the two
/// results agree rather than opposing.
///
/// That is inherent rather than a defect. With identical inputs in both orders
/// there is nothing antisymmetric left to derive a direction from, and any
/// implementation that appeared to manage it would be reading something it
/// should not — an address, an allocation order, a hash.
///
/// **The pipeline never depends on it**, because the broadphase emits every
/// pair with its lower collider first and narrowphase is called in that order.
/// A caller doing its own tests should do the same: order the pair, then ask.
/// [`Manifold::attribute`](crate::contact::Manifold::attribute) is what makes
/// that safe if the order is not already canonical.
#[must_use]
pub fn collide(a: Shape, a_at: Transform, b: Shape, b_at: Transform) -> Option<Manifold> {
    match (a, b) {
        (Shape::Circle { radius: ra }, Shape::Circle { radius: rb }) => {
            circle_circle(a_at.translation, ra, b_at.translation, rb)
        }
        (Shape::Circle { radius }, Shape::Box { half_extents }) => {
            circle_box(a_at.translation, radius, half_extents, b_at)
        }
        (Shape::Box { half_extents }, Shape::Circle { radius }) => {
            // Solved in the other order, then reversed — one implementation of
            // the geometry rather than two that can disagree.
            circle_box(b_at.translation, radius, half_extents, a_at).map(reverse)
        }
        (Shape::Box { half_extents: ha }, Shape::Box { half_extents: hb }) => {
            box_box(ha, a_at, hb, b_at)
        }
        // Capsules are not yet implemented. Returning `None` would be a lie —
        // a caller would read "they do not touch" — so the arm is absent and
        // the match is exhaustive over what exists.
        _ => None,
    }
}

/// Flip a manifold so its normal points the other way.
fn reverse(manifold: Manifold) -> Manifold {
    Manifold {
        normal: -manifold.normal,
        points: manifold.points,
        count: manifold.count,
    }
}

/// Two circles: exact, and the only case with no ambiguity anywhere.
fn circle_circle(a: Vec2, ra: Fixed, b: Vec2, rb: Fixed) -> Option<Manifold> {
    let delta = b - a;
    let reach = ra + rb;
    // Compared wide, so the test is exact at every scale a world reaches.
    if delta.length_squared_wide() > reach.wide_mul(reach) {
        return None;
    }
    let distance = delta.length();
    let normal = delta.normalize().unwrap_or(COINCIDENT_FALLBACK);
    let depth = reach - distance;
    // The midpoint of the overlapped span, so the point does not jump between
    // the two surfaces as the depth changes.
    let position = a + normal * (ra - halve(depth));
    Some(Manifold::single(normal, position, depth.max(Fixed::ZERO)))
}

/// Half, rounding toward zero — depths are non-negative here so the direction
/// only matters for the last raw unit.
fn halve(value: Fixed) -> Fixed {
    Fixed::from_bits(value.to_bits() / 2)
}

/// A circle against a box, solved in the box's frame where the box is
/// axis-aligned by definition.
fn circle_box(centre: Vec2, radius: Fixed, half: Vec2, box_at: Transform) -> Option<Manifold> {
    let axes = box_axes(box_at);
    let offset = centre - box_at.translation;
    // Into box-local coordinates: project onto the box's own axes.
    let local = Vec2::new(offset.dot(axes.0), offset.dot(axes.1));
    let clamped = Vec2::new(
        local.x.clamp(-half.x, half.x),
        local.y.clamp(-half.y, half.y),
    );
    let inside = clamped == local;

    let (local_normal, depth) = if inside {
        // The centre is within the box, so the shortest way out is through
        // the nearest face — and every face is a candidate, which is why the
        // choice is made by comparing distances rather than by a sign test.
        let to_x = half.x - local.x.abs();
        let to_y = half.y - local.y.abs();
        if to_x <= to_y {
            let sign = if local.x < Fixed::ZERO {
                Fixed::from_int(-1)
            } else {
                Fixed::ONE
            };
            (Vec2::new(sign, Fixed::ZERO), to_x + radius)
        } else {
            let sign = if local.y < Fixed::ZERO {
                Fixed::from_int(-1)
            } else {
                Fixed::ONE
            };
            (Vec2::new(Fixed::ZERO, sign), to_y + radius)
        }
    } else {
        let delta = local - clamped;
        if delta.length_squared_wide() > radius.wide_mul(radius) {
            return None;
        }
        let distance = delta.length();
        let direction = delta.normalize().unwrap_or(COINCIDENT_FALLBACK);
        (direction, radius - distance)
    };

    // Back to world: the local normal points from the box toward the circle,
    // and the report wants it from the circle toward the box.
    let world_normal = -(axes.0 * (local_normal.x) + axes.1 * (local_normal.y));
    let surface = box_at.translation + axes.0 * (clamped.x) + axes.1 * (clamped.y);
    Some(Manifold::single(
        world_normal,
        surface,
        depth.max(Fixed::ZERO),
    ))
}

/// The winning separating axis of a box-box test.
struct Separation {
    /// Unit, pointing from the reference box toward the other.
    normal: Vec2,
    depth: Fixed,
    /// Whether the axis came from the second box rather than the first.
    from_second: bool,
}

/// Test one candidate axis, returning the overlap along it or `None` if the
/// boxes are apart there — one separating axis is a proof of no contact.
fn overlap_on(
    axis: Vec2,
    ha: Vec2,
    a_axes: (Vec2, Vec2),
    a_at: Transform,
    hb: Vec2,
    b_axes: (Vec2, Vec2),
    b_at: Transform,
) -> Option<Fixed> {
    let centres = (b_at.translation - a_at.translation).dot(axis).abs();
    let reach = box_reach(ha, a_axes, axis) + box_reach(hb, b_axes, axis);
    if centres > reach {
        None
    } else {
        Some(reach - centres)
    }
}

/// Two oriented boxes, by the separating-axis test over the four face
/// normals — two from each box, since opposite faces share an axis.
fn box_box(ha: Vec2, a_at: Transform, hb: Vec2, b_at: Transform) -> Option<Manifold> {
    let a_axes = box_axes(a_at);
    let b_axes = box_axes(b_at);
    let candidates = [
        (a_axes.0, false),
        (a_axes.1, false),
        (b_axes.0, true),
        (b_axes.1, true),
    ];

    let mut best: Option<Separation> = None;
    for (axis, from_second) in candidates {
        let depth = overlap_on(axis, ha, a_axes, a_at, hb, b_axes, b_at)?;
        // **Earliest in the enumeration wins a tie**, so a symmetric overlap
        // resolves the same way on every machine. Strictly-less is what
        // implements that: a later axis has to beat the incumbent outright.
        let better = best.as_ref().is_none_or(|current| depth < current.depth);
        if better {
            // Orient the axis from a toward b so the report's direction does
            // not depend on which face the axis came from.
            let facing = b_at.translation - a_at.translation;
            let normal = if facing.dot(axis) < Fixed::ZERO {
                -axis
            } else {
                axis
            };
            best = Some(Separation {
                normal,
                depth,
                from_second,
            });
        }
    }

    let separation = best?;
    let (reference_half, reference_at, incident_half, incident_at) = if separation.from_second {
        (hb, b_at, ha, a_at)
    } else {
        (ha, a_at, hb, b_at)
    };
    let points = clip_incident_face(
        separation.normal,
        separation.from_second,
        reference_half,
        reference_at,
        incident_half,
        incident_at,
    );

    Some(Manifold {
        normal: separation.normal,
        points,
        count: 2,
    })
    .map(|manifold| trim(manifold, separation.depth))
}

/// Drop points whose depth came out negative, which clipping can produce at a
/// corner, and fall back to the deepest single point if none survive.
fn trim(manifold: Manifold, depth: Fixed) -> Manifold {
    let mut kept = [ContactPoint {
        position: Vec2::ZERO,
        depth: Fixed::ZERO,
    }; MAX_MANIFOLD_POINTS];
    let mut count = 0usize;
    for point in manifold.points {
        if point.depth >= Fixed::ZERO && count < MAX_MANIFOLD_POINTS {
            if let Some(slot) = kept.get_mut(count) {
                *slot = point;
            }
            count += 1;
        }
    }
    if count == 0 {
        return Manifold::single(manifold.normal, manifold.points[0].position, depth);
    }
    Manifold {
        normal: manifold.normal,
        points: kept,
        count: u8::try_from(count).unwrap_or(1),
    }
}

/// The incident box's most opposed face, clipped to the reference face's span.
///
/// In two dimensions this is the whole of manifold generation: the contact is
/// the overlap of two segments, so the answer is the incident segment cut
/// against the reference face's two side planes.
fn clip_incident_face(
    normal: Vec2,
    from_second: bool,
    reference_half: Vec2,
    reference_at: Transform,
    incident_half: Vec2,
    incident_at: Transform,
) -> [ContactPoint; MAX_MANIFOLD_POINTS] {
    // The separating normal points from a toward b; from the reference box's
    // point of view it points outward only when the reference box is a.
    let outward = if from_second { -normal } else { normal };
    let incident_axes = box_axes(incident_at);

    // The incident face is the one whose outward normal most opposes ours.
    let along_x = incident_axes.0.dot(outward);
    let along_y = incident_axes.1.dot(outward);
    let (face_normal, face_half, edge_axis, edge_half) = if along_x.abs() >= along_y.abs() {
        let sign = if along_x > Fixed::ZERO {
            Fixed::from_int(-1)
        } else {
            Fixed::ONE
        };
        (
            incident_axes.0 * (sign),
            incident_half.x,
            incident_axes.1,
            incident_half.y,
        )
    } else {
        let sign = if along_y > Fixed::ZERO {
            Fixed::from_int(-1)
        } else {
            Fixed::ONE
        };
        (
            incident_axes.1 * (sign),
            incident_half.y,
            incident_axes.0,
            incident_half.x,
        )
    };

    let face_centre = incident_at.translation + face_normal * (face_half);
    let ends = [
        face_centre + edge_axis * (edge_half),
        face_centre - edge_axis * (edge_half),
    ];

    // The reference face's tangent, and how far the reference box spans along
    // it — that span is what the incident segment is clipped to.
    let tangent = outward.perpendicular();
    let reference_axes = box_axes(reference_at);
    let span = box_reach(reference_half, reference_axes, tangent);
    let centre_along = reference_at.translation.dot(tangent);
    let reference_depth = box_reach(reference_half, reference_axes, outward);
    let surface = reference_at.translation.dot(outward) + reference_depth;

    let mut clipped = [ContactPoint {
        position: Vec2::ZERO,
        depth: Fixed::ZERO,
    }; MAX_MANIFOLD_POINTS];
    for (slot, end) in ends.into_iter().enumerate() {
        let along = end
            .dot(tangent)
            .clamp(centre_along - span, centre_along + span);
        // Rebuild the point on the incident segment at the clamped position.
        let shift = along - end.dot(tangent);
        let position = end + tangent * (shift);
        let depth = surface - position.dot(outward);
        if let Some(cell) = clipped.get_mut(slot) {
            *cell = ContactPoint { position, depth };
        }
    }
    clipped
}
