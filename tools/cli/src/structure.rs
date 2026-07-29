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
pub const CORE_CRATES: &[&str] = &["renew-diag", "renew-platform", "renew-memory", "renew-math"];

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
    /// Under the engine module roots (`crates/`)?
    pub engine: bool,
    /// Workspace-internal dependencies, any kind, dev included.
    pub deps: Vec<String>,
    /// Parsed metadata table, or the list of schema problems.
    pub meta: Result<Meta, Vec<String>>,
}

/// The validated fields the rules consume.
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
        let deps = package
            .get("dependencies")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|dep| dep.get("name").and_then(Value::as_str))
                    .filter(|dep_name| names.iter().any(|known| known == dep_name))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let meta = validate_meta(package);
        shapes.push(CrateShape {
            name,
            engine,
            deps,
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
        if !shape.engine && meta.simulation {
            findings.push(Finding {
                rule: "non-engine",
                message: format!(
                    "{} is outside crates/ and must declare simulation = false",
                    shape.name
                ),
            });
        }
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
    let index_of = |name: &str| shapes.iter().position(|shape| shape.name == name);
    let mut findings = Vec::new();

    for start in 0..shapes.len() {
        if states.get(start) != Some(&State::New) {
            continue;
        }
        // (crate index, next dependency position) — an explicit stack so
        // the path is available for the report.
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        if let Some(state) = states.get_mut(start) {
            *state = State::Open;
        }
        while let Some((current, position)) = stack.last().copied() {
            let Some(shape) = shapes.get(current) else {
                break;
            };
            if let Some(dep_name) = shape.deps.get(position) {
                if let Some(last) = stack.last_mut() {
                    last.1 += 1;
                }
                let Some(dep_index) = index_of(dep_name) else {
                    continue;
                };
                match states.get(dep_index).copied() {
                    Some(State::New) => {
                        if let Some(state) = states.get_mut(dep_index) {
                            *state = State::Open;
                        }
                        stack.push((dep_index, 0));
                    }
                    Some(State::Open) => {
                        let mut path: Vec<&str> = stack
                            .iter()
                            .skip_while(|(index, _)| *index != dep_index)
                            .filter_map(|(index, _)| shapes.get(*index).map(|s| s.name.as_str()))
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
            engine,
            deps: deps.iter().map(ToString::to_string).collect(),
            meta: Ok(Meta {
                maturity: maturity.to_string(),
                core,
                simulation: false,
            }),
        }
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
            engine: false,
            deps: Vec::new(),
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
            engine: false,
            deps: Vec::new(),
            meta: Ok(Meta {
                maturity: "bootstrap".to_string(),
                core: true,
                simulation: true,
            }),
        }];
        let findings = evaluate(&flagged);
        assert_eq!(
            findings.iter().filter(|f| f.rule == "non-engine").count(),
            2
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
        let Err(problems) = &shapes[0].meta else {
            panic!("mistyped table should fail");
        };
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
        let Err(problems) = &shapes[0].meta else {
            panic!("missing table should fail");
        };
        assert!(problems[0].contains("missing"), "{problems:?}");
    }

    #[test]
    fn every_wrong_field_type_is_named() {
        let document = crate::json::parse(
            r#"{"workspace_root":"/w","packages":[{"name":"x","manifest_path":"/w/x/Cargo.toml","dependencies":[],"metadata":{"renew":{"purpose":1,"maturity":2,"core":"yes","extension_points":"sink","simulation":"no"}}}]}"#,
        )
        .expect("parses");
        let shapes = shapes_from_metadata(&document).expect("shapes build");
        let Err(problems) = &shapes[0].meta else {
            panic!("wrong types should fail");
        };
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
        let Err(problems) = &shapes[0].meta else {
            panic!("should fail");
        };
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
                engine: true,
                deps: Vec::new(),
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
    fn closed_schema_rejects_unknown_and_missing_fields() {
        let document = crate::json::parse(
            r#"{"workspace_root":"/w","packages":[{"name":"x","manifest_path":"/w/crates/x/Cargo.toml","dependencies":[],"metadata":{"renew":{"purpose":"p","maturity":"weird","core":true,"extension_points":[],"simulation":false,"extra":1}}}]}"#,
        )
        .expect("parses");
        let shapes = shapes_from_metadata(&document).expect("shapes build");
        let Err(problems) = &shapes[0].meta else {
            panic!("schema should fail");
        };
        assert!(problems.iter().any(|p| p.contains("unknown field `extra`")));
        assert!(problems.iter().any(|p| p.contains("`maturity`")));
    }
}
