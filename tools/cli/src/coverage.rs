//! The coverage gate: an `llvm-cov` JSON export read against the
//! repository's named line exemptions.
//!
//! The gate demands full line coverage of everything the manifest does not
//! list, and it ratchets in both directions — an uncovered line with no
//! entry is a new gap, and an entry whose line is covered now is a stale
//! exemption to delete rather than a hole to leave in the gate.
//!
//! # Which lines count as uncovered
//!
//! Not the segment table (`data[].files[].segments[]`): that describes how
//! `llvm-cov show` paints each line, and it marks closing braces and
//! never-taken `else` arms that the report itself counts as covered.
//!
//! The rule here is the region rule, the one `cargo llvm-cov
//! --show-missing-lines` reports against: a line is uncovered when some
//! region with a zero execution count spans it and no region with a
//! positive count does. Regions live under `data[].functions[].regions[]`
//! as `[line_start, column_start, line_end, column_end, execution_count,
//! file_id, …]`, where `file_id` indexes the record's own `filenames`.
//! Verified against a real export of this workspace: the region rule
//! reproduces `--show-missing-lines` exactly, while the segment rule names
//! fifteen further lines that the report itself counts as covered.
//!
//! The measured file set comes from `data[].files[]` — the half
//! `--ignore-filename-regex` has already filtered. Function records still
//! name ignored files (integration-test sources, registry crates, the
//! excluded tool crate), so anything outside that set is not this gate's
//! business.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};

use crate::json::Value;

/// The exemption manifest, read from the workspace root.
pub const MANIFEST: &str = "coverage-exemptions.toml";

/// The table header each manifest entry opens with.
const TABLE: &str = "[[exempt]]";

/// The keys an entry may carry. The schema is closed.
const KEYS: &[&str] = &["file", "lines", "reason"];

/// A region spanning more lines than this is not a region of a source
/// file; it is a misread export, and expanding it would hang the gate.
const MAX_REGION_LINES: u32 = 100_000;

/// One source line, repository-relative with forward slashes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Site {
    pub file: String,
    pub line: u32,
}

impl fmt::Display for Site {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.file, self.line)
    }
}

/// One manifest entry: the lines of one file that share one reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exemption {
    pub file: String,
    pub lines: Vec<u32>,
    pub reason: String,
}

/// Why a listed line no longer earns its exemption.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaleKind {
    /// The report measured the line and found it covered.
    NowCovered,
    /// The report does not measure the file at all — renamed, deleted, or
    /// filtered out of the collection.
    FileAbsent,
}

impl StaleKind {
    /// The machine-readable tag.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::NowCovered => "now-covered",
            Self::FileAbsent => "file-absent",
        }
    }

    /// The human-readable half of the finding, and what to do about it.
    #[must_use]
    pub fn explanation(self) -> &'static str {
        match self {
            Self::NowCovered => "is covered now — delete this exemption",
            Self::FileAbsent => "is not in the report — delete or move this exemption",
        }
    }
}

/// One exemption that has stopped being true.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stale {
    pub site: Site,
    pub reason: String,
    pub kind: StaleKind,
}

/// What the report says. Both lists are sorted and hold no duplicates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Measured {
    pub uncovered: Vec<Site>,
    pub files: Vec<String>,
}

/// The verdict. Both lists empty is a pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Uncovered lines with no exemption: new gaps.
    pub gaps: Vec<Site>,
    /// Exempt lines the report no longer justifies.
    pub stale: Vec<Stale>,
    /// How many lines the manifest exempts.
    pub exempt_lines: usize,
    /// How many files the report measured.
    pub measured_files: usize,
}

impl Outcome {
    /// Whether the gate passes.
    #[must_use]
    pub fn passes(&self) -> bool {
        self.gaps.is_empty() && self.stale.is_empty()
    }

    /// How many findings there are, counting both directions.
    #[must_use]
    pub fn findings(&self) -> usize {
        self.gaps.len() + self.stale.len()
    }
}

/// Rewrite an export path as a repository-relative, forward-slash path.
///
/// The export carries absolute native paths — backslashes on Windows, and
/// a drive letter in whatever case the toolchain emitted; the manifest
/// holds repository-relative forward-slash paths. A path that is not under
/// `root` comes back normalized but whole, so it can still be named in a
/// message rather than quietly turning into something that matches.
#[must_use]
pub fn relative_to(root: &str, path: &str) -> String {
    let prefix = format!("{}/", root.replace('\\', "/").trim_end_matches('/'));
    let path = path.replace('\\', "/");
    match (path.get(..prefix.len()), path.get(prefix.len()..)) {
        (Some(head), Some(tail)) if head.eq_ignore_ascii_case(&prefix) && !tail.is_empty() => {
            tail.to_string()
        }
        _ => path,
    }
}

/// Read the exemption manifest.
///
/// The accepted syntax is the slice of TOML this file needs and no more:
/// whole-line `#` comments, blank lines, `[[exempt]]` headers, and
/// `key = value` where a value is either a double-quoted string with no
/// escapes or a one-line array of line numbers. Anything else is an error
/// naming its line — a gate configuration that half-parses is worse than
/// one that refuses.
///
/// # Errors
///
/// Returns a message naming the offending line for any syntax the subset
/// does not accept, any unknown or repeated key, any entry missing a key,
/// any path that is not repository-relative with forward slashes, and any
/// line exempted twice.
pub fn parse_manifest(text: &str) -> Result<Vec<Exemption>, String> {
    let mut entries: Vec<Exemption> = Vec::new();
    let mut draft: Option<Draft> = None;

    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == TABLE {
            if let Some(previous) = draft.take() {
                entries.push(previous.finish()?);
            }
            draft = Some(Draft::new(number));
            continue;
        }
        if line.starts_with('[') {
            return Err(format!("line {number}: expected `{TABLE}`, found `{line}`"));
        }
        let Some(open) = draft.as_mut() else {
            return Err(format!(
                "line {number}: `{line}` appears before the first `{TABLE}`"
            ));
        };
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!(
                "line {number}: expected `key = value`, found `{line}`"
            ));
        };
        open.set(key.trim(), value.trim(), number)?;
    }
    if let Some(last) = draft.take() {
        entries.push(last.finish()?);
    }

    let mut seen: Vec<Site> = Vec::new();
    for entry in &entries {
        for line in &entry.lines {
            let site = Site {
                file: entry.file.clone(),
                line: *line,
            };
            if seen.contains(&site) {
                return Err(format!("{site} is exempted twice"));
            }
            seen.push(site);
        }
    }
    Ok(entries)
}

/// One entry under construction, so a missing key is reported against the
/// header it belongs to rather than against the end of the file.
struct Draft {
    header: usize,
    file: Option<String>,
    lines: Option<Vec<u32>>,
    reason: Option<String>,
}

impl Draft {
    fn new(header: usize) -> Self {
        Self {
            header,
            file: None,
            lines: None,
            reason: None,
        }
    }

    fn set(&mut self, key: &str, value: &str, number: usize) -> Result<(), String> {
        let taken = match key {
            "file" => self.file.is_some(),
            "lines" => self.lines.is_some(),
            "reason" => self.reason.is_some(),
            other => {
                return Err(format!(
                    "line {number}: unknown key `{other}` (the schema is closed: {})",
                    KEYS.join(", ")
                ));
            }
        };
        if taken {
            return Err(format!("line {number}: `{key}` is set twice in one entry"));
        }
        match key {
            "file" => {
                let path = string_value(value, number)?;
                check_path(&path, number)?;
                self.file = Some(path);
            }
            "lines" => self.lines = Some(line_numbers(value, number)?),
            _ => {
                let reason = string_value(value, number)?;
                if reason.trim().is_empty() {
                    return Err(format!("line {number}: `reason` is empty"));
                }
                self.reason = Some(reason);
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<Exemption, String> {
        let header = self.header;
        let missing = |key: &str| format!("line {header}: this `{TABLE}` entry has no `{key}`");
        Ok(Exemption {
            file: self.file.ok_or_else(|| missing("file"))?,
            lines: self.lines.ok_or_else(|| missing("lines"))?,
            reason: self.reason.ok_or_else(|| missing("reason"))?,
        })
    }
}

/// A double-quoted string with no escapes: the closing quote is the next
/// one, and nothing but whitespace may follow it.
fn string_value(value: &str, number: usize) -> Result<String, String> {
    let rest = value
        .strip_prefix('"')
        .ok_or_else(|| format!("line {number}: expected a double-quoted string"))?;
    let (text, tail) = rest
        .split_once('"')
        .ok_or_else(|| format!("line {number}: unterminated string"))?;
    if !tail.trim().is_empty() {
        return Err(format!(
            "line {number}: unexpected text after the value (values carry no escapes, no inner quotes, and no trailing comment)"
        ));
    }
    Ok(text.to_string())
}

/// A one-line array of positive line numbers.
fn line_numbers(value: &str, number: usize) -> Result<Vec<u32>, String> {
    let inner = value
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .ok_or_else(|| {
            format!("line {number}: `lines` must be one array on one line, like [75, 95]")
        })?;
    if inner.trim().is_empty() {
        return Err(format!(
            "line {number}: `lines` must list at least one line"
        ));
    }
    let mut lines = Vec::new();
    for token in inner.split(',') {
        let token = token.trim();
        let parsed: u32 = token
            .parse()
            .map_err(|_| format!("line {number}: `{token}` is not a line number"))?;
        if parsed == 0 {
            return Err(format!("line {number}: line numbers start at 1"));
        }
        if lines.contains(&parsed) {
            return Err(format!("line {number}: line {parsed} is listed twice"));
        }
        lines.push(parsed);
    }
    Ok(lines)
}

/// The manifest addresses files the way the gate reports them: relative to
/// the repository, forward slashes, never escaping upwards.
fn check_path(path: &str, number: usize) -> Result<(), String> {
    let complaint = |problem: &str| Err(format!("line {number}: `file` {problem}"));
    if path.trim().is_empty() {
        return complaint("is empty");
    }
    if path.contains('\\') {
        return complaint("must use forward slashes, on every platform");
    }
    if path.starts_with('/') || path.contains(':') {
        return complaint("must be repository-relative, not absolute");
    }
    if path.split('/').any(|part| part == "..") {
        return complaint("must not walk out of the repository");
    }
    Ok(())
}

/// Read the uncovered lines out of a parsed `llvm-cov` JSON export.
///
/// # Errors
///
/// Returns a message when the document is not an `llvm-cov` export, when
/// it carries no data, when any region is malformed, or when the export's
/// own summary contradicts what this reader made of its regions. A gate
/// that cannot read its report has failed; it never passes vacuously.
pub fn measure(document: &Value, root: &str) -> Result<Measured, String> {
    let data = document
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "no `data` array in the llvm-cov export".to_string())?;
    if data.is_empty() {
        return Err("the llvm-cov export carries no coverage data".to_string());
    }

    // The measured set, and the subset the export itself calls fully
    // covered — the input to the cross-check at the bottom.
    let mut files: Vec<String> = Vec::new();
    let mut complete: BTreeSet<String> = BTreeSet::new();
    for entry in data {
        let listed = entry
            .get("files")
            .and_then(Value::as_array)
            .ok_or_else(|| "no `files` array in the llvm-cov export".to_string())?;
        for file in listed {
            let name = file
                .get("filename")
                .and_then(Value::as_str)
                .ok_or_else(|| "a file with no `filename` in the llvm-cov export".to_string())?;
            let relative = relative_to(root, name);
            if fully_covered(file) {
                complete.insert(relative.clone());
            }
            files.push(relative);
        }
    }
    if files.is_empty() {
        return Err("the llvm-cov export measured no files".to_string());
    }
    files.sort();
    files.dedup();

    // Keyed by the measured set, so a region naming anything else finds no
    // entry and is skipped rather than judged.
    let mut spans: BTreeMap<&str, (BTreeSet<u32>, BTreeSet<u32>)> = files
        .iter()
        .map(|file| (file.as_str(), (BTreeSet::new(), BTreeSet::new())))
        .collect();
    for entry in data {
        let functions = entry
            .get("functions")
            .and_then(Value::as_array)
            .ok_or_else(|| "no `functions` array in the llvm-cov export".to_string())?;
        for function in functions {
            let filenames = function
                .get("filenames")
                .and_then(Value::as_array)
                .ok_or_else(|| "a function with no `filenames` array".to_string())?;
            let regions = function
                .get("regions")
                .and_then(Value::as_array)
                .ok_or_else(|| "a function with no `regions` array".to_string())?;
            for region in regions {
                let region = Region::read(region)?;
                let path = filenames
                    .get(region.file)
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        format!(
                            "a region names file {} of a record that lists {}",
                            region.file,
                            filenames.len()
                        )
                    })?;
                let Some((zero, hit)) = spans.get_mut(relative_to(root, path).as_str()) else {
                    continue;
                };
                if region.covered {
                    hit.extend(region.span());
                } else {
                    zero.extend(region.span());
                }
            }
        }
    }

    // Sorted by construction: the file map and the line sets are ordered.
    let mut uncovered: Vec<Site> = Vec::new();
    for (file, (zero, hit)) in &spans {
        for line in zero {
            if hit.contains(line) {
                continue;
            }
            uncovered.push(Site {
                file: (*file).to_string(),
                line: *line,
            });
        }
    }

    // Cross-check. The export's summary and this reader may legitimately
    // disagree on how many lines are missed — the summary counts a line
    // once per function that maps it — but a file the summary calls
    // complete cannot hold an uncovered line unless this reader is
    // misreading the export.
    for site in &uncovered {
        if complete.contains(&site.file) {
            return Err(format!(
                "{site}: the export's own summary reports every line of this file covered, so this reader is misreading the export — its shape has changed"
            ));
        }
    }

    Ok(Measured { uncovered, files })
}

/// One region of an `llvm-cov` export, reduced to what the rule needs.
struct Region {
    start: u32,
    end: u32,
    covered: bool,
    file: usize,
}

impl Region {
    /// `[line_start, column_start, line_end, column_end, execution_count,
    /// file_id, …]`.
    fn read(value: &Value) -> Result<Self, String> {
        let fields = value
            .as_array()
            .ok_or_else(|| "a region that is not an array".to_string())?;
        let integer = |index: usize| match fields.get(index) {
            Some(Value::Number(number)) => Ok(*number),
            _ => Err(format!("a region with no field {index}")),
        };
        let line = |index: usize| {
            u32::try_from(integer(index)?)
                .map_err(|_| format!("a region with an out-of-range field {index}"))
        };
        let start = line(0)?;
        let end = line(2)?;
        let file = usize::try_from(integer(5)?)
            .map_err(|_| "a region with an out-of-range file index".to_string())?;
        // Execution counts are unsigned 64-bit and can exceed `i64`, which
        // the JSON reader then hands back as a float; either way the rule
        // only asks whether the region ran at all.
        let covered = match fields.get(4) {
            Some(Value::Number(count)) => *count > 0,
            Some(Value::Float(count)) => *count > 0.0,
            _ => return Err("a region with no execution count".to_string()),
        };
        if end < start {
            return Err(format!("a region that ends (line {end}) before it starts"));
        }
        if end - start > MAX_REGION_LINES {
            return Err(format!(
                "a region spanning lines {start} to {end}, which no source file has"
            ));
        }
        Ok(Self {
            start,
            end,
            covered,
            file,
        })
    }

    fn span(&self) -> core::ops::RangeInclusive<u32> {
        self.start..=self.end
    }
}

/// Whether the export's own summary reports every line of this file
/// covered. A missing or unexpected summary answers no, which costs only
/// the cross-check above.
fn fully_covered(file: &Value) -> bool {
    let lines = file.get("summary").and_then(|summary| summary.get("lines"));
    let count = lines.and_then(|lines| lines.get("count"));
    let covered = lines.and_then(|lines| lines.get("covered"));
    matches!(
        (count, covered),
        (Some(Value::Number(count)), Some(Value::Number(covered))) if count == covered
    )
}

/// Hold the report against the manifest, in both directions.
#[must_use]
pub fn compare(measured: &Measured, exemptions: &[Exemption]) -> Outcome {
    let exempt: Vec<Site> = exemptions
        .iter()
        .flat_map(|exemption| {
            exemption.lines.iter().map(|line| Site {
                file: exemption.file.clone(),
                line: *line,
            })
        })
        .collect();

    let mut gaps: Vec<Site> = measured
        .uncovered
        .iter()
        .filter(|site| !exempt.contains(site))
        .cloned()
        .collect();
    gaps.sort();

    let mut stale: Vec<Stale> = Vec::new();
    for exemption in exemptions {
        for line in &exemption.lines {
            let site = Site {
                file: exemption.file.clone(),
                line: *line,
            };
            if measured.uncovered.contains(&site) {
                continue;
            }
            let kind = if measured.files.contains(&site.file) {
                StaleKind::NowCovered
            } else {
                StaleKind::FileAbsent
            };
            stale.push(Stale {
                site,
                reason: exemption.reason.clone(),
                kind,
            });
        }
    }
    stale.sort_by(|left, right| left.site.cmp(&right.site));

    Outcome {
        gaps,
        stale,
        exempt_lines: exempt.len(),
        measured_files: measured.files.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = concat!(
        "# a comment\n\n",
        "[[exempt]]\n",
        "file = \"crates/a/src/lib.rs\"\n",
        "lines = [10, 12]\n",
        "reason = \"aborts before the counters are written\"\n\n",
        "[[exempt]]\n",
        "file = \"crates/b/src/lib.rs\"\n",
        "lines = [7]\n",
        "reason = \"guarded by a debug assertion on the same condition\"\n",
    );

    #[test]
    fn a_well_formed_manifest_parses_into_its_entries() {
        let entries = parse_manifest(GOOD).expect("the manifest parses");
        assert_eq!(
            entries,
            vec![
                Exemption {
                    file: "crates/a/src/lib.rs".to_string(),
                    lines: vec![10, 12],
                    reason: "aborts before the counters are written".to_string(),
                },
                Exemption {
                    file: "crates/b/src/lib.rs".to_string(),
                    lines: vec![7],
                    reason: "guarded by a debug assertion on the same condition".to_string(),
                },
            ]
        );
    }

    #[test]
    fn an_empty_manifest_exempts_nothing() {
        // Legal, and the strictest gate there is: everything must be
        // covered.
        assert_eq!(parse_manifest("# nothing yet\n"), Ok(Vec::new()));
        assert_eq!(parse_manifest(""), Ok(Vec::new()));
    }

    #[test]
    fn the_repositorys_own_manifest_parses_and_names_real_files() {
        // The committed file, read from the tree: a manifest that does not
        // parse, or that points at a file which no longer exists, would
        // otherwise only surface in CI's coverage job.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let text = std::fs::read_to_string(root.join(MANIFEST)).expect("the manifest is readable");
        let entries = parse_manifest(&text).expect("the manifest parses");
        assert!(!entries.is_empty(), "the manifest exempts nothing");
        for entry in &entries {
            assert!(
                root.join(&entry.file).is_file(),
                "`{}` names no file in the tree",
                entry.file
            );
            assert!(!entry.reason.trim().is_empty(), "{entry:?}");
        }
    }

    #[test]
    fn syntax_outside_the_accepted_subset_is_named_with_its_line() {
        for (text, expected) in [
            ("file = \"a.rs\"\n", "before the first"),
            ("[exempt]\n", "expected `[[exempt]]`"),
            ("[[exempt]]\nnonsense\n", "expected `key = value`"),
            ("[[exempt]]\nwho = \"x\"\n", "unknown key `who`"),
            (
                "[[exempt]]\nfile = \"a.rs\"\nfile = \"b.rs\"\n",
                "`file` is set twice",
            ),
            ("[[exempt]]\nfile = a.rs\n", "expected a double-quoted"),
            ("[[exempt]]\nreason = why\n", "expected a double-quoted"),
            ("[[exempt]]\nfile = \"a.rs\n", "unterminated string"),
            ("[[exempt]]\nfile = \"a.rs\" # note\n", "unexpected text"),
            ("[[exempt]]\nreason = \"  \"\n", "`reason` is empty"),
            ("[[exempt]]\nfile = \"\"\n", "`file` is empty"),
            ("[[exempt]]\nfile = \"a\\\\b.rs\"\n", "forward slashes"),
            ("[[exempt]]\nfile = \"/a.rs\"\n", "repository-relative"),
            ("[[exempt]]\nfile = \"C:/a.rs\"\n", "repository-relative"),
            ("[[exempt]]\nfile = \"../a.rs\"\n", "walk out"),
            ("[[exempt]]\nlines = 7\n", "one array on one line"),
            ("[[exempt]]\nlines = []\n", "at least one line"),
            ("[[exempt]]\nlines = [7, x]\n", "`x` is not a line number"),
            ("[[exempt]]\nlines = [0]\n", "line numbers start at 1"),
            ("[[exempt]]\nlines = [7, 7]\n", "line 7 is listed twice"),
        ] {
            let message = parse_manifest(text).expect_err("must be rejected");
            assert!(
                message.contains(expected),
                "expected {expected:?} in {message:?} for {text:?}"
            );
        }
    }

    #[test]
    fn an_entry_missing_any_key_is_rejected_against_its_own_header() {
        for (text, key) in [
            ("\n[[exempt]]\nlines = [1]\nreason = \"why\"\n", "file"),
            ("\n[[exempt]]\nfile = \"a.rs\"\nreason = \"why\"\n", "lines"),
            ("\n[[exempt]]\nfile = \"a.rs\"\nlines = [1]\n", "reason"),
            // Closed by the next header rather than by the end of file:
            // the same complaint, still against line 2.
            ("\n[[exempt]]\nfile = \"a.rs\"\n[[exempt]]\n", "lines"),
        ] {
            let message = parse_manifest(text).expect_err("must be rejected");
            assert!(message.contains(key), "{message}");
            assert!(message.starts_with("line 2:"), "{message}");
        }
    }

    #[test]
    fn the_same_line_cannot_be_exempted_twice() {
        // Two entries may name one file — their reasons differ — but never
        // the same line, which would hide one reason behind the other.
        let text = concat!(
            "[[exempt]]\nfile = \"a.rs\"\nlines = [1, 2]\nreason = \"first\"\n",
            "[[exempt]]\nfile = \"a.rs\"\nlines = [2]\nreason = \"second\"\n",
        );
        let message = parse_manifest(text).expect_err("must be rejected");
        assert!(message.contains("a.rs:2 is exempted twice"), "{message}");

        let fine = concat!(
            "[[exempt]]\nfile = \"a.rs\"\nlines = [1, 2]\nreason = \"first\"\n",
            "[[exempt]]\nfile = \"a.rs\"\nlines = [3]\nreason = \"second\"\n",
        );
        assert_eq!(
            parse_manifest(fine).map(|entries| entries.len()),
            Ok(2),
            "one file may hold two entries"
        );
    }

    #[test]
    fn export_paths_become_repository_relative() {
        assert_eq!(
            relative_to(
                "E:\\GithubProjects\\renew",
                "E:\\GithubProjects\\renew\\crates\\core\\memory\\src\\arena.rs"
            ),
            "crates/core/memory/src/arena.rs"
        );
        // Drive-letter case differs between toolchains; the tail keeps its
        // own case, and a trailing separator on the root changes nothing.
        assert_eq!(
            relative_to(
                "e:/GithubProjects/renew/",
                "E:/GithubProjects/renew/Crates/A.rs"
            ),
            "Crates/A.rs"
        );
        assert_eq!(
            relative_to("/home/runner/renew", "/home/runner/renew/crates/a.rs"),
            "crates/a.rs"
        );
        // Outside the root: normalized, but named in full rather than
        // quietly becoming something an entry could match.
        assert_eq!(
            relative_to("/w", "C:\\Users\\x\\.cargo\\registry\\src\\dep.rs"),
            "C:/Users/x/.cargo/registry/src/dep.rs"
        );
        // The root itself, and a sibling that merely shares its prefix.
        assert_eq!(relative_to("/w", "/w"), "/w");
        assert_eq!(relative_to("/w", "/works/a.rs"), "/works/a.rs");
        // A multi-byte tail must not be sliced apart.
        assert_eq!(relative_to("/w", "/w/çağdaş.rs"), "çağdaş.rs");
    }

    /// An export of one file whose regions are given as
    /// `(line_start, line_end, execution_count)`, optionally carrying the
    /// summary `(count, covered)`.
    fn export(regions: &[(u32, u32, i64)], summary: Option<(i64, i64)>) -> String {
        let spans: Vec<String> = regions
            .iter()
            .map(|(start, end, count)| format!("[{start},1,{end},9,{count},0,0,0]"))
            .collect();
        let summary = summary.map_or_else(String::new, |(count, covered)| {
            format!(",\"summary\":{{\"lines\":{{\"count\":{count},\"covered\":{covered}}}}}")
        });
        format!(
            concat!(
                "{{\"data\":[{{\"files\":[{{\"filename\":\"/w/crates/a.rs\"{}}}],",
                "\"functions\":[{{\"filenames\":[\"/w/crates/a.rs\"],\"regions\":[{}]}}]}}]}}"
            ),
            summary,
            spans.join(",")
        )
    }

    fn measured(text: &str) -> Result<Measured, String> {
        let document = crate::json::parse(text).expect("the fixture is valid JSON");
        measure(&document, "/w")
    }

    #[test]
    fn a_line_is_uncovered_only_when_no_region_covering_it_ran() {
        // Line 10 is spanned by a zero region and nothing else; line 11 by
        // a zero region and a covered one, which wins; lines 12 and 13 come
        // from the tail of that covered multi-line region.
        let text = export(&[(10, 11, 0), (11, 13, 4)], Some((4, 3)));
        let measured = measured(&text).expect("the export reads");
        assert_eq!(
            measured.uncovered,
            vec![Site {
                file: "crates/a.rs".to_string(),
                line: 10,
            }]
        );
        assert_eq!(measured.files, vec!["crates/a.rs".to_string()]);
    }

    #[test]
    fn uncovered_lines_come_out_sorted_and_deduplicated() {
        let text = export(&[(30, 30, 0), (10, 11, 0), (30, 30, 0)], None);
        let measured = measured(&text).expect("the export reads");
        let lines: Vec<u32> = measured.uncovered.iter().map(|site| site.line).collect();
        assert_eq!(lines, vec![10, 11, 30]);
    }

    #[test]
    fn regions_naming_unmeasured_files_are_left_alone() {
        // The half `--ignore-filename-regex` removed: function records
        // still name those files, and judging them would resurrect files
        // the collection deliberately dropped.
        let text = concat!(
            "{\"data\":[{\"files\":[{\"filename\":\"/w/crates/a.rs\"}],",
            "\"functions\":[{\"filenames\":[\"/w/crates/a.rs\",\"/w/tests/t.rs\"],",
            "\"regions\":[[1,1,1,9,1,0,0,0],[5,1,5,9,0,1,0,0]]}]}]}",
        );
        let measured = measured(text).expect("the export reads");
        assert_eq!(measured.uncovered, Vec::new());
        assert_eq!(measured.files, vec!["crates/a.rs".to_string()]);
    }

    #[test]
    fn an_export_that_cannot_be_read_fails_loudly() {
        for (text, expected) in [
            ("{}", "no `data` array"),
            ("{\"data\":[]}", "no coverage data"),
            ("{\"data\":[{}]}", "no `files` array"),
            ("{\"data\":[{\"files\":[{}]}]}", "no `filename`"),
            ("{\"data\":[{\"files\":[]}]}", "measured no files"),
            (
                "{\"data\":[{\"files\":[{\"filename\":\"/w/a.rs\"}]}]}",
                "no `functions` array",
            ),
            (
                "{\"data\":[{\"files\":[{\"filename\":\"/w/a.rs\"}],\"functions\":[{}]}]}",
                "no `filenames` array",
            ),
            (
                concat!(
                    "{\"data\":[{\"files\":[{\"filename\":\"/w/a.rs\"}],",
                    "\"functions\":[{\"filenames\":[]}]}]}"
                ),
                "no `regions` array",
            ),
        ] {
            let error = measured(text).expect_err("must be rejected");
            assert!(error.contains(expected), "expected {expected:?} in {error}");
        }
    }

    /// An export whose single region is the given JSON array.
    fn with_region(region: &str) -> String {
        format!(
            concat!(
                "{{\"data\":[{{\"files\":[{{\"filename\":\"/w/a.rs\"}}],",
                "\"functions\":[{{\"filenames\":[\"/w/a.rs\"],\"regions\":[{region}]}}]}}]}}"
            ),
            region = region
        )
    }

    #[test]
    fn a_malformed_region_fails_loudly() {
        for (region, expected) in [
            ("7", "not an array"),
            ("[1,1,1,9,0]", "no field 5"),
            ("[1,1,1,9,\"x\",0,0,0]", "no execution count"),
            ("[-1,1,1,9,0,0,0,0]", "out-of-range field 0"),
            ("[1.5,1,1,9,0,0,0,0]", "no field 0"),
            ("[1,1,1,9,0,-2,0,0]", "out-of-range file index"),
            ("[9,1,4,9,0,0,0,0]", "ends (line 4) before it starts"),
            ("[1,1,200000,9,0,0,0,0]", "which no source file has"),
            ("[1,1,1,9,0,3,0,0]", "names file 3 of a record that lists 1"),
        ] {
            let error = measured(&with_region(region)).expect_err("must be rejected");
            assert!(error.contains(expected), "expected {expected:?} in {error}");
        }
    }

    #[test]
    fn a_huge_execution_count_still_reads_as_covered() {
        // Counts are unsigned 64-bit; past `i64` the JSON reader hands back
        // a float, and the region still ran.
        let text = with_region("[1,1,1,9,18446744073709551615,0,0,0]");
        assert_eq!(measured(&text).map(|read| read.uncovered), Ok(Vec::new()));
    }

    #[test]
    fn a_reader_that_disagrees_with_the_exports_own_summary_refuses() {
        // The summary calls the file complete while the regions say line 10
        // never ran: the shape has changed under the reader, and a gate
        // that guessed here would be worse than one that stops.
        let text = export(&[(10, 10, 0)], Some((5, 5)));
        let error = measured(&text).expect_err("the disagreement must surface");
        assert!(error.contains("misreading the export"), "{error}");
    }

    fn site(file: &str, line: u32) -> Site {
        Site {
            file: file.to_string(),
            line,
        }
    }

    fn exemption(file: &str, lines: &[u32]) -> Exemption {
        Exemption {
            file: file.to_string(),
            lines: lines.to_vec(),
            reason: "documented".to_string(),
        }
    }

    #[test]
    fn a_report_matching_the_manifest_passes() {
        let measured = Measured {
            uncovered: vec![site("a.rs", 5), site("a.rs", 9)],
            files: vec!["a.rs".to_string(), "b.rs".to_string()],
        };
        let outcome = compare(&measured, &[exemption("a.rs", &[5, 9])]);
        assert!(outcome.passes());
        assert_eq!(outcome.findings(), 0);
        assert_eq!(outcome.exempt_lines, 2);
        assert_eq!(outcome.measured_files, 2);
    }

    #[test]
    fn an_uncovered_line_with_no_entry_is_a_gap() {
        let measured = Measured {
            uncovered: vec![site("a.rs", 5), site("b.rs", 3)],
            files: vec!["a.rs".to_string(), "b.rs".to_string()],
        };
        let outcome = compare(&measured, &[exemption("a.rs", &[5])]);
        assert!(!outcome.passes());
        assert_eq!(outcome.gaps, vec![site("b.rs", 3)]);
        assert_eq!(outcome.stale, Vec::new());
    }

    #[test]
    fn an_entry_whose_line_is_covered_now_is_stale() {
        let measured = Measured {
            uncovered: Vec::new(),
            files: vec!["a.rs".to_string()],
        };
        let outcome = compare(&measured, &[exemption("a.rs", &[9, 5])]);
        assert!(!outcome.passes());
        assert_eq!(outcome.gaps, Vec::new());
        assert_eq!(
            outcome.stale,
            vec![
                Stale {
                    site: site("a.rs", 5),
                    reason: "documented".to_string(),
                    kind: StaleKind::NowCovered,
                },
                Stale {
                    site: site("a.rs", 9),
                    reason: "documented".to_string(),
                    kind: StaleKind::NowCovered,
                },
            ]
        );
    }

    #[test]
    fn an_entry_naming_a_file_the_report_never_measured_is_stale_too() {
        // Renamed, deleted, or filtered out of the collection: whichever it
        // is, the entry now protects nothing and has to go.
        let measured = Measured {
            uncovered: Vec::new(),
            files: vec!["a.rs".to_string()],
        };
        let outcome = compare(&measured, &[exemption("gone.rs", &[5])]);
        assert_eq!(outcome.stale.len(), 1);
        assert_eq!(
            outcome.stale.first().map(|stale| stale.kind),
            Some(StaleKind::FileAbsent)
        );
        assert_eq!(outcome.findings(), 1);
    }

    #[test]
    fn both_directions_can_fail_at_once() {
        let measured = Measured {
            uncovered: vec![site("a.rs", 7)],
            files: vec!["a.rs".to_string()],
        };
        let outcome = compare(&measured, &[exemption("a.rs", &[5])]);
        assert_eq!(outcome.gaps, vec![site("a.rs", 7)]);
        assert_eq!(outcome.stale.len(), 1);
        assert_eq!(outcome.findings(), 2);
    }

    #[test]
    fn stale_kinds_carry_a_tag_and_an_instruction() {
        assert_eq!(StaleKind::NowCovered.label(), "now-covered");
        assert_eq!(StaleKind::FileAbsent.label(), "file-absent");
        assert!(StaleKind::NowCovered.explanation().contains("delete"));
        assert!(StaleKind::FileAbsent.explanation().contains("delete"));
    }

    #[test]
    fn a_site_prints_as_file_and_line() {
        assert_eq!(site("crates/a.rs", 42).to_string(), "crates/a.rs:42");
    }
}
