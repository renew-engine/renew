//! Which runnable samples this workspace contains.
//!
//! Read out of `cargo metadata` rather than written down here. A list in
//! the source would be a second place to edit every time a sample is
//! added, renamed, or removed — and the copy that rots is always the one
//! nobody runs.

use crate::json::Value;

/// Where samples live, relative to the workspace root. Membership is
/// decided by location, the same way `structure.rs` decides which crates
/// are engine crates, so a sample is a sample by virtue of where it sits.
pub const SAMPLES_ROOT: &str = "samples/";

/// One runnable sample.
///
/// `name` is the binary target's name — what a person types, and what the
/// sample calls itself in its own output. `package` is what cargo needs
/// to be told, and it is deliberately not the same string: the packages
/// carry a `renew-sample-` prefix that nobody should have to type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    pub name: String,
    pub package: String,
}

/// Read the samples out of a parsed `cargo metadata --no-deps` document.
///
/// # Errors
///
/// Returns a message when the document is not `cargo metadata` output at
/// all — a lookup that cannot read its input must fail loudly, never
/// report an empty workspace and blame the caller's spelling.
pub fn from_metadata(doc: &Value) -> Result<Vec<Sample>, String> {
    let packages = doc
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| "no `packages` array in cargo metadata output".to_string())?;
    // Judged relative to the workspace root, never by a substring of the
    // absolute checkout path: a checkout under a directory called
    // `samples` must not turn every crate into a sample.
    let workspace_root = doc
        .get("workspace_root")
        .and_then(Value::as_str)
        .ok_or_else(|| "no `workspace_root` in cargo metadata output".to_string())?
        .replace('\\', "/");
    let samples_root = format!("{}/{SAMPLES_ROOT}", workspace_root.trim_end_matches('/'));

    let mut found = Vec::new();
    for package in packages {
        let manifest = package
            .get("manifest_path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .replace('\\', "/");
        if !manifest.starts_with(&samples_root) {
            continue;
        }
        let Some(package_name) = package.get("name").and_then(Value::as_str) else {
            continue;
        };
        let targets = package
            .get("targets")
            .and_then(Value::as_array)
            .unwrap_or_default();
        for target in targets {
            // Samples also carry library, test, and bench targets; only a
            // binary is something `run` can start.
            if !is_binary(target) {
                continue;
            }
            if let Some(binary) = target.get("name").and_then(Value::as_str) {
                found.push(Sample {
                    name: binary.to_string(),
                    package: package_name.to_string(),
                });
            }
        }
    }
    // Sorted so the list a usage error prints reads the same everywhere,
    // whatever order cargo happened to emit the packages in.
    found.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(found)
}

fn is_binary(target: &Value) -> bool {
    target
        .get("kind")
        .and_then(Value::as_array)
        .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some("bin")))
}

/// Look a sample up by the name it is invoked by.
#[must_use]
pub fn find<'a>(known: &'a [Sample], name: &str) -> Option<&'a Sample> {
    known.iter().find(|sample| sample.name == name)
}

/// The message for a name that matches no sample. It names the samples
/// that do exist, because that list is the only thing worth saying to
/// someone who just mistyped one of them.
#[must_use]
pub fn unknown(name: &str, known: &[Sample]) -> String {
    if known.is_empty() {
        return format!("unknown sample `{name}`; this workspace has no samples");
    }
    let names: Vec<&str> = known.iter().map(|sample| sample.name.as_str()).collect();
    format!(
        "unknown sample `{name}`; this workspace has: {}",
        names.join(", ")
    )
}

/// The marker a sample's digest line begins with. A convention, not a
/// contract the tool can enforce — the same weakness the trace flags
/// have, and it resolves the same way, by the manifest eventually
/// declaring what a sample emits.
const DIGEST_MARKER: &str = "renew-frame ";

/// The digest line in a sample's output, if it printed one.
///
/// The **last** match, not the first: a run that somehow reported twice
/// is described by where it ended up, and taking the first would report a
/// digest the sample itself superseded. Trailing whitespace is trimmed
/// because the line crosses a pipe and a `\r` would otherwise ride into
/// the envelope.
#[must_use]
pub fn digest_line(stdout: &str) -> Option<&str> {
    stdout
        .lines()
        .map(str::trim_end)
        .rfind(|line| line.starts_with(DIGEST_MARKER))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Realistic shape: the sample prints its own chatter, then the
    /// digest last. Written out rather than built from the sample’s own
    /// formatter, because a shared formatter would let both sides drift
    /// together and still agree.
    const RUN_OUTPUT: &str = concat!(
        "started input_echo\n",
        "renew-frame sample=input_echo seed=3 source=trace frames=20 ticks=20 ",
        "dropped=0 schedule_hash=0x0000000000000001 state_hash=0x00000000000000ff\n",
    );

    #[test]
    fn the_digest_line_is_found_among_the_samples_other_output() {
        let found = digest_line(RUN_OUTPUT).expect("the run printed a digest");
        assert!(found.starts_with("renew-frame sample=input_echo seed=3"));
        assert!(found.ends_with("state_hash=0x00000000000000ff"));
    }

    #[test]
    fn output_without_a_digest_has_none() {
        assert_eq!(digest_line(""), None);
        assert_eq!(digest_line("started input_echo\nfinished\n"), None);
        // The marker has to begin the line: a sample quoting it mid-
        // sentence is talking about a digest, not printing one.
        assert_eq!(digest_line("about to print renew-frame sample=x"), None);
    }

    #[test]
    fn the_last_digest_wins_when_a_run_printed_more_than_one() {
        let twice = concat!(
            "renew-frame sample=a state_hash=0x1\n",
            "renew-frame sample=a state_hash=0x2\n",
        );
        assert_eq!(
            digest_line(twice),
            Some("renew-frame sample=a state_hash=0x2")
        );
    }

    /// The line crosses a pipe, and a child on Windows ends it with a
    /// carriage return before the newline. That byte must not ride into
    /// the envelope, where it would make two identical digests compare
    /// unequal depending on which platform recorded them.
    #[test]
    fn a_carriage_return_does_not_reach_the_caller() {
        let windows = "renew-frame sample=a state_hash=0x1\r\n";
        assert!(
            windows.contains('\r'),
            "the input must carry the byte under test"
        );
        assert_eq!(
            digest_line(windows),
            Some("renew-frame sample=a state_hash=0x1")
        );
    }

    /// Two sample packages and one crate that is not one, in the shape
    /// `cargo metadata --format-version 1 --no-deps` emits: a sample
    /// carries a library and a test target beside its binary, and its
    /// package name is not the name the binary answers to. The root and
    /// one manifest are spelled with backslashes, as they are on Windows.
    const METADATA: &str = concat!(
        r#"{"workspace_root":"C:\\w","packages":["#,
        r#"{"name":"renew-sample-input-echo","#,
        r#""manifest_path":"C:/w/samples/input_echo/Cargo.toml","#,
        r#""targets":[{"kind":["bin"],"name":"input_echo"}]},"#,
        r#"{"name":"renew-sample-hello-triangle","#,
        r#""manifest_path":"C:\\w\\samples\\hello_triangle\\Cargo.toml","targets":["#,
        r#"{"kind":["lib"],"name":"renew_sample_hello_triangle"},"#,
        r#"{"kind":["bin"],"name":"hello_triangle"},"#,
        r#"{"kind":["test"],"name":"headless_frame"}]},"#,
        r#"{"name":"renew-cli","manifest_path":"C:/w/tools/cli/Cargo.toml","#,
        r#""targets":[{"kind":["bin"],"name":"renew"}]}]}"#,
    );

    fn parsed(text: &str) -> Value {
        crate::json::parse(text).expect("document parses")
    }

    #[test]
    fn only_binaries_under_the_samples_root_are_samples() {
        let found = from_metadata(&parsed(METADATA)).expect("samples read");
        assert_eq!(
            found,
            vec![
                Sample {
                    name: "hello_triangle".to_string(),
                    package: "renew-sample-hello-triangle".to_string(),
                },
                Sample {
                    name: "input_echo".to_string(),
                    package: "renew-sample-input-echo".to_string(),
                },
            ],
            "sorted by name, backslash paths normalized, `renew` itself excluded"
        );
    }

    #[test]
    fn a_checkout_under_a_directory_named_samples_does_not_reclassify() {
        let document = parsed(concat!(
            r#"{"workspace_root":"/d/samples/renew","packages":[{"name":"renew-cli","#,
            r#""manifest_path":"/d/samples/renew/tools/cli/Cargo.toml","#,
            r#""targets":[{"kind":["bin"],"name":"renew"}]}]}"#,
        ));
        assert_eq!(from_metadata(&document), Ok(Vec::new()));
    }

    #[test]
    fn packages_missing_the_fields_a_sample_needs_are_skipped() {
        // A nameless package, a package with no targets at all, and a
        // target with a kind but no name: each is unusable, and none of
        // them may take the whole lookup down with it.
        let document = parsed(concat!(
            r#"{"workspace_root":"/w","packages":["#,
            r#"{"manifest_path":"/w/samples/anonymous/Cargo.toml","#,
            r#""targets":[{"kind":["bin"],"name":"anonymous"}]},"#,
            r#"{"name":"bare","manifest_path":"/w/samples/bare/Cargo.toml"},"#,
            r#"{"name":"nameless","manifest_path":"/w/samples/nameless/Cargo.toml","#,
            r#""targets":[{"kind":["bin"]},{"name":"kindless"}]}]}"#,
        ));
        assert_eq!(from_metadata(&document), Ok(Vec::new()));
    }

    #[test]
    fn a_document_that_is_not_metadata_fails_loudly() {
        assert!(from_metadata(&parsed(r#"{"something":"else"}"#)).is_err());
        assert!(from_metadata(&parsed(r#"{"packages":[]}"#)).is_err());
    }

    #[test]
    fn lookup_matches_the_invocation_name_and_nothing_else() {
        let found = from_metadata(&parsed(METADATA)).expect("samples read");
        assert_eq!(
            find(&found, "input_echo").map(|sample| sample.package.as_str()),
            Some("renew-sample-input-echo")
        );
        // The package name is not an alias for the binary name.
        assert_eq!(find(&found, "renew-sample-input-echo"), None);
    }

    #[test]
    fn the_unknown_message_lists_what_does_exist() {
        let found = from_metadata(&parsed(METADATA)).expect("samples read");
        let message = unknown("hello_trinagle", &found);
        assert!(
            message.contains("unknown sample `hello_trinagle`"),
            "{message}"
        );
        assert!(message.contains("hello_triangle, input_echo"), "{message}");

        // A workspace with no samples has no list to print, and says so
        // rather than trailing off after a colon.
        let message = unknown("anything", &[]);
        assert!(message.contains("no samples"), "{message}");
    }
}
