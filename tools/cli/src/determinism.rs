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
pub const TARGETS: [(&str, &str); 3] = [
    ("linux", "x86_64"),
    ("windows", "x86_64"),
    ("macos", "aarch64"),
];

/// The architectures [`TARGETS`] binds, in the shape [`compare`] wants.
#[must_use]
pub fn expected_arches() -> Vec<&'static str> {
    TARGETS.iter().map(|(_, arch)| *arch).collect()
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
    /// `target_os`, as the emitting build saw itself.
    pub os: String,
    /// `target_arch`, likewise. Compared against the expected set, so a
    /// runner fleet that quietly changes instruction set is caught
    /// rather than passing while proving less.
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
/// `expected_arches` is the set the target list binds — every one must
/// appear exactly once. Passing an empty set is itself a refusal: a
/// comparison with nothing to expect cannot detect a fleet that narrowed.
///
/// The order of checks is the order of blame. Environment problems are
/// reported as `Inconclusive` *before* digests are compared at all,
/// because two legs on different compilers producing different digests
/// is not evidence of anything, and reporting it as divergence would send
/// somebody hunting a bug that is not there.
#[must_use]
pub fn compare(legs: &[Leg], expected_arches: &[&str]) -> Verdict {
    // First, because a comparison of nothing is the one failure that
    // most resembles success. Placing it after the checks below would
    // leave it unreachable — the count check catches an empty slice — and
    // an unreachable refusal is a refusal nobody can test.
    let Some(reference) = legs.first() else {
        return Verdict::Inconclusive(vec![
            "no reports were supplied, so there was nothing to compare".to_string(),
        ]);
    };

    let mut blocked = Vec::new();

    if expected_arches.is_empty() {
        blocked.push(
            "no expected architectures were given, so this comparison could not have \
             detected a target set that narrowed"
                .to_string(),
        );
    }
    if legs.len() < expected_arches.len() {
        blocked.push(format!(
            "expected {} legs, one per target row, and received {} — a missing leg is an \
             untested target, not a passing one",
            expected_arches.len(),
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

    // The architecture set, matched to the rows rather than merely
    // counted: three x86_64 legs satisfy a count of three and prove far
    // less than the target list claims.
    let mut seen: Vec<&str> = legs.iter().map(|leg| leg.arch.as_str()).collect();
    seen.sort_unstable();
    let mut wanted: Vec<&str> = expected_arches.to_vec();
    wanted.sort_unstable();
    if !expected_arches.is_empty() && seen != wanted {
        blocked.push(format!(
            "the legs report architectures [{}] and the target list binds [{}] — the lane \
             is proving a different set than it claims",
            seen.join(", "),
            wanted.join(", ")
        ));
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

    if !blocked.is_empty() {
        return Verdict::Inconclusive(blocked);
    }

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
                divergences.push(format!(
                    "`{name}` is {expected} on {} ({}) and {value} on {} ({}) — the \
                     same inputs produced different state on two targets",
                    reference.origin, reference.arch, leg.origin, leg.arch
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
        // Hex strings, not numbers: a u64 digest exceeds what JSON
        // numbers carry exactly, and a consumer that rounded one would
        // report two different states as the same.
        if !digest.starts_with("0x") || digest.len() < 3 {
            return Err(format!(
                "`{origin}`'s digest `{name}` is `{digest}`, which is not a 0x-prefixed \
                 hex string — digests are strings so that no reader rounds one"
            ));
        }
        digests.insert(name.clone(), digest.to_string());
    }

    Ok(Leg {
        origin: origin.to_string(),
        os: field("os")?,
        arch: field("arch")?,
        toolchain: field("toolchain")?,
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

/// The subset of JSON string escaping these fields can actually need:
/// a toolchain string carries a version and a hash, a digest is hex, a
/// name is chosen here. Backslash and quote are escaped anyway, because
/// "cannot contain" is a claim about today's inputs.
fn escape(text: &str) -> String {
    text.replace('\\', r"\\").replace('"', "\\\"")
}

/// Pull the digests a pinned run reported out of what it printed.
///
/// The pure half of `--emit`: everything between "a child process wrote
/// some bytes" and "the report holds two digests". Separated from the
/// orchestration around it because the orchestration spawns four cargo
/// runs and cannot be reached by a unit test, while every way this can go
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
        digests.push((format!("{name}/{field}"), value.to_string()));
    }
    Ok(digests)
}

#[cfg(test)]
mod tests {
    use super::{Leg, Verdict, compare, describe, parse_leg, render_leg};
    use std::collections::BTreeMap;

    const ARCHES: [&str; 3] = ["aarch64", "x86_64", "x86_64"];

    fn leg(origin: &str, arch: &str, pairs: &[(&str, &str)]) -> Leg {
        Leg {
            origin: origin.to_string(),
            os: "linux".to_string(),
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

    /// The bound set is a list rather than a number, and this is what
    /// says so: a row with no leg is a target promised and never proved.
    #[test]
    fn the_expected_architectures_come_from_the_target_rows() {
        let arches = super::expected_arches();
        assert_eq!(arches.len(), super::TARGETS.len());
        for (platform, arch) in super::TARGETS {
            assert!(
                arches.contains(&arch),
                "{platform} binds {arch} and the expected set omits it"
            );
        }
        // Not a set: two rows may share an instruction set, and
        // collapsing them would let two legs satisfy three rows.
        assert!(arches.len() > 1);
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
    /// logged after would too.
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

    #[test]
    fn three_agreeing_targets_pass() {
        let verdict = compare(&three("0x1", "0x1", "0x1"), &ARCHES);
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
        let verdict = compare(&three("0x1", "0x1", "0xdead"), &ARCHES);
        assert!(!verdict.is_pass());
        // Read through `describe`, which is what a person staring at a red
        // lane actually sees. Destructuring would test the shape and leave
        // the rendering — the part with a reader — untested.
        let text = describe(&verdict);
        assert!(text.contains("DIVERGED"), "{text}");
        assert!(text.contains("macos.json"), "{text}");
        assert!(text.contains("0xdead"), "{text}");
        assert!(text.contains("aarch64"), "{text}");
    }

    /// The check this lane's whole credibility rests on. A comparison of
    /// nothing is equality, and equality is what this lane reports as
    /// success — so a leg that ran no simulations must never reach the
    /// comparison at all.
    #[test]
    fn a_leg_with_no_digests_is_inconclusive_rather_than_equal() {
        let mut legs = three("0x1", "0x1", "0x1");
        legs[1].digests = BTreeMap::new();
        let verdict = compare(&legs, &ARCHES);
        assert!(!verdict.is_pass());
        let text = describe(&verdict);
        assert!(text.contains("INCONCLUSIVE"), "{text}");
        assert!(text.contains("windows.json"), "{text}");
    }

    #[test]
    fn a_missing_leg_is_inconclusive_rather_than_two_legs_agreeing() {
        let mut legs = three("0x1", "0x1", "0x1");
        legs.pop();
        let verdict = compare(&legs, &ARCHES);
        assert!(!verdict.is_pass());
        assert!(matches!(verdict, Verdict::Inconclusive(_)));
    }

    /// Three legs on one instruction set satisfy a count of three and
    /// prove strictly less than the target list claims. Counting is not
    /// enough; the set has to match row for row.
    #[test]
    fn the_wrong_architecture_set_is_inconclusive_even_at_the_right_count() {
        let mut legs = three("0x1", "0x1", "0x1");
        legs[2].arch = "x86_64".to_string();
        let verdict = compare(&legs, &ARCHES);
        assert!(!verdict.is_pass());
        let text = describe(&verdict);
        assert!(text.contains("INCONCLUSIVE"), "{text}");
        assert!(text.contains("aarch64"), "{text}");
    }

    /// Two compilers producing two digests is not evidence of a
    /// portability bug, and reporting it as one sends somebody hunting
    /// something that is not there.
    #[test]
    fn a_toolchain_mismatch_is_inconclusive_and_not_divergence() {
        let mut legs = three("0x1", "0x1", "0xdead");
        legs[2].toolchain = "rustc 1.98.0".to_string();
        let verdict = compare(&legs, &ARCHES);
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
        let verdict = compare(&legs, &ARCHES);
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
        let verdict = compare(&legs, &ARCHES);
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

    #[test]
    fn an_empty_expectation_refuses_rather_than_passing_vacuously() {
        let verdict = compare(&three("0x1", "0x1", "0x1"), &[]);
        assert!(!verdict.is_pass(), "{verdict:?}");
    }

    #[test]
    fn no_legs_at_all_is_inconclusive() {
        let verdict = compare(&[], &ARCHES);
        assert!(!verdict.is_pass());
        assert!(describe(&verdict).contains("nothing to compare"));
    }

    /// Written and read by hand, so the round trip is the only thing
    /// standing between a renderer of one shape and a parser of another.
    #[test]
    fn a_leg_survives_the_round_trip_it_is_written_for() {
        let original = leg(
            "linux.json",
            "x86_64",
            &[("glide/seed-7/state", "0xdeadbeefcafef00d"), ("a/b", "0x1")],
        );
        let text = render_leg(&original);
        let parsed = parse_leg("linux.json", &text).expect("what we render, we read");
        assert_eq!(parsed, original);
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
        for bad in ["12345", r#""deadbeef""#, r#""0x""#, "null"] {
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
            compare(&three("0x1", "0x1", "0x1"), &ARCHES),
            compare(&three("0x1", "0x1", "0x2"), &ARCHES),
            compare(&[], &ARCHES),
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

        // Naming no fields at all yields nothing, which the comparison treats
        // as a leg that reported nothing rather than as agreement.
        assert_eq!(
            super::digests_from_output("l/y", one, &[]).expect("nothing asked for"),
            Vec::new()
        );
    }
}
