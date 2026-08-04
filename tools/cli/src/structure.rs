//! Workspace structure rules, evaluated over `cargo metadata` output.
//!
//! Every workspace crate carries a `[package.metadata.renew]` table; these
//! rules hold the tables and the dependency graph to the workspace's
//! structural commitments: valid closed-schema metadata, an acyclic
//! dependency graph, agreement between `core` flags and the core list,
//! core crates depending only on core crates, maturity flowing in one
//! direction, and non-engine crates staying in their lane.

use crate::json::Value;

/// The core crate list the `core` flags must agree with. Crates listed
/// here but not yet in the workspace are fine (the list leads the code);
/// a crate that exists must match.
pub const CORE_CRATES: &[&str] = &[
    "renew-diag",
    "renew-event",
    "renew-platform",
    "renew-memory",
    "renew-math",
];

/// The crate holding the OS capabilities a deterministic crate must not
/// reach: a wall clock, the filesystem, thread spawning. Rule 8 is about
/// reachability to this crate by name, so a rename would silently disable
/// it — which is why the workspace check below refuses to pass when a
/// crate claims determinism and this name is absent.
const PLATFORM_CRATE: &str = "renew-platform";

const MATURITIES: &[&str] = &["bootstrap", "internal", "stable"];
const REQUIRED_FIELDS: &[&str] = &[
    "purpose",
    "maturity",
    "core",
    "extension_points",
    "simulation",
];

/// One crate as the rules see it.
pub struct CrateShape {
    pub name: String,
    /// The directory holding this crate's `Cargo.toml`, forward-slashed.
    pub dir: String,
    /// Under the engine module roots (`crates/`)?
    pub engine: bool,
    /// Workspace-internal dependencies, any kind, dev included.
    pub deps: Vec<String>,
    /// Workspace dependencies that ship: `deps` without the dev edges.
    ///
    /// The float-closure rule walks these rather than `deps`, because a
    /// test framework is linked into test binaries and into nothing a
    /// simulation ships — `proptest` computes floats and cannot put one
    /// anywhere a digest can see it. The OS-capability rule keeps using
    /// `deps`, dev edges included, because a test that opens a clock
    /// through a wrapper is exactly the laundering it exists to catch.
    pub runtime_deps: Vec<String>,
    /// Shipping dependencies this checker cannot see inside: everything
    /// in `runtime_deps`' position that is not a workspace member. The
    /// closure rules are graph walks over workspace crates, so a foreign
    /// edge is not a crate they clear — it is a crate they cannot reach,
    /// which is a different thing and has to be reported as one.
    pub foreign_deps: Vec<String>,
    /// Does this crate's root deny `clippy::float_arithmetic`?
    ///
    /// Read from source at scan time rather than checked here, so the
    /// rules stay pure functions of the shapes and their tests can
    /// construct a crate that does or does not carry it.
    pub denies_float: bool,
    /// Parsed metadata table, or the list of schema problems.
    pub meta: Result<Meta, Vec<String>>,
}

/// The validated fields the rules consume.
#[derive(Debug)]
pub struct Meta {
    pub maturity: String,
    pub core: bool,
    pub simulation: bool,
}

/// One rule violation.
#[derive(Debug, PartialEq, Eq)]
pub struct Finding {
    pub rule: &'static str,
    pub message: String,
}

/// Build crate shapes from a parsed `cargo metadata --no-deps` document.
///
/// # Errors
///
/// Returns a message when the document does not look like `cargo metadata`
/// output at all — a check that cannot read its input must fail loudly,
/// never pass vacuously.
pub fn shapes_from_metadata(doc: &Value) -> Result<Vec<CrateShape>, String> {
    let packages = doc
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| "no `packages` array in cargo metadata output".to_string())?;
    // Engine-ness is judged relative to the workspace root, never by a
    // substring of the absolute checkout path (a checkout under a
    // directory named `crates` must not reclassify every crate).
    let workspace_root = doc
        .get("workspace_root")
        .and_then(Value::as_str)
        .ok_or_else(|| "no `workspace_root` in cargo metadata output".to_string())?
        .replace('\\', "/");
    let engine_root = format!("{}/crates/", workspace_root.trim_end_matches('/'));

    let names: Vec<String> = packages
        .iter()
        .filter_map(|package| package.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    if names.len() != packages.len() {
        return Err("a package without a name in cargo metadata output".to_string());
    }

    let mut shapes = Vec::new();
    for package in packages {
        let name = package
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let manifest_path = package
            .get("manifest_path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .replace('\\', "/");
        let engine = manifest_path.starts_with(&engine_root);
        let dir = manifest_path
            .rsplit_once('/')
            .map_or(String::new(), |(parent, _)| parent.to_string());
        let empty: Vec<Value> = Vec::new();
        let declared = package
            .get("dependencies")
            .and_then(Value::as_array)
            .unwrap_or(&empty);
        let dep_name = |dep: &Value| dep.get("name").and_then(Value::as_str).map(str::to_string);
        // `kind` is absent (null) for a normal dependency and carries
        // "dev" or "build" otherwise, so absence is what ships.
        let ships = |dep: &Value| {
            !matches!(
                dep.get("kind").and_then(Value::as_str),
                Some("dev" | "development")
            )
        };
        let known = |name: &String| names.iter().any(|candidate| candidate == name);
        let deps: Vec<String> = declared.iter().filter_map(dep_name).filter(known).collect();
        let runtime_deps: Vec<String> = declared
            .iter()
            .filter(|dep| ships(dep))
            .filter_map(dep_name)
            .filter(known)
            .collect();
        let mut foreign_deps: Vec<String> = declared
            .iter()
            .filter(|dep| ships(dep))
            .filter_map(dep_name)
            .filter(|candidate| !known(candidate))
            .collect();
        foreign_deps.sort_unstable();
        foreign_deps.dedup();
        // A crate root that is not there reads as "does not deny", which
        // is the safe direction: the rule reports rather than assumes.
        let denies_float = std::fs::read_to_string(format!("{dir}/src/lib.rs"))
            .is_ok_and(|source| source.contains("clippy::float_arithmetic"));
        let meta = validate_meta(package);
        shapes.push(CrateShape {
            name,
            dir,
            engine,
            deps,
            runtime_deps,
            foreign_deps,
            denies_float,
            meta,
        });
    }
    Ok(shapes)
}

fn validate_meta(package: &Value) -> Result<Meta, Vec<String>> {
    let renew = package
        .get("metadata")
        .and_then(|metadata| metadata.get("renew"));
    let table = match renew.map(Value::as_object) {
        Some(Some(table)) => table,
        Some(None) => {
            return Err(vec![
                "`[package.metadata.renew]` is not a table".to_string(),
            ]);
        }
        None => {
            return Err(vec!["missing `[package.metadata.renew]` table".to_string()]);
        }
    };

    let mut problems = Vec::new();
    let field = |key: &str| {
        table
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    };

    for required in REQUIRED_FIELDS {
        if field(required).is_none() {
            problems.push(format!("missing field `{required}`"));
        }
    }
    for (key, _) in table {
        if !REQUIRED_FIELDS.contains(&key.as_str()) {
            problems.push(format!("unknown field `{key}` (the schema is closed)"));
        }
    }

    if let Some(purpose) = field("purpose") {
        match purpose.as_str() {
            Some(text) if !text.trim().is_empty() => {}
            Some(_) => problems.push("`purpose` is empty".to_string()),
            None => problems.push("`purpose` is not a string".to_string()),
        }
    }
    let maturity = match field("maturity").map(|value| (value, value.as_str())) {
        Some((_, Some(text))) if MATURITIES.contains(&text) => Some(text.to_string()),
        Some((_, Some(text))) => {
            problems.push(format!("`maturity` is `{text}`, not one of {MATURITIES:?}"));
            None
        }
        Some((_, None)) => {
            problems.push("`maturity` is not a string".to_string());
            None
        }
        None => None,
    };
    let core = match field("core").map(Value::as_bool) {
        Some(Some(flag)) => Some(flag),
        Some(None) => {
            problems.push("`core` is not a boolean".to_string());
            None
        }
        None => None,
    };
    let simulation = match field("simulation").map(Value::as_bool) {
        Some(Some(flag)) => Some(flag),
        Some(None) => {
            problems.push("`simulation` is not a boolean".to_string());
            None
        }
        None => None,
    };
    if let Some(points) = field("extension_points") {
        match points.as_array() {
            Some(items) if items.iter().all(|item| item.as_str().is_some()) => {}
            Some(_) => problems.push("`extension_points` items must be strings".to_string()),
            None => problems.push("`extension_points` is not an array".to_string()),
        }
    }

    match (problems.is_empty(), maturity, core, simulation) {
        (true, Some(maturity), Some(core), Some(simulation)) => Ok(Meta {
            maturity,
            core,
            simulation,
        }),
        _ => Err(problems),
    }
}

/// Evaluate every rule; an empty result is a passing workspace.
#[must_use]
pub fn evaluate(shapes: &[CrateShape]) -> Vec<Finding> {
    let mut findings = Vec::new();

    // Rule 1 — schema.
    for shape in shapes {
        if let Err(problems) = &shape.meta {
            for problem in problems {
                findings.push(Finding {
                    rule: "schema",
                    message: format!("{}: {problem}", shape.name),
                });
            }
        }
    }

    // Rule 2 — dependency cycles (any kind, dev included).
    findings.extend(find_cycles(shapes));

    for shape in shapes {
        if let Ok(meta) = &shape.meta {
            flag_rules(shape, meta, shapes, &mut findings);
            edge_rules(shape, meta, shapes, &mut findings);
        }
    }

    // The guard on Rule 8's guard. The rule names one crate as a string,
    // so renaming that crate would turn the check into a walk that can
    // never match and report green forever. Demanded only when some crate
    // actually claims determinism: a workspace with no such claim has
    // nothing for the rule to protect, and synthetic ones are common.
    if shapes
        .iter()
        .any(|shape| shape.meta.as_ref().is_ok_and(|meta| meta.simulation))
        && !shapes.iter().any(|shape| shape.name == PLATFORM_CRATE)
    {
        findings.push(Finding {
            rule: "simulation-closure",
            message: format!(
                "a crate declares simulation = true but no crate is named {PLATFORM_CRATE} — the closure rule names it literally, so a rename disables the rule instead of failing it"
            ),
        });
    }

    findings
}

/// Rule 7: every engine crate carries a crate-local `clippy.toml`.
///
/// The zoning policy each engine crate enforces lives in that file —
/// tripwires that reject a clock, a filesystem call, or a raw thread
/// spawn from a crate whose contract forbids them. Nine crates have added
/// it by hand, correctly, every time. That is exactly when the habit
/// should stop being a habit: the crate that forgets will not fail to
/// compile, will not fail a test, and will simply have no tripwires,
/// which is indistinguishable from having them and passing.
///
/// The predicate is injected rather than called directly so the rule is
/// exercisable without a filesystem — the same reason the rest of this
/// module takes parsed metadata rather than running cargo itself.
pub fn lint_file_findings(shapes: &[CrateShape], exists: &dyn Fn(&str) -> bool) -> Vec<Finding> {
    let mut findings = Vec::new();
    for shape in shapes {
        if !shape.engine {
            continue;
        }
        let path = format!("{}/clippy.toml", shape.dir);
        if !exists(&path) {
            findings.push(Finding {
                rule: "lint-files",
                message: format!(
                    "engine crate {} has no clippy.toml, so its zoning tripwires are enforced by review rather than by the compiler",
                    shape.name
                ),
            });
        }
    }
    findings
}

fn meta_of<'a>(shapes: &'a [CrateShape], name: &str) -> Option<(&'a CrateShape, &'a Meta)> {
    shapes
        .iter()
        .find(|shape| shape.name == name)
        .and_then(|shape| shape.meta.as_ref().ok().map(|meta| (shape, meta)))
}

/// Rules 3 and 6's flag half: core agreement and non-engine flag lanes.
fn flag_rules(
    shape: &CrateShape,
    meta: &Meta,
    _shapes: &[CrateShape],
    findings: &mut Vec<Finding>,
) {
    {
        // Rule 3 — core agreement (one-sided: listed-but-absent is fine).
        let listed = CORE_CRATES.contains(&shape.name.as_str());
        if shape.engine && meta.core && !listed {
            findings.push(Finding {
                rule: "core-agreement",
                message: format!(
                    "{} declares core = true but is not in the core list",
                    shape.name
                ),
            });
        }
        if listed && shape.engine && !meta.core {
            findings.push(Finding {
                rule: "core-agreement",
                message: format!(
                    "{} is in the core list but declares core = false",
                    shape.name
                ),
            });
        }
        if listed && !shape.engine {
            findings.push(Finding {
                rule: "core-agreement",
                message: format!(
                    "{} is in the core list but is not an engine crate",
                    shape.name
                ),
            });
        }

        // Rule 6 — non-engine constraints.
        if !shape.engine && meta.core {
            findings.push(Finding {
                rule: "non-engine",
                message: format!(
                    "{} is outside crates/ and must declare core = false",
                    shape.name
                ),
            });
        }
        // `simulation` is deliberately absent here. The rule used to
        // forbid both flags outside crates/, which conflated "is this
        // engine core" with "is this deterministic fixed-step code" —
        // unrelated claims. A sample's game world is the second without
        // being the first, and while the flags travelled together no
        // sample's determinism could be machine-checked. Declaring the
        // flag only ever adds obligations, so nothing can use it to
        // escape a rule.
    }
}

/// Rules 4, 5, and 6's edge half: closure, maturity ordering, and
/// engine→non-engine dependencies.
fn edge_rules(shape: &CrateShape, meta: &Meta, shapes: &[CrateShape], findings: &mut Vec<Finding>) {
    for dep in &shape.deps {
        let dep_engine = shapes
            .iter()
            .find(|candidate| &candidate.name == dep)
            .is_some_and(|candidate| candidate.engine);
        if shape.engine && !dep_engine {
            findings.push(Finding {
                rule: "non-engine",
                message: format!(
                    "engine crate {} depends on non-engine crate {dep}",
                    shape.name
                ),
            });
        }

        // Rule 5 — maturity ordering on engine→engine edges.
        if shape.engine
            && let Some((dep_shape, dep_meta)) = meta_of(shapes, dep)
            && dep_shape.engine
            && maturity_rank(&meta.maturity) > maturity_rank(&dep_meta.maturity)
        {
            findings.push(Finding {
                rule: "maturity-order",
                message: format!(
                    "{} ({}) depends on {} ({}) — maturity must not decrease along dependencies",
                    shape.name, meta.maturity, dep_shape.name, dep_meta.maturity
                ),
            });
        }
    }

    // Rule 4 — core closure: a core engine crate reaches only core
    // engine crates.
    if shape.engine && meta.core {
        let mut stack: Vec<&str> = shape.deps.iter().map(String::as_str).collect();
        let mut visited: Vec<&str> = Vec::new();
        while let Some(current) = stack.pop() {
            if visited.contains(&current) {
                continue;
            }
            visited.push(current);
            if let Some((dep_shape, dep_meta)) = meta_of(shapes, current) {
                if dep_shape.engine && !dep_meta.core {
                    findings.push(Finding {
                        rule: "core-closure",
                        message: format!(
                            "core crate {} depends (transitively) on optional engine crate {current}",
                            shape.name
                        ),
                    });
                }
                stack.extend(dep_shape.deps.iter().map(String::as_str));
            }
        }
    }

    // Rule 8 — simulation closure: a crate promising determinism cannot
    // reach the crate holding OS capabilities, at any depth.
    //
    // The lint set that enforces determinism matches a *definition path*
    // in the crate being compiled, so it is exactly one wrapper deep: a
    // first-party type re-exported from an intermediate crate defeats
    // every ban while compiling clean. That is a graph property, not a
    // source-text property, which is why it is checked here.
    //
    // Resolved through `shapes` rather than `meta_of` on purpose — a
    // dependency whose own metadata failed to parse must not truncate the
    // walk and hide a path. Its schema failure is already its own finding.
    if meta.simulation {
        float_closure_rules(shape, shapes, findings);
        let mut stack: Vec<&str> = shape.deps.iter().map(String::as_str).collect();
        let mut visited: Vec<&str> = Vec::new();
        while let Some(current) = stack.pop() {
            if visited.contains(&current) {
                continue;
            }
            visited.push(current);
            if current == PLATFORM_CRATE {
                findings.push(Finding {
                    rule: "simulation-closure",
                    message: format!(
                        "{} declares simulation = true and reaches {PLATFORM_CRATE} (transitively) — a wrapper cannot launder an OS capability",
                        shape.name
                    ),
                });
                continue;
            }
            if let Some(dep_shape) = shapes.iter().find(|candidate| candidate.name == current) {
                stack.extend(dep_shape.deps.iter().map(String::as_str));
            }
        }
    }
}

/// Rule 9 — float closure.
///
/// Determinism is a property of arithmetic, and arithmetic one edge away
/// is no less able to reach digested state than arithmetic in the crate
/// itself. So the deny that keeps floats out of a simulation has to hold
/// across the simulation's whole shipping closure, or it only holds where
/// somebody remembered to write it.
///
/// **Stated over the closure rather than as "may not reach a crate that
/// computes floats"**, which was the first formulation and is both
/// undecidable and false. Undecidable because nothing declares whether a
/// crate computes floats — no manifest field says it and no walk can
/// infer it. False because the game's world reaches the frame loop, which
/// computes the one interpolation factor named as an exemption, so the
/// rule would have reddened the tree the moment it landed. The closure
/// form asks instead a question every crate can answer about itself, and
/// leaves named exemptions where they belong: at the expression, inside a
/// crate that denies.
///
/// Two halves, and the second is the one that keeps the first honest:
/// every workspace crate in the closure denies, **and** the closure holds
/// no crate this checker cannot open. A dependency outside the workspace
/// is not a crate the rule cleared; it is a crate it could not read, and
/// reporting that as absence is the green-gate-measuring-nothing failure
/// this tree has already paid for. There are none today, which is what
/// makes saying so cheap.
///
/// Walks shipping edges — `runtime_deps`, not `deps`. A test framework is
/// linked into test binaries and into nothing a simulation ships:
/// `proptest` computes floats and cannot put one where a digest sees it.
/// The OS-capability rule above keeps its dev edges, because a test
/// reaching a clock through a wrapper is the laundering it exists to
/// catch.
fn float_closure_rules(shape: &CrateShape, shapes: &[CrateShape], findings: &mut Vec<Finding>) {
    let unreadable = |owner: &str, through: &str, foreign: &str| Finding {
        rule: "float-closure",
        message: if owner == through {
            format!(
                "{owner} declares simulation = true and ships a dependency on {foreign}, which is not a workspace crate — this rule cannot read its source, so its arithmetic is unchecked"
            )
        } else {
            format!(
                "{owner}'s shipping closure reaches {through}, which depends on {foreign} outside the workspace — this rule cannot read its source, so its arithmetic is unchecked"
            )
        },
    };

    for foreign in &shape.foreign_deps {
        findings.push(unreadable(&shape.name, &shape.name, foreign));
    }

    let mut stack: Vec<&str> = shape.runtime_deps.iter().map(String::as_str).collect();
    let mut seen: Vec<&str> = Vec::new();
    while let Some(current) = stack.pop() {
        if seen.contains(&current) {
            continue;
        }
        seen.push(current);
        let Some(dep) = shapes.iter().find(|candidate| candidate.name == current) else {
            continue;
        };
        if !dep.denies_float {
            findings.push(Finding {
                rule: "float-closure",
                message: format!(
                    "{} declares simulation = true and reaches {current} (transitively), which does not deny clippy::float_arithmetic — a float computed there is a float the simulation can digest",
                    shape.name
                ),
            });
        }
        for foreign in &dep.foreign_deps {
            findings.push(unreadable(&shape.name, current, foreign));
        }
        stack.extend(dep.runtime_deps.iter().map(String::as_str));
    }
}

fn maturity_rank(maturity: &str) -> u8 {
    match maturity {
        "stable" => 2,
        "internal" => 1,
        _ => 0,
    }
}

fn find_cycles(shapes: &[CrateShape]) -> Vec<Finding> {
    // Iterative depth-first search, three states per crate.
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        New,
        Open,
        Done,
    }
    let mut states: Vec<State> = vec![State::New; shapes.len()];
    // Resolve a name to its index *and* the crate itself, so the walk
    // carries what it is visiting and never re-indexes `shapes`.
    let lookup = |name: &str| {
        shapes
            .iter()
            .enumerate()
            .find(|(_, shape)| shape.name == name)
    };
    let mut findings = Vec::new();

    for (start, start_shape) in shapes.iter().enumerate() {
        if states.get(start) != Some(&State::New) {
            continue;
        }
        // (crate index, the crate, next dependency position) — an explicit
        // stack so the path is available for the report.
        let mut stack: Vec<(usize, &CrateShape, usize)> = vec![(start, start_shape, 0)];
        if let Some(state) = states.get_mut(start) {
            *state = State::Open;
        }
        while let Some((current, shape, position)) = stack.last().copied() {
            if let Some(dep_name) = shape.deps.get(position) {
                if let Some(last) = stack.last_mut() {
                    last.2 += 1;
                }
                let Some((dep_index, dep_shape)) = lookup(dep_name) else {
                    continue;
                };
                match states.get(dep_index).copied() {
                    Some(State::New) => {
                        if let Some(state) = states.get_mut(dep_index) {
                            *state = State::Open;
                        }
                        stack.push((dep_index, dep_shape, 0));
                    }
                    Some(State::Open) => {
                        let mut path: Vec<&str> = stack
                            .iter()
                            .skip_while(|(index, _, _)| *index != dep_index)
                            .map(|(_, shape, _)| shape.name.as_str())
                            .collect();
                        path.push(dep_name);
                        findings.push(Finding {
                            rule: "dependency-cycle",
                            message: path.join(" -> "),
                        });
                    }
                    _ => {}
                }
            } else {
                if let Some(state) = states.get_mut(current) {
                    *state = State::Done;
                }
                stack.pop();
            }
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(name: &str, engine: bool, deps: &[&str], maturity: &str, core: bool) -> CrateShape {
        CrateShape {
            name: name.to_string(),
            dir: format!("/w/crates/{name}"),
            engine,
            deps: deps.iter().map(ToString::to_string).collect(),
            runtime_deps: deps.iter().map(ToString::to_string).collect(),
            foreign_deps: Vec::new(),
            denies_float: true,
            meta: Ok(Meta {
                maturity: maturity.to_string(),
                core,
                simulation: false,
            }),
        }
    }

    #[test]
    fn an_engine_crate_without_a_clippy_file_is_a_finding() {
        let shapes = [shape("renew-diag", true, &[], "bootstrap", true)];
        let findings = lint_file_findings(&shapes, &|_| false);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].rule, "lint-files");
        assert!(findings[0].message.contains("renew-diag"), "{findings:?}");
    }

    #[test]
    fn an_engine_crate_with_a_clippy_file_is_not() {
        let shapes = [shape("renew-diag", true, &[], "bootstrap", true)];
        assert!(lint_file_findings(&shapes, &|_| true).is_empty());
    }

    /// The rule binds engine crates only. Samples, tools and benchmarks
    /// are outside the module roots and legitimately carry no zoning of
    /// their own, so demanding the file there would train people to add
    /// empty ones.
    #[test]
    fn a_non_engine_crate_without_a_clippy_file_is_ignored() {
        let shapes = [shape("renew-cli", false, &[], "bootstrap", false)];
        assert!(lint_file_findings(&shapes, &|_| false).is_empty());
    }

    /// The predicate is asked about the crate's own directory, not about
    /// some path that merely ends in the right name — a rule that looked
    /// anywhere else would pass on a stray file at the workspace root.
    #[test]
    fn the_rule_looks_beside_the_crates_own_manifest() {
        let shapes = [shape("renew-math", true, &[], "internal", true)];
        let asked = std::cell::RefCell::new(Vec::new());
        let findings = lint_file_findings(&shapes, &|path| {
            asked.borrow_mut().push(path.to_string());
            false
        });
        assert_eq!(asked.into_inner(), ["/w/crates/renew-math/clippy.toml"]);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn every_engine_crate_missing_the_file_is_named_separately() {
        let shapes = [
            shape("renew-diag", true, &[], "bootstrap", true),
            shape("renew-math", true, &[], "bootstrap", true),
            shape("renew-cli", false, &[], "bootstrap", false),
        ];
        let findings = lint_file_findings(&shapes, &|_| false);
        assert_eq!(findings.len(), 2, "{findings:?}");
    }

    #[test]
    fn a_healthy_workspace_produces_no_findings() {
        let shapes = [
            shape("renew-diag", true, &[], "bootstrap", true),
            shape("renew-cli", false, &[], "bootstrap", false),
            shape("hello-engine", false, &[], "bootstrap", false),
        ];
        assert_eq!(evaluate(&shapes), Vec::new());
    }

    #[test]
    fn schema_problems_surface_per_crate() {
        let shapes = [CrateShape {
            name: "broken".to_string(),
            dir: "/w/crates/x".to_string(),
            engine: false,
            deps: Vec::new(),
            runtime_deps: Vec::new(),
            foreign_deps: Vec::new(),
            denies_float: true,
            meta: Err(vec!["missing field `core`".to_string()]),
        }];
        let findings = evaluate(&shapes);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, "schema");
        assert!(findings[0].message.contains("broken"));
    }

    #[test]
    fn dependency_cycles_are_reported_with_their_path() {
        let shapes = [
            shape("a", true, &["b"], "bootstrap", false),
            shape("b", true, &["c"], "bootstrap", false),
            shape("c", true, &["a"], "bootstrap", false),
        ];
        let findings = evaluate(&shapes);
        let cycle = findings
            .iter()
            .find(|finding| finding.rule == "dependency-cycle")
            .expect("cycle reported");
        assert!(
            cycle.message.contains("a -> b -> c -> a"),
            "{}",
            cycle.message
        );
    }

    #[test]
    fn core_agreement_fails_in_both_directions_but_not_for_absent_crates() {
        // renew-platform is listed but absent: no finding (one-sided).
        let unlisted_core = [shape("renew-rogue", true, &[], "bootstrap", true)];
        let findings = evaluate(&unlisted_core);
        assert!(findings.iter().any(|f| f.rule == "core-agreement"));

        let listed_not_core = [shape("renew-diag", true, &[], "bootstrap", false)];
        let findings = evaluate(&listed_not_core);
        assert!(findings.iter().any(|f| f.rule == "core-agreement"));

        let healthy = [shape("renew-diag", true, &[], "bootstrap", true)];
        assert_eq!(evaluate(&healthy), Vec::new());
    }

    #[test]
    fn core_closure_catches_transitive_optional_dependencies() {
        let shapes = [
            shape("renew-diag", true, &["renew-mid"], "bootstrap", true),
            shape("renew-mid", true, &["renew-leaf"], "bootstrap", true),
            shape("renew-leaf", true, &[], "bootstrap", false),
        ];
        let findings = evaluate(&shapes);
        let closure = findings
            .iter()
            .find(|finding| finding.rule == "core-closure")
            .expect("closure violation reported");
        assert!(closure.message.contains("renew-leaf"));
        // renew-mid and renew-leaf also trip core-agreement (not listed);
        // that is correct and separate.
    }

    #[test]
    fn maturity_must_not_decrease_along_engine_edges() {
        let shapes = [
            shape("renew-diag", true, &["renew-young"], "stable", true),
            shape("renew-young", true, &[], "bootstrap", false),
        ];
        let findings = evaluate(&shapes);
        assert!(findings.iter().any(|f| f.rule == "maturity-order"));

        let fine = [
            shape("renew-diag", true, &["renew-deep"], "bootstrap", true),
            shape("renew-deep", true, &[], "stable", true),
        ];
        assert!(!evaluate(&fine).iter().any(|f| f.rule == "maturity-order"));
    }

    #[test]
    fn non_engine_crates_stay_in_their_lane() {
        let flagged = [CrateShape {
            name: "tool".to_string(),
            dir: "/w/crates/x".to_string(),
            engine: false,
            deps: Vec::new(),
            runtime_deps: Vec::new(),
            foreign_deps: Vec::new(),
            denies_float: true,
            meta: Ok(Meta {
                maturity: "bootstrap".to_string(),
                core: true,
                simulation: true,
            }),
        }];
        let findings = evaluate(&flagged);
        // One, not two: `core = true` outside crates/ is still wrong, and
        // `simulation = true` outside crates/ is now allowed. The two used
        // to be forbidden together, which is what stopped a sample's
        // fixed-step world from ever declaring what it is.
        assert_eq!(
            findings.iter().filter(|f| f.rule == "non-engine").count(),
            1
        );
        assert!(
            findings
                .iter()
                .all(|f| !f.message.contains("must declare simulation = false")),
            "location must no longer constrain the determinism flag: {findings:?}"
        );

        let backwards = [
            shape("renew-diag", true, &["tool"], "bootstrap", true),
            shape("tool", false, &[], "bootstrap", false),
        ];
        let findings = evaluate(&backwards);
        assert!(
            findings
                .iter()
                .any(|f| f.rule == "non-engine" && f.message.contains("depends on non-engine"))
        );
    }

    /// A crate declaring determinism, with `deps` and nothing else set.
    fn sim(name: &str, deps: &[&str], simulation: bool) -> CrateShape {
        CrateShape {
            name: name.to_string(),
            dir: format!("/w/crates/{name}"),
            engine: true,
            deps: deps.iter().map(|d| (*d).to_string()).collect(),
            runtime_deps: deps.iter().map(|d| (*d).to_string()).collect(),
            foreign_deps: Vec::new(),
            // Denying by default keeps every existing test a statement
            // about the rule it was written for; the float tests below
            // set it false deliberately.
            denies_float: true,
            meta: Ok(Meta {
                maturity: "bootstrap".to_string(),
                core: false,
                simulation,
            }),
        }
    }

    /// `sim`, with the two knobs the float-closure rule reads.
    fn float_sim(
        name: &str,
        deps: &[&str],
        simulation: bool,
        denies_float: bool,
        foreign: &[&str],
    ) -> CrateShape {
        CrateShape {
            denies_float,
            foreign_deps: foreign.iter().map(|d| (*d).to_string()).collect(),
            ..sim(name, deps, simulation)
        }
    }

    /// A dev edge is in `deps` and not in `runtime_deps` — the shape a
    /// test-only dependency has.
    fn dev_only(name: &str, dev: &[&str], simulation: bool) -> CrateShape {
        CrateShape {
            deps: dev.iter().map(|d| (*d).to_string()).collect(),
            runtime_deps: Vec::new(),
            ..sim(name, &[], simulation)
        }
    }

    #[test]
    fn float_closure_reaches_through_an_intermediate_crate() {
        // The rule's whole reason for existing: the deny is a crate-root
        // attribute, so it says nothing about the crate one edge away,
        // and a float computed there reaches digested state just as well.
        let shapes = [
            float_sim("world", &["middle"], true, true, &[]),
            float_sim("middle", &["maths"], false, true, &[]),
            float_sim("maths", &[], false, false, &[]),
        ];
        let findings = evaluate(&shapes);
        let found = findings
            .iter()
            .find(|finding| finding.rule == "float-closure")
            .expect("the transitive float crate is reported");
        assert!(found.message.contains("maths"), "{}", found.message);

        // The same graph with the far crate denying is silent.
        let clean = [
            float_sim("world", &["middle"], true, true, &[]),
            float_sim("middle", &["maths"], false, true, &[]),
            float_sim("maths", &[], false, true, &[]),
        ];
        assert!(
            !evaluate(&clean)
                .iter()
                .any(|finding| finding.rule == "float-closure"),
            "every crate in the closure denies, so the rule has nothing to say"
        );
    }

    #[test]
    fn float_closure_ignores_a_dev_only_edge() {
        // `proptest` computes floats and is linked into test binaries
        // only. Reporting it would train a reader to ignore the rule, and
        // the rule has to be readable on the day it is right.
        let shapes = [
            dev_only("world", &["harness"], true),
            float_sim("harness", &[], false, false, &["proptest"]),
        ];
        assert!(
            !evaluate(&shapes)
                .iter()
                .any(|finding| finding.rule == "float-closure"),
            "a dev-only edge must not reach the float-closure rule"
        );
    }

    #[test]
    fn float_closure_reports_what_it_cannot_read() {
        // A crate outside the workspace is not a crate this rule cleared;
        // it is one it could not open. Reporting absence as a pass is the
        // green-gate-measuring-nothing failure, so it is reported as what
        // it is — both on a direct edge and through the closure.
        let direct = [float_sim("world", &[], true, true, &["nalgebra"])];
        let message = &evaluate(&direct)
            .into_iter()
            .find(|finding| finding.rule == "float-closure")
            .expect("a foreign direct dependency is reported")
            .message;
        assert!(message.contains("nalgebra"), "{message}");

        let indirect = [
            float_sim("world", &["middle"], true, true, &[]),
            float_sim("middle", &[], false, true, &["nalgebra"]),
        ];
        let message = &evaluate(&indirect)
            .into_iter()
            .find(|finding| finding.rule == "float-closure")
            .expect("a foreign dependency in the closure is reported")
            .message;
        assert!(message.contains("nalgebra"), "{message}");
    }

    #[test]
    fn float_closure_skips_a_dependency_it_was_handed_no_shape_for() {
        // The walk resolves each name through `shapes`, and a name with no
        // shape is skipped rather than assumed guilty. That branch is not
        // decoration: the metadata scan can drop a package whose manifest
        // failed to parse, and its schema failure is already its own
        // finding — truncating this walk into a second, wrong one would
        // report the same problem twice under a rule that is not about it.
        let shapes = [float_sim("world", &["vanished"], true, true, &[])];
        assert!(
            !evaluate(&shapes)
                .iter()
                .any(|finding| finding.rule == "float-closure"),
            "a name with no shape must be skipped, not reported as a float crate"
        );
    }

    #[test]
    fn float_closure_says_nothing_about_a_crate_that_is_not_a_simulation() {
        let shapes = [
            float_sim("tool", &["maths"], false, true, &[]),
            float_sim("maths", &[], false, false, &["nalgebra"]),
        ];
        assert!(
            !evaluate(&shapes)
                .iter()
                .any(|finding| finding.rule == "float-closure"),
            "the rule binds simulation crates and nothing else"
        );
    }

    #[test]
    fn simulation_closure_sees_through_a_wrapper_crate() {
        // The whole point of checking the graph instead of the source.
        // The determinism lints match a definition path in the crate being
        // compiled, so `world` naming a re-exported type from `wrapper`
        // trips nothing while the capability is one hop away.
        let shapes = [
            sim("world", &["wrapper"], true),
            sim("wrapper", &[PLATFORM_CRATE], false),
            sim(PLATFORM_CRATE, &[], false),
        ];
        let findings = evaluate(&shapes);
        let closure: Vec<_> = findings
            .iter()
            .filter(|f| f.rule == "simulation-closure")
            .collect();
        assert_eq!(closure.len(), 1, "{findings:?}");
        // Bound first: an argument that is only evaluated when the
        // assertion fails is a line the coverage gate never sees run.
        let message = &closure[0].message;
        assert!(
            message.starts_with("world declares simulation = true and reaches"),
            "{message}"
        );
    }

    #[test]
    fn simulation_closure_reports_a_shared_dependency_once() {
        // Two paths to the same capability crate is one defect, not two.
        // The walk therefore remembers where it has been -- and without a
        // diamond in the graph that memory is never exercised, which is
        // how this test earned its place rather than duplicating the one
        // above.
        let shapes = [
            sim("world", &["left", "right"], true),
            sim("left", &[PLATFORM_CRATE], false),
            sim("right", &[PLATFORM_CRATE], false),
            sim(PLATFORM_CRATE, &[], false),
        ];
        let findings = evaluate(&shapes);
        assert_eq!(
            findings
                .iter()
                .filter(|f| f.rule == "simulation-closure")
                .count(),
            1,
            "{findings:?}"
        );
    }

    #[test]
    fn simulation_closure_leaves_a_clean_chain_alone() {
        // Depth is not the trigger — reaching the capability crate is. A
        // deterministic crate may sit on any number of pure crates.
        let shapes = [
            sim("world", &["vocabulary"], true),
            sim("vocabulary", &["values"], false),
            sim("values", &[], false),
            sim(PLATFORM_CRATE, &[], false),
        ];
        assert!(
            evaluate(&shapes)
                .iter()
                .all(|f| f.rule != "simulation-closure"),
            "a chain that never reaches the capability crate is legal"
        );
    }

    #[test]
    fn simulation_closure_refuses_to_pass_when_its_target_is_missing() {
        // The rule names one crate as a string. Renaming that crate would
        // turn this into a walk that can never match, and a rule that can
        // never match reports green forever.
        let shapes = [sim("world", &[], true)];
        assert!(
            evaluate(&shapes)
                .iter()
                .any(|f| f.rule == "simulation-closure" && f.message.contains("no crate is named")),
            "a missing target must fail loudly rather than vacuously pass"
        );
    }

    #[test]
    fn shapes_come_out_of_metadata_shaped_json() {
        let document = crate::json::parse(
            r#"{"workspace_root":"/w","packages":[{"name":"renew-diag","manifest_path":"/w/crates/core/diag/Cargo.toml","dependencies":[{"name":"outside"},{"name":"renew-cli"}],"metadata":{"renew":{"purpose":"p","maturity":"bootstrap","core":true,"extension_points":["sink"],"simulation":false}}},{"name":"renew-cli","manifest_path":"C:\\w\\tools\\cli\\Cargo.toml","dependencies":[],"metadata":{"renew":{"purpose":"p","maturity":"bootstrap","core":false,"extension_points":[],"simulation":false}}}]}"#,
        )
        .expect("document parses");
        let shapes = shapes_from_metadata(&document).expect("shapes build");
        assert_eq!(shapes.len(), 2);
        assert!(shapes[0].engine, "crates/ path is an engine crate");
        assert!(
            !shapes[1].engine,
            "tools/ path is not (backslashes normalized)"
        );
        // Non-workspace dependency names are dropped; workspace ones kept.
        assert_eq!(shapes[0].deps, ["renew-cli"]);
        assert!(shapes[0].meta.is_ok());
    }

    #[test]
    fn metadata_without_packages_fails_loudly() {
        let document = crate::json::parse(r#"{"something":"else"}"#).expect("parses");
        assert!(shapes_from_metadata(&document).is_err());
    }

    #[test]
    fn metadata_without_a_workspace_root_fails_loudly() {
        let document = crate::json::parse(r#"{"packages":[]}"#).expect("parses");
        assert!(shapes_from_metadata(&document).is_err());
    }

    #[test]
    fn a_checkout_under_a_directory_named_crates_does_not_reclassify() {
        // The workspace root itself lives under a `crates` directory; only
        // paths under `<root>/crates/` are engine crates.
        let document = crate::json::parse(
            r#"{"workspace_root":"/d/crates/renew","packages":[{"name":"tool","manifest_path":"/d/crates/renew/tools/x/Cargo.toml","dependencies":[],"metadata":{"renew":{"purpose":"p","maturity":"bootstrap","core":false,"extension_points":[],"simulation":false}}},{"name":"renew-diag","manifest_path":"/d/crates/renew/crates/core/diag/Cargo.toml","dependencies":[],"metadata":{"renew":{"purpose":"p","maturity":"bootstrap","core":true,"extension_points":[],"simulation":false}}}]}"#,
        )
        .expect("parses");
        let shapes = shapes_from_metadata(&document).expect("shapes build");
        assert!(!shapes[0].engine, "tools crate misclassified as engine");
        assert!(shapes[1].engine, "engine crate not recognized");
    }

    #[test]
    fn a_mistyped_metadata_table_is_named_as_mistyped() {
        let document = crate::json::parse(
            r#"{"workspace_root":"/w","packages":[{"name":"x","manifest_path":"/w/x/Cargo.toml","dependencies":[],"metadata":{"renew":5}}]}"#,
        )
        .expect("parses");
        let shapes = shapes_from_metadata(&document).expect("shapes build");
        let problems = shapes[0]
            .meta
            .as_ref()
            .expect_err("a mistyped table is rejected");
        assert!(problems[0].contains("not a table"), "{problems:?}");
    }

    #[test]
    fn a_package_without_a_name_fails_loudly() {
        let document = crate::json::parse(
            r#"{"workspace_root":"/w","packages":[{"manifest_path":"/w/x/Cargo.toml"}]}"#,
        )
        .expect("parses");
        assert!(shapes_from_metadata(&document).is_err());
    }

    #[test]
    fn a_missing_metadata_table_is_reported() {
        let document = crate::json::parse(
            r#"{"workspace_root":"/w","packages":[{"name":"x","manifest_path":"/w/x/Cargo.toml","dependencies":[],"metadata":null}]}"#,
        )
        .expect("parses");
        let shapes = shapes_from_metadata(&document).expect("shapes build");
        let problems = shapes[0]
            .meta
            .as_ref()
            .expect_err("a missing table is rejected");
        assert!(problems[0].contains("missing"), "{problems:?}");
    }

    #[test]
    fn every_wrong_field_type_is_named() {
        let document = crate::json::parse(
            r#"{"workspace_root":"/w","packages":[{"name":"x","manifest_path":"/w/x/Cargo.toml","dependencies":[],"metadata":{"renew":{"purpose":1,"maturity":2,"core":"yes","extension_points":"sink","simulation":"no"}}}]}"#,
        )
        .expect("parses");
        let shapes = shapes_from_metadata(&document).expect("shapes build");
        let problems = shapes[0]
            .meta
            .as_ref()
            .expect_err("wrong field types are rejected");
        for named in [
            "`purpose` is not a string",
            "`maturity` is not a string",
            "`core` is not a boolean",
            "`extension_points` is not an array",
            "`simulation` is not a boolean",
        ] {
            assert!(
                problems.iter().any(|problem| problem == named),
                "missing {named:?} in {problems:?}"
            );
        }
    }

    #[test]
    fn empty_purpose_and_non_string_extension_items_are_named() {
        let document = crate::json::parse(
            r#"{"workspace_root":"/w","packages":[{"name":"x","manifest_path":"/w/x/Cargo.toml","dependencies":[],"metadata":{"renew":{"purpose":"  ","maturity":"bootstrap","core":false,"extension_points":[1],"simulation":false}}}]}"#,
        )
        .expect("parses");
        let shapes = shapes_from_metadata(&document).expect("shapes build");
        let problems = shapes[0]
            .meta
            .as_ref()
            .expect_err("an empty purpose is rejected");
        assert!(problems.iter().any(|p| p.contains("`purpose` is empty")));
        assert!(problems.iter().any(|p| p.contains("items must be strings")));
    }

    #[test]
    fn unknown_dependency_names_do_not_disturb_the_rules() {
        let shapes = [shape("renew-diag", true, &["ghost"], "bootstrap", true)];
        // "ghost" is not a workspace crate: no cycle, no closure finding,
        // but the engine→non-engine rule fires because ghost is unknown.
        let findings = evaluate(&shapes);
        assert!(!findings.iter().any(|f| f.rule == "dependency-cycle"));
        assert!(!findings.iter().any(|f| f.rule == "core-closure"));
    }

    #[test]
    fn a_self_loop_is_a_cycle() {
        let shapes = [shape("a", true, &["a"], "bootstrap", false)];
        let findings = evaluate(&shapes);
        let cycle = findings
            .iter()
            .find(|finding| finding.rule == "dependency-cycle")
            .expect("self-loop reported");
        assert!(cycle.message.contains("a -> a"), "{}", cycle.message);
    }

    #[test]
    fn broken_metadata_dependencies_skip_the_edge_rules() {
        let shapes = [
            shape("renew-diag", true, &["renew-broken"], "bootstrap", true),
            CrateShape {
                name: "renew-broken".to_string(),
                dir: "/w/crates/x".to_string(),
                engine: true,
                deps: Vec::new(),
                runtime_deps: Vec::new(),
                foreign_deps: Vec::new(),
                denies_float: true,
                meta: Err(vec!["missing field `core`".to_string()]),
            },
        ];
        let findings = evaluate(&shapes);
        // The broken crate surfaces as a schema finding, not as closure or
        // maturity noise derived from metadata it does not have.
        assert!(findings.iter().any(|f| f.rule == "schema"));
        assert!(!findings.iter().any(|f| f.rule == "core-closure"));
        assert!(!findings.iter().any(|f| f.rule == "maturity-order"));
    }

    #[test]
    fn an_empty_metadata_table_names_every_missing_field() {
        let document = crate::json::parse(
            r#"{"workspace_root":"/w","packages":[{"name":"x","manifest_path":"/w/crates/x/Cargo.toml","dependencies":[],"metadata":{"renew":{}}}]}"#,
        )
        .expect("parses");
        let shapes = shapes_from_metadata(&document).expect("shapes build");
        let problems = shapes[0]
            .meta
            .as_ref()
            .expect_err("an empty table is missing everything");
        let expected: Vec<String> = REQUIRED_FIELDS
            .iter()
            .map(|required| format!("missing field `{required}`"))
            .collect();
        // Exactly the five: an absent field is reported once, and never
        // doubled up with a type complaint about a value that is not there.
        assert_eq!(problems, &expected);
    }

    #[test]
    fn a_core_listed_crate_outside_the_engine_roots_is_flagged() {
        let shapes = [shape("renew-math", false, &[], "bootstrap", false)];
        let findings = evaluate(&shapes);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].rule, "core-agreement");
        assert!(
            findings[0].message.contains("not an engine crate"),
            "{findings:?}"
        );
    }

    #[test]
    fn core_closure_walks_a_diamond_once_and_terminates() {
        // renew-memory is reachable by two paths. The walk must not
        // re-expand it — and must still find nothing wrong here.
        let shapes = [
            shape(
                "renew-diag",
                true,
                &["renew-platform", "renew-math"],
                "bootstrap",
                true,
            ),
            shape("renew-platform", true, &["renew-memory"], "bootstrap", true),
            shape("renew-math", true, &["renew-memory"], "bootstrap", true),
            shape("renew-memory", true, &[], "bootstrap", true),
        ];
        assert_eq!(evaluate(&shapes), Vec::new());
    }

    #[test]
    fn closed_schema_rejects_unknown_fields_and_an_invalid_maturity() {
        let document = crate::json::parse(
            r#"{"workspace_root":"/w","packages":[{"name":"x","manifest_path":"/w/crates/x/Cargo.toml","dependencies":[],"metadata":{"renew":{"purpose":"p","maturity":"weird","core":true,"extension_points":[],"simulation":false,"extra":1}}}]}"#,
        )
        .expect("parses");
        let shapes = shapes_from_metadata(&document).expect("shapes build");
        let problems = shapes[0]
            .meta
            .as_ref()
            .expect_err("the closed schema is enforced");
        assert!(problems.iter().any(|p| p.contains("unknown field `extra`")));
        assert!(problems.iter().any(|p| p.contains("`maturity`")));
    }
}
