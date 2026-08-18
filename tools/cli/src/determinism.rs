//! Cross-platform determinism: emit one target's digests, compare several.
//!
//! A simulation that reproduces on the machine that wrote it has proved
//! an unseeded generator absent. It has not proved the machine did not
//! matter, and it cannot — both halves of that comparison ran on the same
//! machine. The only evidence for the second claim is two machines
//! disagreeing, or failing to.
//!
//! So this is two modes and they are deliberately asymmetric. **Emit**
//! runs the pinned simulations on whatever target it finds itself on and
//! writes what it saw. **Compare** takes several of those and holds them
//! against each other — never against a committed constant, which is a
//! regression guard and says nothing about portability.
//!
//! # Why the comparison is here and not in workflow YAML
//!
//! Because it has to be testable, and because a developer staring at a
//! red lane has to be able to reproduce it locally. A gate whose logic
//! lives only in CI configuration is a gate no test can reach, which is
//! how a gate ends up passing while measuring nothing — a failure this
//! repository has paid for and does not intend to repeat.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Every (platform, instruction set) pair the determinism claim binds,
/// one row per lane leg.
///
/// **This list is the claim.** A row with no leg is a target asserted and
/// never exercised; a leg with no row is a target proved and never
/// claimed. Either is a lie in one direction, so the comparison requires
/// the reported set to match this one exactly rather than merely counting
/// to the same number — three legs on one instruction set satisfy a count
/// of three and prove strictly less.
///
/// Widening it is not a code change in spirit even though it is one in
/// fact: adding a row changes what the engine promises, and the row and
/// its lane leg land together or the lane starts lying.
pub const TARGETS: [(&str, &str); 5] = [
    ("linux", "x86_64"),
    ("windows", "x86_64"),
    ("macos", "aarch64"),
    ("android", "x86_64"),
    ("ios-simulator", "aarch64"),
];

/// The rows [`TARGETS`] binds, in the shape [`compare`] wants.
///
/// Whole rows rather than their instruction sets alone: a fleet that
/// swapped a windows runner for a second macos one would keep the
/// architecture multiset intact while leaving a bound platform
/// unexercised, and a gate that could not see the difference would call
/// that agreement.
#[must_use]
pub fn expected_rows() -> Vec<(&'static str, &'static str)> {
    TARGETS.to_vec()
}

/// The simulations the cross-platform lane compares, and the exact
/// arguments that pin them.
///
/// Each entry names a run whose every output is a function of the flags
/// beside it. Widening this list widens what the claim covers; it is a
/// list rather than one run because a single configuration
/// exercises one path through the world, and a divergence in a path the
/// list never walks is a divergence the lane never sees.
/// One simulation the lane pins: what to call it, which package answers,
/// what to pass, and which fields of the answer carry a digest.
pub type PinnedRun = (
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);

/// Glide reports two hashes; every run added since reports one.
const GLIDE_FIELDS: &[&str] = &["schedule_hash", "state_hash"];
const ONE_DIGEST: &[&str] = &["digest"];

pub const PINNED_RUNS: [PinnedRun; 11] = [
    // The widget tree: a scripted menu session through the fixed-point
    // solver, the hit-tester, and the decision fold. Everything in it
    // is integer arithmetic, and this row is what turns that claim
    // into three targets agreeing rather than one target asserting.
    ("ui/menu-16", "renew-ui", &[], ONE_DIGEST),
    // Four lockstep peers played against a scripted hostile link: loss,
    // duplication, reordering, one-way blackouts and a silent peer. The
    // digest folds the confirmed input stream and nothing else, so what
    // three targets are being asked to agree about is that arrival was
    // unobservable — not that their networks behaved the same, which
    // they did not even within one process.
    ("net/lockstep-4x600", "renew-net", &[], ONE_DIGEST),
    (
        "glide/seed-7-600",
        "renew-sample-glide",
        &["--seed", "7", "--frames", "600", "--json"],
        GLIDE_FIELDS,
    ),
    (
        "glide/seed-7-2000",
        "renew-sample-glide",
        &["--seed", "7", "--frames", "2000", "--json"],
        GLIDE_FIELDS,
    ),
    (
        "glide/seed-99-600",
        "renew-sample-glide",
        &["--seed", "99", "--frames", "600", "--json"],
        GLIDE_FIELDS,
    ),
    (
        "glide/sink-1500",
        "renew-sample-glide",
        &[
            "--seed",
            "3",
            "--frames",
            "1500",
            "--input-trace",
            "sink",
            "--json",
        ],
        GLIDE_FIELDS,
    ),
    // The platformer: swept motion against geometry, where a divergence would
    // come from the collision arithmetic rather than from a generator.
    (
        "leap/dash-600",
        "renew-sample-leap",
        &["--script", "dash", "--ticks", "600", "--json"],
        ONE_DIGEST,
    ),
    (
        "leap/hop-900",
        "renew-sample-leap",
        &["--script", "hop", "--ticks", "900", "--json"],
        ONE_DIGEST,
    ),
    // The voxel world: the same arithmetic in three dimensions, plus terrain
    // that the run itself edits.
    (
        "cube/patrol-600",
        "renew-sample-cube",
        &["--script", "patrol", "--ticks", "600", "--json"],
        ONE_DIGEST,
    ),
    (
        "cube/build-900",
        "renew-sample-cube",
        &["--script", "build", "--ticks", "900", "--json"],
        ONE_DIGEST,
    ),
    // Chess: no floating point and no geometry at all, so a divergence here
    // would be in the integer state itself rather than in any arithmetic the
    // other three share. A different kind of witness for the same claim.
    (
        "chess/play-60",
        "renew-sample-chess",
        &["--play", "--depth", "60", "--json"],
        ONE_DIGEST,
    ),
];

/// Every digest name [`PINNED_RUNS`] binds, spelled the way the emitting
/// side writes them: each run's name crossed with the fields its own
/// report carries.
///
/// [`compare`] holds each leg against this, so a leg that ran fewer
/// simulations than the list claims is inconclusive rather than agreed
/// with. The emitting side refuses to narrow a leg — two runs sharing a
/// digest name is an abort — and this is the reading side's half of the
/// same guarantee.
#[must_use]
pub fn expected_digest_names() -> Vec<String> {
    PINNED_RUNS
        .iter()
        .flat_map(|(name, _, _, fields)| fields.iter().map(move |field| digest_name(name, field)))
        .collect()
}

/// The one spelling of a digest's key: the run's name and the report
/// field it came from.
///
/// Both sides call this. Spelled twice, the two could drift and the
/// reader would refuse every leg the writer produced — a lane red for a
/// naming disagreement rather than for a disagreement about state.
#[must_use]
pub fn digest_name(run: &str, field: &str) -> String {
    format!("{run}/{field}")
}

/// The output schema this module reads and writes.
///
/// Present from the first release, because a consumer holding a document
/// it cannot version has to guess.
pub const SCHEMA_VERSION: u32 = 1;

/// One target's report: what it ran, and the environment it ran in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leg {
    /// Where the report came from, for error messages — a file name.
    pub origin: String,
    /// `target_os` for the platform this leg's runs executed on:
    /// [`platform_of_triple`] of the emitting command's `--target` when
    /// it had one, and `env::consts` of the emitting process otherwise.
    /// The two agree for a host build, which its own test pins.
    pub os: String,
    /// `target_arch`, likewise. Compared against the expected rows
    /// together with `os`, so a runner fleet that quietly changes
    /// platform or instruction set is caught rather than passing while
    /// proving less.
    pub arch: String,
    /// The exact `rustc --version` string. Legs that disagree here make
    /// the comparison inconclusive, not passing.
    pub toolchain: String,
    /// Digest name to digest, e.g. `glide/seed-7/state_hash`. A map so
    /// two legs that ran different sets are caught as a set difference
    /// rather than by position.
    pub digests: BTreeMap<String, String>,
}

/// What a comparison concluded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Every leg agreed, over a non-empty digest set.
    Agree { legs: usize, digests: usize },
    /// The comparison could not be made. **Not a pass.**
    Inconclusive(Vec<String>),
    /// The legs disagree — the finding this lane exists to produce.
    Diverged(Vec<String>),
}

impl Verdict {
    /// Only `Agree` is success. Stated as a method so no caller can treat
    /// `Inconclusive` as a pass by writing `!matches!(v, Diverged(_))`.
    #[must_use]
    pub const fn is_pass(&self) -> bool {
        matches!(self, Self::Agree { .. })
    }
}

/// Hold the legs against each other.
///
/// `expected_rows` is the whole set the target list binds — every
/// (os, arch) row must appear exactly once. Rows rather than
/// architectures, because a fleet that swapped one platform's runner for
/// another's leaves the architecture multiset intact while a bound
/// target goes unexercised. Passing an empty set is itself a refusal: a
/// comparison with nothing to expect cannot detect a fleet that narrowed.
///
/// The order of checks is the order of blame. Environment problems are
/// reported as `Inconclusive` *before* digests are compared at all,
/// because two legs on different compilers producing different digests
/// is not evidence of anything, and reporting it as divergence would send
/// somebody hunting a bug that is not there.
///
/// `expected_digests` is the set of digest names the pinned list binds.
/// The row check proves the lane ran everywhere it claims; this proves
/// each leg ran *everything* it claims. Without it a narrowing that is
/// uniform across legs — a stale artifact, a leg restored from an
/// earlier run, a table that grew after a leg was written — passes as
/// agreement, because legs carrying the same one digest agree perfectly
/// while proving a fraction of the claim.
///
/// Passing an empty set is a refusal, exactly as an empty row set is: a
/// comparison with nothing to expect cannot detect a fleet that
/// narrowed, and a caller who simply forgot the expectation must not be
/// handed a green for it. Both halves of the claim are supplied or the
/// comparison declines to make one.
#[must_use]
pub fn compare(
    legs: &[Leg],
    expected_rows: &[(&str, &str)],
    expected_digests: &[String],
) -> Verdict {
    // First, because a comparison of nothing is the one failure that
    // most resembles success. Placing it after the checks below would
    // leave it unreachable — the count check catches an empty slice — and
    // an unreachable refusal is a refusal nobody can test.
    let Some(reference) = legs.first() else {
        return Verdict::Inconclusive(vec![
            "no reports were supplied, so there was nothing to compare".to_string(),
        ]);
    };

    let blocked = collection_problems(legs, expected_rows, expected_digests, reference);
    if !blocked.is_empty() {
        return Verdict::Inconclusive(blocked);
    }
    digest_verdict(legs, reference)
}

/// Everything that makes the *collection* of legs unjudgeable — before a
/// single digest is looked at. Split from the digest comparison so each
/// half stays readable on its own; the order inside is the order of
/// blame the [`compare`] doc describes.
fn collection_problems(
    legs: &[Leg],
    expected_rows: &[(&str, &str)],
    expected_digests: &[String],
    reference: &Leg,
) -> Vec<String> {
    let mut blocked = Vec::new();

    // The breadth of what each leg ran, not only of where it ran. A leg
    // missing a bound digest proves less than the pinned list claims,
    // and a narrowing shared by every leg would otherwise read as
    // perfect agreement.
    for leg in legs {
        let missing: Vec<&str> = expected_digests
            .iter()
            .filter(|name| !leg.digests.contains_key(*name))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            blocked.push(format!(
                "leg `{}` is missing {} of the {} digests the pinned list binds, among them \
                 `{}` — a leg that ran less than the list claims proves less than the \
                 comparison would report",
                leg.origin,
                missing.len(),
                expected_digests.len(),
                missing[0]
            ));
        }
    }

    if expected_rows.is_empty() {
        blocked.push(
            "no expected targets were given, so this comparison could not have \
             detected a target set that narrowed"
                .to_string(),
        );
    }
    if expected_digests.is_empty() {
        blocked.push(
            "no expected digests were given, so this comparison could not have \
             detected legs that all ran less than the pinned list binds"
                .to_string(),
        );
    }
    if legs.len() < expected_rows.len() {
        blocked.push(format!(
            "expected {} legs, one per target row, and received {} — a missing leg is an \
             untested target, not a passing one",
            expected_rows.len(),
            legs.len()
        ));
    }

    // Anti-vacuity: a leg carrying nothing compares equal to every other
    // leg carrying nothing, and the lane would report success.
    for leg in legs {
        if leg.digests.is_empty() {
            blocked.push(format!(
                "leg `{}` carries no digests at all, so comparing it proves nothing",
                leg.origin
            ));
        }
    }

    // The bound set, matched row for whole row rather than counted or
    // matched on one column: three legs on one instruction set satisfy a
    // count of three, and a fleet that swapped a windows runner for a
    // second macos one keeps the architecture multiset intact — each
    // proves strictly less than the target list claims, and only the
    // whole (os, arch) pair can tell.
    let mut seen: Vec<String> = legs
        .iter()
        .map(|leg| format!("{}/{}", leg.os, leg.arch))
        .collect();
    seen.sort();
    let mut wanted: Vec<String> = expected_rows
        .iter()
        .map(|(os, arch)| format!("{os}/{arch}"))
        .collect();
    wanted.sort();
    if !expected_rows.is_empty() && seen != wanted {
        blocked.push(format!(
            "the legs report targets [{}] and the target list binds [{}] — the lane \
             is proving a different set than it claims",
            seen.join(", "),
            wanted.join(", ")
        ));
    }

    // One (os, arch) row, one leg. Two legs claiming the same row are one
    // target reported twice, and counting them as two would let the lane
    // claim a breadth it did not run — a duplicated leg plus one other
    // architecture can satisfy the arch multiset while proving only two
    // targets.
    for (index, leg) in legs.iter().enumerate() {
        if let Some(earlier) = legs[..index]
            .iter()
            .find(|earlier| earlier.os == leg.os && earlier.arch == leg.arch)
        {
            blocked.push(format!(
                "legs `{}` and `{}` both report {}/{} — the same target twice proves \
                 one target, and the comparison must not count it as two",
                earlier.origin, leg.origin, leg.os, leg.arch
            ));
        }
    }

    // Toolchain: a mismatch is inconclusive, never a quiet pass and never
    // reported as divergence.
    for leg in legs {
        if leg.toolchain != reference.toolchain {
            blocked.push(format!(
                "leg `{}` built with `{}` and leg `{}` with `{}` — determinism is \
                 claimed for one toolchain version, so this comparison is \
                 inconclusive rather than failing",
                reference.origin, reference.toolchain, leg.origin, leg.toolchain
            ));
            break;
        }
    }

    blocked
}

/// The digest comparison itself, reached only once the collection was
/// judged sound.
fn digest_verdict(legs: &[Leg], reference: &Leg) -> Verdict {
    let mut divergences = Vec::new();
    for leg in legs.iter().skip(1) {
        // A leg that ran a different set of simulations is a divergence
        // in itself: the comparison would otherwise silently cover only
        // the intersection.
        for name in reference.digests.keys() {
            if !leg.digests.contains_key(name) {
                divergences.push(format!(
                    "leg `{}` reported `{name}` and leg `{}` did not run it",
                    reference.origin, leg.origin
                ));
            }
        }
        for name in leg.digests.keys() {
            if !reference.digests.contains_key(name) {
                divergences.push(format!(
                    "leg `{}` reported `{name}` and leg `{}` did not run it",
                    leg.origin, reference.origin
                ));
            }
        }
        for (name, value) in &leg.digests {
            if let Some(expected) = reference.digests.get(name)
                && value != expected
            {
                // Both halves of the row, because the row is what the
                // comparison matches on: two legs differing only in
                // platform would otherwise print the same parenthetical
                // twice and read as one target contradicting itself.
                divergences.push(format!(
                    "`{name}` is {expected} on {} ({}/{}) and {value} on {} ({}/{}) — the \
                     same inputs produced different state on two targets",
                    reference.origin, reference.os, reference.arch, leg.origin, leg.os, leg.arch
                ));
            }
        }
    }

    if divergences.is_empty() {
        Verdict::Agree {
            legs: legs.len(),
            digests: reference.digests.len(),
        }
    } else {
        divergences.sort_unstable();
        divergences.dedup();
        Verdict::Diverged(divergences)
    }
}

/// Render a verdict for a person reading a failed lane.
#[must_use]
pub fn describe(verdict: &Verdict) -> String {
    let mut out = String::new();
    match verdict {
        Verdict::Agree { legs, digests } => {
            let _ = write!(
                out,
                "{legs} targets agree on all {digests} digests — the same inputs produce \
                 the same state on every target the list binds"
            );
        }
        Verdict::Inconclusive(reasons) => {
            out.push_str("INCONCLUSIVE — this is a failure, not a pass:\n");
            for reason in reasons {
                let _ = writeln!(out, "  - {reason}");
            }
        }
        Verdict::Diverged(items) => {
            out.push_str("DIVERGED — targets disagree:\n");
            for item in items {
                let _ = writeln!(out, "  - {item}");
            }
        }
    }
    out
}

/// Read a leg from the JSON one `--emit` run wrote.
///
/// Every field is required and every absence is named. A parser that
/// defaulted a missing digest set to empty, or a missing architecture to
/// the host's, would turn a broken emit step into a passing comparison —
/// which is the failure mode this whole module is arranged against.
///
/// # Errors
///
/// Returns a human-readable reason when the document is not a leg: bad
/// JSON, a schema version this build does not know, a missing or
/// wrong-typed field, or a digest that is not a hex string.
pub fn parse_leg(origin: &str, text: &str) -> Result<Leg, String> {
    let value = crate::json::parse(text)
        .map_err(|error| format!("`{origin}` is not readable as JSON: {error:?}"))?;
    let field = |name: &str| -> Result<String, String> {
        value
            .get(name)
            .and_then(crate::json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("`{origin}` has no string field `{name}`"))
    };

    // The version gate refuses forward as well as backward. A newer
    // emitter may mean something different by a field this build reads
    // by the same name, and guessing is how a comparison quietly
    // compares the wrong things.
    let version = match value.get("schema_version") {
        Some(crate::json::Value::Number(n)) => u32::try_from(*n).ok(),
        _ => None,
    }
    .ok_or_else(|| format!("`{origin}` has no readable `schema_version`"))?;
    if version != SCHEMA_VERSION {
        return Err(format!(
            "`{origin}` is schema version {version} and this build reads {SCHEMA_VERSION}"
        ));
    }

    let digests_value = value
        .get("digests")
        .ok_or_else(|| format!("`{origin}` has no `digests` object"))?;
    let mut digests = BTreeMap::new();
    for (name, entry) in digests_value
        .as_object()
        .ok_or_else(|| format!("`{origin}`'s `digests` is not an object"))?
    {
        let digest = entry
            .as_str()
            .ok_or_else(|| format!("`{origin}`'s digest `{name}` is not a string"))?;
        // The same rule the emitting side applies, so what this module
        // renders it can always read back.
        if !is_digest(digest) {
            return Err(format!(
                "`{origin}`'s digest `{name}` is `{digest}`, which is not a 0x-prefixed \
                 hex string — digests are strings so that no reader rounds one"
            ));
        }
        digests.insert(name.clone(), digest.to_string());
    }

    let toolchain = field("toolchain")?;
    if toolchain.trim().is_empty() {
        return Err(format!(
            "`{origin}` names no toolchain, and a comparison that cannot say which compiler \
             built its legs is inconclusive rather than passing"
        ));
    }
    Ok(Leg {
        origin: origin.to_string(),
        os: field("os")?,
        arch: field("arch")?,
        toolchain,
        digests,
    })
}

/// Render a leg as the JSON `--emit` writes and `parse_leg` reads.
///
/// Written by hand rather than through a serializer because this tree has
/// no serialization dependency and one document does not justify adding
/// one. The round-trip is tested, which is the property that matters.
#[must_use]
pub fn render_leg(leg: &Leg) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "{{\n  \"schema_version\": {SCHEMA_VERSION},\n  \"os\": \"{}\",\n  \
         \"arch\": \"{}\",\n  \"toolchain\": \"{}\",\n  \"digests\": {{",
        escape(&leg.os),
        escape(&leg.arch),
        escape(&leg.toolchain)
    );
    for (index, (name, digest)) in leg.digests.iter().enumerate() {
        let comma = if index + 1 == leg.digests.len() {
            ""
        } else {
            ","
        };
        let _ = write!(
            out,
            "\n    \"{}\": \"{}\"{comma}",
            escape(name),
            escape(digest)
        );
    }
    out.push_str("\n  }\n}\n");
    out
}

/// JSON string escaping, by the same rule the rest of this crate's
/// documents are written with.
///
/// It used to handle backslash and quote only, on the reasoning that
/// these fields "cannot contain" anything else — and `toolchain` is
/// whatever `rustc --version` printed, so a compiler wrapper that
/// prefixed a warning line put a raw newline inside a JSON string. The
/// document was invalid, `parse_leg` could not read it back, and the
/// emit still exited zero: a green over an artifact the next stage
/// cannot use. "Cannot contain" was a claim about the inputs of the day.
fn escape(text: &str) -> String {
    crate::json::escaped(text)
}

/// Pull the digests a pinned run reported out of what it printed.
///
/// The pure half of `--emit`: everything between "a child process wrote
/// some bytes" and "the report holds two digests". Separated from the
/// orchestration around it because the orchestration spawns a cargo run
/// per pinned scenario and cannot be reached by a unit test, while every way this can go
/// wrong can be — a run that printed nothing, printed something that is
/// not JSON, or printed JSON missing a hash the comparison needs.
///
/// `name` labels the run and prefixes every digest, so two runs that
/// disagree can be told apart by which configuration produced them.
///
/// # Errors
///
/// Returns a reason naming the run whenever its output cannot yield both
/// digests. Never returns a partial set: a report missing one hash is a
/// report proving half of what it claims, and half is reported as none.
pub fn digests_from_output(
    name: &str,
    stdout: &str,
    fields: &[&str],
) -> Result<Vec<(String, String)>, String> {
    let Some(line) = stdout
        .lines()
        .map(str::trim)
        .rev()
        .find(|line| line.starts_with('{'))
    else {
        return Err(format!(
            "the pinned run `{name}` printed no JSON object; a lane that accepted this \
             would compare an empty digest set against another empty one"
        ));
    };
    let report = crate::json::parse(line)
        .map_err(|error| format!("`{name}` printed unreadable JSON: {error:?}"))?;

    let mut digests = Vec::new();
    // **Which fields carry a digest is the run's business, not this
    // function's.** It was a fixed pair when one sample fed the lane; a second
    // sample with different field names would have been told its report
    // carried no `state_hash`, which is true and useless.
    for &field in fields {
        let Some(value) = report.get(field).and_then(crate::json::Value::as_str) else {
            return Err(format!("`{name}`'s report carries no `{field}`"));
        };
        // The same rule the comparison enforces when it reads a leg
        // back. Checked here too, so a run whose digest formatting
        // regressed reds the emit that produced it and names that run,
        // rather than surfacing a file away as an unreadable leg.
        if !is_digest(value) {
            return Err(format!(
                "`{name}`'s `{field}` is `{value}`, which is not a 0x-prefixed hex \
                 string — digests are strings so that no reader rounds one"
            ));
        }
        digests.push((digest_name(name, field), value.to_string()));
    }
    Ok(digests)
}

/// The platform a target triple names, in the words a leg uses — or
/// nothing, for a triple this lane has not been taught.
///
/// **Deliberately the same words `env::consts` produces**, because a
/// leg emitted for the host and a leg emitted for a device are compared
/// against one table: `x86_64-linux-android` has to read as
/// `("android", "x86_64")` exactly as a binary running there would
/// report itself, or the comparison would refuse a leg for spelling
/// rather than for disagreeing.
///
/// **One row deliberately breaks that rule, and it is the more
/// important rule that makes it.** A binary built for
/// `aarch64-apple-ios-sim` reports `ios` from `env::consts`, exactly as
/// one built for a phone does — so following the spelling would give
/// the two triples one identity, and a determinism row reading
/// `ios/aarch64` would promise that phones agree when only simulators
/// had ever been measured. The simulator is therefore named
/// `ios-simulator`, which no binary reports, because **the words exist
/// to say what was proved and a row that overclaims is worse than a row
/// that is oddly spelled.**
///
/// This costs nothing elsewhere: `env::consts` is read only when no
/// `--target` is given, which happens only on the three desktop rows,
/// where the spellings still match exactly and a test pins that they
/// do.
///
/// **A table, not a parser, and the difference is the whole point.**
/// Neither column can be read off a triple by position. `android` lives
/// in the *environment* field of `x86_64-linux-android`, whose os field
/// says `linux`. And the leading field is a spelling, not a
/// `target_arch`: `armv7-linux-androideabi` builds with
/// `target_arch = "arm"`, `i686-pc-windows-msvc` with `x86`, and the two
/// disagree on **more than half** the targets rustc ships — 163 of the
/// 322 the pinned toolchain lists, counted rather than estimated.
/// Guessing either column mislabels a leg — and a mislabelled leg is
/// worse than a refused one, because it is then compared against rows
/// it does not belong to.
///
/// So an unknown triple is refused **here, where the label is made**,
/// rather than left for a downstream check to catch: this function
/// cannot answer for a target nobody has stated the answer for, and
/// saying so is the only honest return. Teaching the lane a new target
/// is one row below, added beside the row that puts it in CI.
#[must_use]
pub fn platform_of_triple(triple: &str) -> Option<(&'static str, &'static str)> {
    KNOWN_TARGETS
        .iter()
        .find(|(known, _, _)| *known == triple)
        .map(|(_, os, arch)| (*os, *arch))
}

/// Triple, then the `target_os` and `target_arch` a binary built for it
/// reports — transcribed from the compiler, never derived from the text.
///
/// A module constant rather than a local one so that a test can walk it
/// and hold every row against `rustc --print cfg`. Held only against a
/// copy of itself, a table proves nothing: the copy would be wrong in
/// the same way.
pub const KNOWN_TARGETS: [(&str, &str, &str); 7] = [
    ("x86_64-unknown-linux-gnu", "linux", "x86_64"),
    ("x86_64-pc-windows-msvc", "windows", "x86_64"),
    ("aarch64-apple-darwin", "macos", "aarch64"),
    ("aarch64-linux-android", "android", "aarch64"),
    ("x86_64-linux-android", "android", "x86_64"),
    ("aarch64-apple-ios", "ios", "aarch64"),
    ("aarch64-apple-ios-sim", "ios-simulator", "aarch64"),
];

/// Build one pinned run's cargo command line.
///
/// Split out of the emit loop so that **the pass-through can be
/// asserted without a device**. It is the whole mechanism by which a
/// leg comes from somewhere else — delete it and cargo builds for the
/// host, the runner is never invoked, and the leg is still stamped with
/// the triple's platform because the label comes from the flag rather
/// than from the run. That failure is silent and looks exactly like
/// success, so it is pinned here rather than left to a lane.
///
/// `--target` must precede the `--` separator: everything after it
/// belongs to the program, not to cargo.
#[must_use]
pub fn pinned_invocation<'a>(
    package: &'a str,
    args: &[&'a str],
    target: Option<&'a str>,
) -> Vec<&'a str> {
    let mut invocation = vec!["run", "--quiet", "--package", package];
    if let Some(triple) = target {
        invocation.push("--target");
        invocation.push(triple);
    }
    invocation.push("--");
    invocation.extend_from_slice(args);
    invocation
}
/// The one spelling of what a digest looks like: `0x` and at least one
/// hex digit, all the way down. Hex strings rather than numbers because
/// a `u64` exceeds what a JSON number carries exactly, and a consumer
/// that rounded one would report two different states as the same.
#[must_use]
fn is_digest(value: &str) -> bool {
    value
        .strip_prefix("0x")
        .is_some_and(|tail| !tail.is_empty() && tail.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    /// Every triple this project builds reads as the words a binary
    /// running there would report about itself. The desktop three are
    /// the check that matters most: they are already emitted from
    /// `env::consts`, so the two paths must agree or one leg would be
    /// refused for its spelling rather than its digests.
    #[test]
    fn a_triple_names_the_platform_the_way_a_binary_there_would() {
        use super::platform_of_triple;

        // The host's own triple, assembled from a constant the compiler
        // picks. `#[cfg]` on three definitions rather than `cfg!` in
        // three branches, because the branch form compiles all three and
        // executes one — leaving the other two as regions no run on this
        // platform can reach, and a coverage gap that lands on a
        // different pair of lines on every platform that measures it.
        #[cfg(windows)]
        const HOST_REST: &str = "pc-windows-msvc";
        #[cfg(target_os = "macos")]
        const HOST_REST: &str = "apple-darwin";
        #[cfg(all(not(windows), not(target_os = "macos")))]
        const HOST_REST: &str = "unknown-linux-gnu";

        for (triple, os, arch) in [
            ("x86_64-unknown-linux-gnu", "linux", "x86_64"),
            ("x86_64-pc-windows-msvc", "windows", "x86_64"),
            ("aarch64-apple-darwin", "macos", "aarch64"),
            ("x86_64-linux-android", "android", "x86_64"),
            ("aarch64-linux-android", "android", "aarch64"),
            ("aarch64-apple-ios", "ios", "aarch64"),
            ("aarch64-apple-ios-sim", "ios-simulator", "aarch64"),
        ] {
            assert_eq!(platform_of_triple(triple), Some((os, arch)), "{triple}");
        }

        // The one this process can check against the real thing: the
        // host's own triple must read as the host's own constants.
        let host = format!("{}-{HOST_REST}", std::env::consts::ARCH);
        assert_eq!(
            platform_of_triple(&host),
            Some((std::env::consts::OS, std::env::consts::ARCH)),
            "the two identity paths disagree about this very machine"
        );

        // Every row held against the compiler, which is the authority
        // on what a binary built for a triple reports about itself.
        // Without this the table is checked against a copy of itself
        // written in the same sitting, which is worth nothing: a row
        // wrong in both columns passes, and the four mobile rows — the
        // reason this function exists — are exercised by no lane that
        // can fail. `--print cfg` answers for a target whether or not
        // its standard library is installed, so this runs anywhere the
        // suite does.
        for (triple, os, arch) in super::KNOWN_TARGETS {
            let printed = std::process::Command::new("rustc")
                .args(["--print", "cfg", "--target", triple])
                .output()
                .unwrap_or_else(|error| panic!("could not ask rustc about {triple}: {error}"));
            assert!(
                printed.status.success(),
                "rustc refused to describe {triple}, so this row is unverifiable"
            );
            let cfg = String::from_utf8_lossy(&printed.stdout);
            // Compared as `Option`, so a key rustc did not print fails
            // as `None` against the expected value rather than through
            // an arm of its own. An arm no run can take is a coverage
            // hole that has to be argued for, and this needs no arm.
            let value = |key: &str| {
                cfg.lines()
                    .find_map(|line| line.strip_prefix(key))
                    .map(|rest| rest.trim_matches('"'))
            };
            assert_eq!(
                value("target_arch="),
                Some(arch),
                "target_arch for {triple}"
            );

            // **The one row that deliberately disagrees with rustc, and
            // it is checked harder than the others rather than
            // excused.** The simulator triple reports `ios`, exactly as
            // the device triple does, so following rustc would give the
            // two one identity and let a determinism row promise phones
            // on a simulator's evidence. Both halves are asserted: that
            // rustc still says `ios` (if that ever changed, the reason
            // for the exception would have changed with it) and that
            // this table still says something else.
            if triple == "aarch64-apple-ios-sim" {
                assert_eq!(
                    value("target_os="),
                    Some("ios"),
                    "the simulator triple no longer reports `ios`, so the reason this row \
                     departs from rustc needs re-reading"
                );
                assert_eq!(
                    os, "ios-simulator",
                    "the simulator must not share a name with the device it stands in for"
                );
            } else {
                assert_eq!(value("target_os="), Some(os), "target_os for {triple}");
            }
        }

        // A triple nobody has stated the answer for gets no answer.
        // Both of these would have been mislabelled by reading the
        // fields off by position: the first has no `target_arch` called
        // `armv7` and no os field naming android, and the second is a
        // target this lane does not build at all.
        for unknown in ["armv7-linux-androideabi", "wasm32-unknown-unknown"] {
            assert_eq!(platform_of_triple(unknown), None, "{unknown}");
        }
    }

    use super::{Leg, Verdict, compare, describe, parse_leg, render_leg};
    use std::collections::BTreeMap;

    /// The pass-through is the whole mechanism by which a leg comes
    /// from somewhere else, and it is invisible from the outside: with
    /// it deleted, cargo builds for the host, the runner is never
    /// invoked, and the leg is *still* stamped with the triple's
    /// platform, because the label comes from the flag rather than from
    /// the run. The artifact would be byte-identical to a desktop leg
    /// and the lane would report agreement it never measured. So the
    /// argv is asserted here, where no device is required.
    #[test]
    fn a_triple_reaches_cargo_ahead_of_the_program_s_own_arguments() {
        use super::pinned_invocation;

        let args = ["--play", "--json"];

        assert_eq!(
            pinned_invocation("renew-sample-chess", &args, None),
            [
                "run",
                "--quiet",
                "--package",
                "renew-sample-chess",
                "--",
                "--play",
                "--json"
            ],
            "without a triple the command must be exactly what it always was"
        );

        let targeted = pinned_invocation("renew-sample-chess", &args, Some("x86_64-linux-android"));
        assert_eq!(
            targeted,
            [
                "run",
                "--quiet",
                "--package",
                "renew-sample-chess",
                "--target",
                "x86_64-linux-android",
                "--",
                "--play",
                "--json"
            ]
        );

        // Stated separately from the equality above, because this is the
        // property rather than the spelling: everything after `--` is the
        // program's, so a triple that drifted past it would be handed to
        // the simulation as an argument instead of to cargo as a target.
        let separator = targeted.iter().position(|word| *word == "--").unwrap();
        let flag = targeted
            .iter()
            .position(|word| *word == "--target")
            .unwrap();
        assert!(flag < separator, "--target must precede the `--` separator");
    }

    /// The rows the `three()` fixture reports, in a different order than
    /// it builds them — the comparison matches multisets, not positions.
    const ROWS: [(&str, &str); 3] = [
        ("macos", "aarch64"),
        ("linux", "x86_64"),
        ("windows", "x86_64"),
    ];

    /// The digest names that fixture's legs carry, standing in for the
    /// pinned list's own.
    fn digests() -> Vec<String> {
        vec!["glide/state".to_string()]
    }

    fn leg(origin: &str, arch: &str, pairs: &[(&str, &str)]) -> Leg {
        Leg {
            origin: origin.to_string(),
            // The origin's stem, so the three-row fixture spans three
            // distinct (os, arch) targets the way the real lane does.
            os: origin.trim_end_matches(".json").to_string(),
            arch: arch.to_string(),
            toolchain: "rustc 1.97.0".to_string(),
            digests: pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        }
    }

    fn three(a: &str, b: &str, c: &str) -> Vec<Leg> {
        vec![
            leg("linux.json", "x86_64", &[("glide/state", a)]),
            leg("windows.json", "x86_64", &[("glide/state", b)]),
            leg("macos.json", "aarch64", &[("glide/state", c)]),
        ]
    }

    /// The bound set is a list of whole rows rather than a number or a
    /// column: a row with no leg is a target promised and never proved.
    #[test]
    fn the_expected_rows_come_from_the_target_rows() {
        let rows = super::expected_rows();
        assert_eq!(rows.len(), super::TARGETS.len());
        for (platform, arch) in super::TARGETS {
            assert!(
                rows.contains(&(platform, arch)),
                "{platform}/{arch} is bound and the expected set omits it"
            );
        }
        // Not a set of architectures: two rows may share an instruction
        // set, and collapsing them would let two legs satisfy three rows
        // — and would blind the comparison to a swapped platform.
        assert!(
            rows.iter().filter(|(_, arch)| *arch == "x86_64").count() > 1,
            "the fixture for this property needs two rows sharing an instruction set"
        );
    }

    /// Breadth in the other dimension. Legs that all ran the same
    /// *fraction* of the pinned list agree with each other perfectly, so
    /// without an expectation about what they should have run, a
    /// uniformly narrowed fleet reads as full agreement.
    #[test]
    fn a_leg_missing_a_bound_digest_is_inconclusive_not_agreement() {
        let legs = three("0x1", "0x1", "0x1");
        // The list binds a second simulation none of these legs ran.
        let bound = vec![
            "glide/state".to_string(),
            "chess/play-60/digest".to_string(),
        ];
        let verdict = compare(&legs, &ROWS, &bound);
        assert!(
            !verdict.is_pass(),
            "a narrowed leg proves less than claimed"
        );
        let text = describe(&verdict);
        assert!(text.contains("INCONCLUSIVE"), "{text}");
        assert!(
            text.contains("chess/play-60/digest"),
            "the reason names a simulation that went unrun: {text}"
        );

        // And the same legs against the list they actually satisfy still
        // agree — the check is an expectation, not a floor on count.
        assert!(compare(&legs, &ROWS, &digests()).is_pass());
    }

    /// The whole point of matching rows rather than architectures: a
    /// fleet that swaps one platform's runner for another's keeps the
    /// architecture multiset intact while a bound target goes unrun.
    #[test]
    fn a_swapped_platform_is_inconclusive_even_with_the_arches_intact() {
        let mut legs = three("0x1", "0x1", "0x1");
        // The windows leg becomes a second macos one: still two x86_64
        // and one aarch64, still three legs, still all agreeing — and
        // windows was never exercised.
        legs[1].os = "macos".to_string();
        let verdict = compare(&legs, &ROWS, &digests());
        assert!(!verdict.is_pass(), "a bound target went unrun");
        let text = describe(&verdict);
        assert!(text.contains("INCONCLUSIVE"), "{text}");
        assert!(
            text.contains("windows/x86_64"),
            "the reason names the target that went unproved: {text}"
        );
    }

    const SAMPLE_LINE: &str = concat!(
        r#"{"schema_version":1,"sample":"glide","seed":7,"source":"soar","frames":600,"#,
        r#""ticks":600,"dropped":0,"score":3,"alive":true,"#,
        r#""schedule_hash":"0x55ce27c8dcb97c4d","state_hash":"0xe191d32ff48fb06a"}"#
    );

    #[test]
    fn a_runs_output_yields_both_of_its_digests_prefixed_by_the_run() {
        let digests = super::digests_from_output(
            "glide/seed-7",
            SAMPLE_LINE,
            &["schedule_hash", "state_hash"],
        )
        .expect("parses");
        assert_eq!(
            digests,
            vec![
                (
                    "glide/seed-7/schedule_hash".to_string(),
                    "0x55ce27c8dcb97c4d".to_string()
                ),
                (
                    "glide/seed-7/state_hash".to_string(),
                    "0xe191d32ff48fb06a".to_string()
                ),
            ]
        );
    }

    /// The object is found by scanning backwards, so a run that logged
    /// before it reported still yields its digests — and a run that
    /// logged *another object* after it reports the last one, which is
    /// the report.
    #[test]
    fn chatter_around_the_object_does_not_hide_it() {
        let noisy = format!(
            "   Compiling something
warning: unused
{SAMPLE_LINE}
"
        );
        assert!(
            super::digests_from_output("run", &noisy, &["schedule_hash", "state_hash"]).is_ok()
        );

        // A `{`-leading line *after* the report is what makes the scan
        // direction observable: read forwards, this object would be
        // taken for the report and its missing fields refused.
        let trailing = format!("{SAMPLE_LINE}\n{{\"note\":\"done\"}}\n");
        let error = super::digests_from_output("run", &trailing, &["schedule_hash", "state_hash"])
            .expect_err("the last object is the report, and it carries no hashes");
        assert!(error.contains("schedule_hash"), "{error}");

        // And the report last, with an object before it: the same scan
        // direction finds the report rather than the chatter.
        let leading = format!("{{\"note\":\"starting\"}}\n{SAMPLE_LINE}\n");
        assert!(
            super::digests_from_output("run", &leading, &["schedule_hash", "state_hash"]).is_ok()
        );
    }

    /// Every way a run can fail to report, named rather than defaulted.
    /// A partial set is reported as none: a report missing one hash
    /// proves half of what it claims, and half must not reach the
    /// comparison as if it were whole.
    #[test]
    fn a_run_that_did_not_report_is_refused_by_name() {
        let cases = [
            ("silent", String::new()),
            (
                "chatter only",
                "   Compiling glide
"
                .to_string(),
            ),
            (
                "not json",
                "{not json at all
"
                .to_string(),
            ),
            (
                "no schedule hash",
                SAMPLE_LINE.replace("schedule_hash", "other_hash"),
            ),
            (
                "no state hash",
                SAMPLE_LINE.replace("state_hash", "other_hash"),
            ),
            // The emitting side holds digests to the same rule the
            // reading side does: a run whose formatting regressed reds
            // the emit that produced it, rather than surfacing a stage
            // later as a leg file nobody can read.
            (
                "digest without the prefix",
                SAMPLE_LINE.replace("0xe191d32ff48fb06a", "deadbeef"),
            ),
            (
                "digest that is not hex",
                SAMPLE_LINE.replace("0xe191d32ff48fb06a", "0xZZZZ"),
            ),
        ];
        for (label, stdout) in cases {
            let error = super::digests_from_output(
                "glide/seed-7",
                &stdout,
                &["schedule_hash", "state_hash"],
            )
            .expect_err("a run that did not report must be refused");
            assert!(
                error.contains("glide/seed-7"),
                "the `{label}` case must name the run: {error}"
            );
        }
    }

    /// The agreement line is a breadth claim — "N targets agree on all M
    /// digests" — so both numbers are what the fixture has to be able to
    /// tell from a constant. Two digests, three legs, and the rendered
    /// text asserted, or either count could be a literal.
    #[test]
    fn the_agreement_names_how_many_targets_and_how_many_digests() {
        let mut legs = three("0x1", "0x1", "0x1");
        for leg in &mut legs {
            leg.digests
                .insert("frame/schedule".to_string(), "0x7".to_string());
        }
        let verdict = compare(&legs, &ROWS, &digests());
        assert!(verdict.is_pass());
        let text = describe(&verdict);
        assert!(
            text.contains("3 targets agree on all 2 digests"),
            "the report counts both dimensions: {text}"
        );
    }

    #[test]
    fn three_agreeing_targets_pass() {
        let verdict = compare(&three("0x1", "0x1", "0x1"), &ROWS, &digests());
        assert_eq!(
            verdict,
            Verdict::Agree {
                legs: 3,
                digests: 1
            }
        );
        assert!(verdict.is_pass());
    }

    #[test]
    fn one_disagreeing_target_is_named_with_its_value() {
        let verdict = compare(&three("0x1", "0x1", "0xdead"), &ROWS, &digests());
        assert!(!verdict.is_pass());
        // Read through `describe`, which is what a person staring at a red
        // lane actually sees. Destructuring would test the shape and leave
        // the rendering — the part with a reader — untested.
        let text = describe(&verdict);
        assert!(text.contains("DIVERGED"), "{text}");
        assert!(text.contains("macos.json"), "{text}");
        assert!(text.contains("0xdead"), "{text}");
        // The whole row, not half of it: two legs that differ only in
        // platform would print the same parenthetical twice and read as
        // one target contradicting itself.
        assert!(text.contains("macos/aarch64"), "{text}");
        assert!(text.contains("linux/x86_64"), "{text}");

        // The case that makes the point: same instruction set, different
        // platforms, so the arch alone cannot tell the two legs apart.
        let mut legs = three("0x1", "0xdead", "0x1");
        legs[2].digests = legs[0].digests.clone();
        let text = describe(&compare(&legs, &ROWS, &digests()));
        assert!(text.contains("linux/x86_64"), "{text}");
        assert!(text.contains("windows/x86_64"), "{text}");
    }

    /// The check this lane's whole credibility rests on. A comparison of
    /// nothing is equality, and equality is what this lane reports as
    /// success — so a leg that ran no simulations must never reach the
    /// comparison at all.
    #[test]
    fn a_leg_with_no_digests_is_inconclusive_rather_than_equal() {
        let mut legs = three("0x1", "0x1", "0x1");
        legs[1].digests = BTreeMap::new();
        let verdict = compare(&legs, &ROWS, &digests());
        assert!(!verdict.is_pass());
        let text = describe(&verdict);
        assert!(text.contains("INCONCLUSIVE"), "{text}");
        assert!(text.contains("windows.json"), "{text}");
        // The reason this test is named for, not merely an inconclusive
        // verdict: the bound-digest check fires on the same fixture and
        // would satisfy a class-only assertion by itself.
        assert!(text.contains("carries no digests at all"), "{text}");
    }

    #[test]
    fn a_missing_leg_is_inconclusive_rather_than_two_legs_agreeing() {
        let mut legs = three("0x1", "0x1", "0x1");
        legs.pop();
        let verdict = compare(&legs, &ROWS, &digests());
        assert!(!verdict.is_pass());
        assert!(matches!(verdict, Verdict::Inconclusive(_)));
        // The count is named, not merely implied by the row mismatch:
        // the registry advertises "fewer targets than the engine claims"
        // as its own cause, so the wording is what a reader is promised.
        let text = describe(&verdict);
        assert!(
            text.contains("expected 3 legs") && text.contains("received 2"),
            "the reason states the shortfall: {text}"
        );
    }

    /// Three legs on one instruction set satisfy a count of three and
    /// prove strictly less than the target list claims. Counting is not
    /// enough; the set has to match row for row.
    #[test]
    fn the_wrong_architecture_set_is_inconclusive_even_at_the_right_count() {
        let mut legs = three("0x1", "0x1", "0x1");
        legs[2].arch = "x86_64".to_string();
        let verdict = compare(&legs, &ROWS, &digests());
        assert!(!verdict.is_pass());
        let text = describe(&verdict);
        assert!(text.contains("INCONCLUSIVE"), "{text}");
        assert!(text.contains("aarch64"), "{text}");
    }

    /// A duplicated leg plus one other architecture satisfies the arch
    /// multiset while proving only two targets — the arch check alone
    /// cannot see the os dimension, so the row check must.
    #[test]
    fn the_same_target_reported_twice_is_inconclusive_not_agreement() {
        let mut legs = three("0x1", "0x1", "0x1");
        // The windows row now claims linux/x86_64: agreeing digests, the
        // right count, the right arch multiset — and one target counted
        // twice.
        legs[1].os = legs[0].os.clone();
        let verdict = compare(&legs, &ROWS, &digests());
        assert!(!verdict.is_pass());
        let text = describe(&verdict);
        assert!(text.contains("INCONCLUSIVE"), "{text}");
        assert!(
            text.contains("twice"),
            "the reason names the duplication: {text}"
        );
    }

    /// Two compilers producing two digests is not evidence of a
    /// portability bug, and reporting it as one sends somebody hunting
    /// something that is not there.
    #[test]
    fn a_toolchain_mismatch_is_inconclusive_and_not_divergence() {
        let mut legs = three("0x1", "0x1", "0xdead");
        legs[2].toolchain = "rustc 1.98.0".to_string();
        let verdict = compare(&legs, &ROWS, &digests());
        let text = describe(&verdict);
        assert!(
            text.contains("INCONCLUSIVE"),
            "a toolchain mismatch must outrank the digest difference: {text}"
        );
        assert!(text.contains("1.98.0"), "{text}");
    }

    /// A leg that ran fewer simulations would otherwise narrow the
    /// comparison to the intersection, silently.
    #[test]
    fn a_leg_that_skipped_a_simulation_is_a_divergence() {
        let mut legs = three("0x1", "0x1", "0x1");
        legs[0]
            .digests
            .insert("frame/schedule".to_string(), "0x2".to_string());
        let verdict = compare(&legs, &ROWS, &digests());
        let text = describe(&verdict);
        assert!(text.contains("DIVERGED"), "{text}");
        assert!(text.contains("frame/schedule"), "{text}");
    }

    /// The other direction of the same check. A *later* leg carrying a
    /// digest the reference lacks is just as much a set difference, and
    /// covering only one direction would let half of it through.
    #[test]
    fn a_leg_that_ran_an_extra_simulation_is_a_divergence_too() {
        let mut legs = three("0x1", "0x1", "0x1");
        legs[2]
            .digests
            .insert("frame/schedule".to_string(), "0x2".to_string());
        let verdict = compare(&legs, &ROWS, &digests());
        let text = describe(&verdict);
        assert!(text.contains("DIVERGED"), "{text}");
        assert!(text.contains("frame/schedule"), "{text}");
    }

    /// The version must be a JSON number. A string `"1"` is a document
    /// written by something that does not share this shape, and reading
    /// it anyway is how a comparison compares the wrong things.
    #[test]
    fn a_schema_version_that_is_not_a_number_is_refused() {
        for bad in [r#""1""#, "null", "true", r#"{"a":1}"#] {
            let text = format!(
                concat!(
                    r#"{{"schema_version": {}, "os": "linux", "arch": "x86_64", "#,
                    r#""toolchain": "rustc 1.97.0", "digests": {{"a": "0x1"}}}}"#
                ),
                bad
            );
            let error = parse_leg("leg.json", &text)
                .expect_err("a non-numeric schema version must be refused");
            assert!(error.contains("schema_version"), "{error}");
        }
    }

    /// Both halves of the claim are supplied or no claim is made. A
    /// caller who forgets either expectation is refused rather than
    /// handed a green — which is what stops the expectation being
    /// droppable at the call site without anything noticing.
    #[test]
    fn an_empty_expectation_refuses_rather_than_passing_vacuously() {
        let legs = three("0x1", "0x1", "0x1");

        let no_rows = compare(&legs, &[], &digests());
        assert!(!no_rows.is_pass(), "{no_rows:?}");
        assert!(
            describe(&no_rows).contains("no expected targets were given"),
            "{no_rows:?}"
        );

        let no_digests = compare(&legs, &ROWS, &[]);
        assert!(!no_digests.is_pass(), "{no_digests:?}");
        assert!(
            describe(&no_digests).contains("no expected digests were given"),
            "{no_digests:?}"
        );
    }

    #[test]
    fn no_legs_at_all_is_inconclusive() {
        let verdict = compare(&[], &ROWS, &digests());
        assert!(!verdict.is_pass());
        assert!(describe(&verdict).contains("nothing to compare"));
    }

    /// Written and read by hand, so the round trip is the only thing
    /// standing between a renderer of one shape and a parser of another.
    #[test]
    fn a_leg_survives_the_round_trip_it_is_written_for() {
        let mut original = leg(
            "linux.json",
            "x86_64",
            &[("glide/seed-7/state", "0xdeadbeefcafef00d"), ("a/b", "0x1")],
        );
        // A toolchain string carrying every character JSON reserves —
        // the two obvious ones and a control character. `toolchain` is
        // whatever `rustc --version` printed, so a compiler wrapper that
        // prefixes a warning line really does put a newline here, and an
        // escaper that handled only quote and backslash rendered an
        // invalid document while the emit exited zero.
        original.toolchain = "warning: shim noise\n\trustc 1.97.0 (path \"C:\\rust\")".to_string();
        let text = render_leg(&original);
        let parsed = parse_leg("linux.json", &text).expect("what we render, we read");
        assert_eq!(parsed, original);
        // And the rendered text really is one JSON document, not merely
        // one this module's own reader tolerates.
        crate::json::parse(&text).expect("a leg is a valid JSON document");
    }

    /// The version gate refuses forward as well as back: a newer emitter
    /// may mean something different by a field this build reads by the
    /// same name, and guessing is how a lane compares the wrong things.
    #[test]
    fn a_document_from_another_schema_version_is_refused_in_both_directions() {
        for other in ["0", "2", "99"] {
            let text = format!(
                "{{\"schema_version\": {other}, \"os\": \"linux\", \"arch\": \"x86_64\",                  \"toolchain\": \"rustc 1.97.0\", \"digests\": {{\"a\": \"0x1\"}}}}"
            );
            let error = parse_leg("leg.json", &text).expect_err("version {other} must be refused");
            assert!(error.contains(other), "{error}");
        }
    }

    /// Every absence is named rather than defaulted. A parser that
    /// defaulted a missing digest set to empty would turn a broken emit
    /// step into a passing comparison.
    #[test]
    fn a_leg_missing_any_field_is_refused_by_name() {
        let full = concat!(
            r#"{"schema_version": 1, "os": "linux", "arch": "x86_64", "#,
            r#""toolchain": "rustc 1.97.0", "digests": {"a": "0x1"}}"#
        );
        assert!(
            parse_leg("leg.json", full).is_ok(),
            "the control must parse"
        );

        for field in ["os", "arch", "toolchain", "digests"] {
            let without = full.replace(&format!("\"{field}\""), "\"removed\"");
            let error = parse_leg("leg.json", &without)
                .expect_err("a leg without `{field}` must be refused");
            assert!(error.contains(field), "{error}");
        }
    }

    /// Digests are strings because a `u64` exceeds what a JSON number
    /// carries exactly, and a reader that rounded one would call two
    /// different states equal — the one failure this lane exists to stop.
    #[test]
    fn a_digest_that_is_not_a_hex_string_is_refused() {
        // `"0xZZZZ"` is the case the name of this test always claimed:
        // a prefix check alone would accept it, and the error message
        // says "hex string", so the tail has to be held to that.
        // A leg that names no compiler is refused for the same reason a
        // digest-less one is: the comparison's whole premise is that one
        // toolchain built every leg, and it cannot say that of a blank.
        for blank in ["", "   "] {
            let text = format!(
                concat!(
                    r#"{{"schema_version": 1, "os": "linux", "arch": "x86_64", "#,
                    r#""toolchain": "{}", "digests": {{"a": "0x1"}}}}"#
                ),
                blank
            );
            let error = parse_leg("leg.json", &text).expect_err("a blank toolchain is refused");
            assert!(error.contains("names no toolchain"), "{error}");
        }
        for bad in ["12345", r#""deadbeef""#, r#""0x""#, r#""0xZZZZ""#, "null"] {
            let text = format!(
                concat!(
                    r#"{{"schema_version": 1, "os": "linux", "arch": "x86_64", "#,
                    r#""toolchain": "rustc 1.97.0", "digests": {{"a": {}}}}}"#
                ),
                bad
            );
            assert!(
                parse_leg("leg.json", &text).is_err(),
                "`{bad}` should not be accepted as a digest"
            );
        }
    }

    #[test]
    fn every_verdict_describes_itself_without_panicking() {
        for verdict in [
            compare(&three("0x1", "0x1", "0x1"), &ROWS, &digests()),
            compare(&three("0x1", "0x1", "0x2"), &ROWS, &digests()),
            compare(&[], &ROWS, &digests()),
        ] {
            let text = describe(&verdict);
            assert!(!text.is_empty());
            // A failure must never read as a success at a glance.
            if !verdict.is_pass() {
                assert!(
                    text.contains("INCONCLUSIVE") || text.contains("DIVERGED"),
                    "{text}"
                );
            }
        }
    }
}

#[cfg(test)]
mod field_selection {
    /// **A report's digest fields are the run's business.** When one sample fed
    /// the lane, the pair was fixed in the parser; a second sample naming its
    /// one hash `digest` would have been told it carried no `state_hash`, which
    /// is true and useless.
    #[test]
    fn a_run_contributes_exactly_the_fields_it_names() {
        let two = r#"{"schema_version":1,"schedule_hash":"0xaa","state_hash":"0xbb"}"#;
        let pairs = super::digests_from_output("g/x", two, &["schedule_hash", "state_hash"])
            .expect("both are there");
        assert_eq!(pairs.len(), 2);

        let one = r#"{"schema_version":1,"sample":"leap","digest":"0xcc"}"#;
        let pairs = super::digests_from_output("l/y", one, &["digest"]).expect("one is there");
        assert_eq!(pairs, vec![("l/y/digest".to_string(), "0xcc".to_string())]);

        // And asking for a field a report does not carry is still a failure,
        // rather than a quietly smaller digest set.
        let missing = super::digests_from_output("l/y", one, &["state_hash"])
            .expect_err("the field is absent");
        assert!(missing.contains("state_hash"), "got: {missing}");

        // Naming no fields at all yields nothing — and nothing is all it
        // yields: the anti-vacuity check in `compare` only fires when a
        // leg is empty *entirely*, so one such row among several would
        // narrow every target identically and be agreed over rather than
        // caught. The pinned table is what forbids that shape, in
        // `every_pinned_run_has_its_own_name_and_no_repeated_field`.
        assert_eq!(
            super::digests_from_output("l/y", one, &[]).expect("nothing asked for"),
            Vec::new()
        );
    }
}
