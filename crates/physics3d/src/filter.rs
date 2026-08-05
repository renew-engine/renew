//! Which pairs are allowed to collide, and which shapes a query can see.

/// A bit set naming what a shape *is*, and what it *cares about*.
///
/// # Why both halves, and why per shape
///
/// The body-kind matrix decides only static/kinematic/dynamic, and two
/// kinematic bodies always collide by it. A game needs them not to: the
/// character ignores pickups but collides with boxes, a bullet ignores
/// whatever fired it, the ground query ignores the character. None of that
/// is expressible without a filter, and no game can be written without all
/// three.
///
/// It lives on the shape rather than the body because a body may own several
/// — a one-way platform is one body whose top and volume filter differently,
/// and one filter per body pushes the caller back into splitting it into two
/// bodies it must then keep in step by hand.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Filter {
    /// What this shape is.
    pub layer: u32,
    /// What this shape collides with.
    pub mask: u32,
}

impl Filter {
    /// A shape on every layer that collides with everything.
    pub const ALL: Self = Self {
        layer: u32::MAX,
        mask: u32::MAX,
    };

    /// A shape on no layer that collides with nothing — invisible to pair
    /// tests, and still visible to a query whose mask names its layer, which
    /// is `NONE`'s point of difference from simply not existing.
    pub const NONE: Self = Self { layer: 0, mask: 0 };

    /// Layer and mask, in that order.
    #[must_use]
    pub const fn new(layer: u32, mask: u32) -> Self {
        Self { layer, mask }
    }

    /// Whether two shapes are eligible to collide.
    ///
    /// **Symmetric by construction**, so a pair cannot be eligible in one
    /// direction only — which would make the answer depend on which of the
    /// two the broadphase happened to visit first, and that is precisely the
    /// class of storage-order dependence the ordering rules exist to remove.
    #[must_use]
    pub const fn eligible(self, other: Self) -> bool {
        (self.layer & other.mask) != 0 && (other.layer & self.mask) != 0
    }

    /// Whether a query carrying `mask` can see this shape.
    ///
    /// **One-sided, and deliberately.** A query has no layer, so the
    /// symmetric rule cannot be evaluated for one at all. Consulting the
    /// shape's own mask instead would make a trigger configured with an empty
    /// mask — the ordinary way to write "collides with nothing" — invisible
    /// to a ray, and casting against triggers is a large part of why rays
    /// exist.
    #[must_use]
    pub const fn visible_to_query(self, mask: u32) -> bool {
        (self.layer & mask) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::Filter;

    #[test]
    fn eligibility_is_symmetric() {
        let character = Filter::new(0b0001, 0b0010);
        let wall = Filter::new(0b0010, 0b0001);
        assert!(character.eligible(wall));
        assert!(wall.eligible(character));
    }

    #[test]
    fn one_sided_interest_is_not_enough() {
        // The character wants to collide with the pickup; the pickup does not
        // want to collide with anything. No contact, in either direction.
        let character = Filter::new(0b0001, 0b1111);
        let pickup = Filter::new(0b0100, 0b0000);
        assert!(!character.eligible(pickup));
        assert!(!pickup.eligible(character));
    }

    /// The case the one-sided query rule exists for: a shape that collides
    /// with nothing must still be findable by a cast, or triggers cannot be
    /// implemented.
    #[test]
    fn a_shape_that_collides_with_nothing_is_still_visible_to_a_query() {
        let trigger = Filter::new(0b0100, 0b0000);
        assert!(!trigger.eligible(Filter::ALL), "it blocks nothing");
        assert!(
            trigger.visible_to_query(0b0100),
            "and a query naming its layer still finds it"
        );
        assert!(
            !trigger.visible_to_query(0b1011),
            "while a query not naming its layer does not"
        );
    }

    #[test]
    fn the_constants_mean_what_they_say() {
        assert!(Filter::ALL.eligible(Filter::ALL));
        assert!(!Filter::NONE.eligible(Filter::ALL));
        assert!(!Filter::NONE.eligible(Filter::NONE));
        assert!(Filter::ALL.visible_to_query(1));
        assert!(!Filter::NONE.visible_to_query(u32::MAX));
    }
}
