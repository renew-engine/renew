//! Environment checks: pure pass/fail logic over facts gathered by the
//! binary, so the rules are testable without spawning processes.

/// Fallback minimum `cargo` version (major, minor), used only when the
/// workspace manifest's `rust-version` cannot be read — the manifest is the
/// source of truth.
pub const MIN_CARGO: (u64, u64) = (1, 97);

/// Facts about the environment, gathered by the caller.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub rustup_found: bool,
    /// Why the rustup probe could not run, when it could not. Absent
    /// means it ran; the report then never has to guess at a cause.
    pub rustup_unavailable: Option<String>,
    pub active_toolchain: Option<String>,
    pub cargo_version: Option<(u64, u64)>,
    pub toolchain_file_channel: Option<String>,
    /// `rust-version` floor parsed from the workspace manifest, if readable.
    pub required_cargo: Option<(u64, u64)>,
    pub workspace_root_found: bool,
    pub git_found: bool,
    /// Why the git probe could not run, when it could not.
    pub git_unavailable: Option<String>,
}

/// How to describe a probe that would not run.
///
/// "Not found on PATH" is the common cause and was, until this existed,
/// the only thing the report could say — including when the program was
/// plainly there and the failure was something else entirely. A tool that
/// names the wrong cause sends its reader to fix the wrong thing, so an
/// unexpected kind is now reported as itself.
#[must_use]
pub fn probe_failure(kind: std::io::ErrorKind) -> String {
    if kind == std::io::ErrorKind::NotFound {
        String::from("not found on PATH")
    } else {
        format!("on PATH but would not run ({kind})")
    }
}

/// One evaluated check.
#[derive(Clone, Debug)]
pub struct Check {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

/// Whether the active toolchain matches the pinned channel: exact, or the
/// channel followed by a target-triple suffix (`stable` matches
/// `stable-x86_64-pc-windows-msvc`).
fn active_matches_pin(active: &str, pin: &str) -> bool {
    active == pin
        || active
            .strip_prefix(pin)
            .is_some_and(|rest| rest.starts_with('-'))
}

/// Evaluate every doctor check against the gathered facts.
#[must_use]
pub fn evaluate(facts: &Facts) -> Vec<Check> {
    let active = facts.active_toolchain.as_deref();
    let pin = facts.toolchain_file_channel.as_deref();
    let toolchain_ok = matches!((active, pin), (Some(a), Some(p)) if active_matches_pin(a, p));
    let toolchain_detail = match (active, pin) {
        (Some(a), Some(p)) if toolchain_ok => format!("{a} (matches pin `{p}`)"),
        (Some(a), Some(p)) => format!("{a} does not match pin `{p}`"),
        (Some(a), None) => format!("{a} (no pin to compare against)"),
        (None, _) => String::from("not detected"),
    };
    let floor = facts.required_cargo.unwrap_or(MIN_CARGO);
    let cargo_ok = facts.cargo_version.is_some_and(|version| version >= floor);
    vec![
        Check {
            name: "rustup",
            ok: facts.rustup_found,
            detail: if facts.rustup_found {
                String::from("found")
            } else {
                facts
                    .rustup_unavailable
                    .clone()
                    .unwrap_or_else(|| String::from("not found on PATH"))
            },
        },
        Check {
            name: "toolchain",
            ok: toolchain_ok,
            detail: toolchain_detail,
        },
        Check {
            name: "toolchain-pin",
            ok: facts.toolchain_file_channel.is_some(),
            detail: facts
                .toolchain_file_channel
                .clone()
                .unwrap_or_else(|| String::from("rust-toolchain.toml missing or unreadable")),
        },
        Check {
            name: "cargo",
            ok: cargo_ok,
            detail: match facts.cargo_version {
                Some((major, minor)) => {
                    format!("cargo {major}.{minor} (floor {}.{})", floor.0, floor.1)
                }
                None => String::from("not detected"),
            },
        },
        Check {
            name: "workspace",
            ok: facts.workspace_root_found,
            detail: if facts.workspace_root_found {
                String::from("found")
            } else {
                String::from("no workspace root above the current directory")
            },
        },
        Check {
            name: "git",
            ok: facts.git_found,
            detail: if facts.git_found {
                String::from("found")
            } else {
                facts
                    .git_unavailable
                    .clone()
                    .unwrap_or_else(|| String::from("not found on PATH"))
            },
        },
    ]
}

/// Whether every check passed.
#[must_use]
pub fn all_ok(checks: &[Check]) -> bool {
    checks.iter().all(|check| check.ok)
}

/// Parse a major/minor pair out of `cargo --version` output
/// (e.g. `cargo 1.97.1 (…)`).
#[must_use]
pub fn parse_cargo_version(text: &str) -> Option<(u64, u64)> {
    let mut words = text.split_whitespace();
    let _tool = words.next()?;
    let version = words.next()?;
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Extract the pinned channel from `rust-toolchain.toml` text.
#[must_use]
pub fn parse_toolchain_channel(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("channel")
            && let Some(value) = rest.trim_start().strip_prefix('=')
        {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// Extract the `rust-version` floor from workspace-manifest text
/// (e.g. `rust-version = "1.97"`).
#[must_use]
pub fn parse_rust_version(text: &str) -> Option<(u64, u64)> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("rust-version")
            && let Some(value) = rest.trim_start().strip_prefix('=')
        {
            let version = value.trim().trim_matches('"');
            let mut parts = version.split('.');
            let major = parts.next()?.parse().ok()?;
            let minor = parts.next()?.parse().ok()?;
            return Some((major, minor));
        }
    }
    None
}

/// The first whitespace-delimited token of a text, if any.
#[must_use]
pub fn first_token(text: &str) -> Option<String> {
    text.split_whitespace().next().map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A probe that could not run says which way it could not. Reporting
    /// "not found on PATH" for a program that is plainly on PATH sends the
    /// reader to fix the wrong thing, which is the failure this exists to
    /// prevent.
    #[test]
    fn a_probe_failure_names_the_kind_unless_it_really_is_missing() {
        assert_eq!(
            probe_failure(std::io::ErrorKind::NotFound),
            "not found on PATH"
        );
        let denied = probe_failure(std::io::ErrorKind::PermissionDenied);
        assert!(denied.contains("would not run"), "{denied}");
        assert!(denied.starts_with("on PATH"), "{denied}");
        assert_ne!(denied, probe_failure(std::io::ErrorKind::NotFound));
    }

    fn healthy() -> Facts {
        Facts {
            rustup_found: true,
            rustup_unavailable: None,
            active_toolchain: Some(String::from("stable-x86_64-pc-windows-msvc")),
            cargo_version: Some((1, 97)),
            toolchain_file_channel: Some(String::from("stable")),
            required_cargo: Some((1, 97)),
            workspace_root_found: true,
            git_found: true,
            git_unavailable: None,
        }
    }

    #[test]
    fn healthy_environment_passes_every_check() {
        let checks = evaluate(&healthy());
        assert_eq!(checks.len(), 6);
        assert!(all_ok(&checks), "failing checks: {checks:?}");
    }

    #[test]
    fn an_empty_environment_fails_every_check_with_a_stated_reason() {
        let checks = evaluate(&Facts::default());
        assert!(!all_ok(&checks));
        let reported: Vec<(&str, bool, &str)> = checks
            .iter()
            .map(|check| (check.name, check.ok, check.detail.as_str()))
            .collect();
        assert_eq!(
            reported,
            [
                ("rustup", false, "not found on PATH"),
                ("toolchain", false, "not detected"),
                (
                    "toolchain-pin",
                    false,
                    "rust-toolchain.toml missing or unreadable"
                ),
                ("cargo", false, "not detected"),
                (
                    "workspace",
                    false,
                    "no workspace root above the current directory"
                ),
                ("git", false, "not found on PATH"),
            ]
        );
    }

    #[test]
    fn missing_rustup_fails_only_its_check() {
        let facts = Facts {
            rustup_found: false,
            ..healthy()
        };
        let checks = evaluate(&facts);
        let failing: Vec<&str> = checks
            .iter()
            .filter(|check| !check.ok)
            .map(|check| check.name)
            .collect();
        assert_eq!(failing, ["rustup"]);
    }

    #[test]
    fn toolchain_not_matching_the_pin_fails_the_toolchain_check() {
        let facts = Facts {
            active_toolchain: Some(String::from("nightly-x86_64-unknown-linux-gnu")),
            ..healthy()
        };
        assert!(!all_ok(&evaluate(&facts)));
    }

    #[test]
    fn toolchain_check_follows_the_pin_not_a_hardcoded_channel() {
        // A version pin is matched exactly the same way as a channel pin.
        let facts = Facts {
            active_toolchain: Some(String::from("1.98.0-x86_64-pc-windows-msvc")),
            toolchain_file_channel: Some(String::from("1.98.0")),
            ..healthy()
        };
        assert!(all_ok(&evaluate(&facts)), "{:?}", evaluate(&facts));
        // A prefix that is not followed by a target-triple separator is not
        // a match: pin `1.9` must not accept active `1.98.0-...`.
        let near_miss = Facts {
            active_toolchain: Some(String::from("1.98.0-x86_64-pc-windows-msvc")),
            toolchain_file_channel: Some(String::from("1.9")),
            ..healthy()
        };
        assert!(!all_ok(&evaluate(&near_miss)));
    }

    #[test]
    fn missing_pin_fails_both_toolchain_checks() {
        let facts = Facts {
            toolchain_file_channel: None,
            ..healthy()
        };
        let checks = evaluate(&facts);
        let failing: Vec<&str> = checks
            .iter()
            .filter(|check| !check.ok)
            .map(|check| check.name)
            .collect();
        assert_eq!(failing, ["toolchain", "toolchain-pin"]);
    }

    #[test]
    fn cargo_version_floor_is_inclusive() {
        for (version, expected_ok) in [((1, 96), false), ((1, 97), true), ((2, 0), true)] {
            let facts = Facts {
                cargo_version: Some(version),
                ..healthy()
            };
            let checks = evaluate(&facts);
            let cargo = checks
                .iter()
                .find(|check| check.name == "cargo")
                .expect("cargo check present");
            assert_eq!(cargo.ok, expected_ok, "version {version:?}");
        }
    }

    #[test]
    fn cargo_version_output_parses() {
        assert_eq!(
            parse_cargo_version("cargo 1.97.1 (c980f4866 2026-06-30)"),
            Some((1, 97))
        );
        assert_eq!(parse_cargo_version("cargo 2.0.0"), Some((2, 0)));
        assert_eq!(parse_cargo_version("not a version"), None);
        assert_eq!(parse_cargo_version(""), None);
    }

    #[test]
    fn toolchain_channel_parses_from_toml_text() {
        assert_eq!(
            parse_toolchain_channel("[toolchain]\nchannel = \"stable\"\n"),
            Some(String::from("stable"))
        );
        assert_eq!(parse_toolchain_channel("[toolchain]\n"), None);
    }

    #[test]
    fn rust_version_floor_parses_from_manifest_text() {
        assert_eq!(
            parse_rust_version("[workspace.package]\nrust-version = \"1.97\"\n"),
            Some((1, 97))
        );
        assert_eq!(parse_rust_version("[workspace.package]\n"), None);
        assert_eq!(parse_rust_version("rust-version = \"nonsense\""), None);
    }

    #[test]
    fn manifest_floor_overrides_the_fallback() {
        let facts = Facts {
            required_cargo: Some((2, 0)),
            cargo_version: Some((1, 97)),
            ..healthy()
        };
        let checks = evaluate(&facts);
        let cargo = checks
            .iter()
            .find(|check| check.name == "cargo")
            .expect("cargo check present");
        assert!(!cargo.ok, "floor (2,0) must reject cargo 1.97");
    }

    #[test]
    fn first_token_takes_the_leading_word() {
        assert_eq!(
            first_token("stable-x86_64-pc-windows-msvc (overridden)"),
            Some(String::from("stable-x86_64-pc-windows-msvc"))
        );
        assert_eq!(first_token("   \n"), None);
    }
}
