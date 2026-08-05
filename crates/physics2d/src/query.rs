//! Asking the world what is where.

use crate::narrow::collide;
use crate::ray::cast;
use crate::shape::{Shape, Transform};
use crate::world::{Collider, World};
use renew_ecs::Entity;
use renew_fixed::{Fixed, Vec2};

/// How much a query found, and how much it could write down.
///
/// **Both numbers, always.** A query that returned only what fitted would let
/// a world quietly stop reporting under load — the same shape of failure as a
/// gate that passes while measuring nothing. A caller that sees the two differ
/// knows it lost information and can grow its buffer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// How many results were written into the caller's buffer.
    pub written: usize,
    /// How many existed. Never less than `written`.
    pub existed: usize,
}

impl Counts {
    /// Whether the buffer was too small.
    #[must_use]
    pub const fn truncated(self) -> bool {
        self.existed > self.written
    }
}

/// Bodies a query must ignore.
///
/// Names **bodies**, not colliders: excluding a body excludes all its shapes,
/// because the reason to exclude one is almost always "this is me" and a body
/// is what "me" means. A handle naming no body excludes nothing and the query
/// answers — refusing there would collide with the legitimate empty answer.
#[derive(Clone, Copy, Debug, Default)]
pub struct Exclude<'a> {
    handles: &'a [Entity],
}

impl<'a> Exclude<'a> {
    /// Exclude nothing.
    pub const NONE: Self = Self { handles: &[] };

    /// Exclude these bodies.
    #[must_use]
    pub const fn bodies(handles: &'a [Entity]) -> Self {
        Self { handles }
    }

    fn covers(self, handle: Entity) -> bool {
        let mut index = 0;
        while index < self.handles.len() {
            if let Some(&excluded) = self.handles.get(index)
                && excluded == handle
            {
                return true;
            }
            index += 1;
        }
        false
    }
}

/// Where a world-space ray met something.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    /// What it met.
    pub collider: Collider,
    /// Distance along the direction.
    pub distance: Fixed,
    /// Where, in world space.
    pub point: Vec2,
    /// The surface normal, facing the ray.
    pub normal: Vec2,
}

impl World {
    /// Whether a collider is visible to a query with this mask and exclusion.
    pub(crate) fn query_visible(
        &self,
        collider: Collider,
        mask: u32,
        exclude: Exclude<'_>,
    ) -> Option<(Shape, Transform)> {
        if exclude.covers(collider.handle) {
            return None;
        }
        let (shape, _, filter) = self.shape(collider)?;
        if !filter.visible_to_query(mask) {
            return None;
        }
        Some((shape, self.world_transform(collider)?))
    }

    /// Every collider containing this point.
    ///
    /// Multi-valued: a point inside three overlapping shapes is inside three
    /// of them, and picking one would be answering a different question.
    pub fn point_query(
        &self,
        point: Vec2,
        mask: u32,
        exclude: Exclude<'_>,
        out: &mut [Collider],
    ) -> Counts {
        let mut counts = Counts::default();
        for collider in self.colliders() {
            let Some((shape, at)) = self.query_visible(collider, mask, exclude) else {
                continue;
            };
            // A point is a zero-radius circle, and the geometry already knows
            // how to answer that — one implementation rather than a second
            // containment test that can disagree with the first.
            let dot = Shape::Circle {
                radius: Fixed::ZERO,
            };
            if collide(dot, Transform::at(point), shape, at).is_none() {
                continue;
            }
            if let Some(slot) = out.get_mut(counts.written) {
                *slot = collider;
                counts.written += 1;
            }
            counts.existed += 1;
        }
        counts
    }

    /// The nearest thing along a ray, or nothing.
    ///
    /// **Ties go to the lowest collider**, which matters more than it sounds:
    /// in a tile-aligned world almost everything shares an edge, so a ray down
    /// a seam meets two tiles at the same distance and "nearest" would
    /// otherwise be whichever the iteration happened to reach first.
    #[must_use]
    pub fn ray_query(
        &self,
        origin: Vec2,
        direction: Vec2,
        max_distance: Fixed,
        mask: u32,
        exclude: Exclude<'_>,
    ) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        for collider in self.colliders() {
            let Some((shape, at)) = self.query_visible(collider, mask, exclude) else {
                continue;
            };
            let Some(hit) = cast(origin, direction, max_distance, shape, at) else {
                continue;
            };
            // Strictly nearer, so an equal distance leaves the incumbent —
            // and the incumbent is the lower collider, because `colliders()`
            // ascends.
            let nearer = best
                .as_ref()
                .is_none_or(|current| hit.distance < current.distance);
            if nearer {
                best = Some(Hit {
                    collider,
                    distance: hit.distance,
                    point: hit.point,
                    normal: hit.normal,
                });
            }
        }
        best
    }

    /// Every collider a shape placed here would intersect.
    ///
    /// Membership only. A caller wanting depth and normals is asking for a
    /// contact, which is what the step produces.
    pub fn overlap_query(
        &self,
        shape: Shape,
        at: Transform,
        mask: u32,
        exclude: Exclude<'_>,
        out: &mut [Collider],
    ) -> Counts {
        let mut counts = Counts::default();
        for collider in self.colliders() {
            let Some((other, other_at)) = self.query_visible(collider, mask, exclude) else {
                continue;
            };
            if collide(shape, at, other, other_at).is_none() {
                continue;
            }
            if let Some(slot) = out.get_mut(counts.written) {
                *slot = collider;
                counts.written += 1;
            }
            counts.existed += 1;
        }
        counts
    }
}
