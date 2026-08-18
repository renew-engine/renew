//! Argument parsing for the fixed subcommand set.

use core::fmt;

/// The subcommands the binary accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Configure,
    Build,
    Test,
    Bench,
    Run,
    Lint,
    Check,
    Coverage,
    Modules,
    AssetPack,
    AssetInspect,
    Doctor,
    Record,
    Replay,
    Determinism,
    UiCompile,
}

impl Command {
    /// Every subcommand, in the order `usage` lists them.
    pub const ALL: [Self; 16] = [
        Self::Configure,
        Self::Build,
        Self::Test,
        Self::Bench,
        Self::Run,
        Self::Record,
        Self::Replay,
        Self::Lint,
        Self::Check,
        Self::Coverage,
        Self::Modules,
        Self::AssetPack,
        Self::AssetInspect,
        Self::UiCompile,
        Self::Determinism,
        Self::Doctor,
    ];

    /// The name the subcommand is invoked by.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Configure => "configure",
            Self::Build => "build",
            Self::Test => "test",
            Self::Bench => "bench",
            Self::Run => "run",
            Self::Lint => "lint",
            Self::Check => "check",
            Self::Coverage => "coverage",
            Self::Modules => "modules",
            Self::AssetPack => "asset-pack",
            Self::AssetInspect => "asset-inspect",
            Self::Doctor => "doctor",
            Self::Record => "record",
            Self::Replay => "replay",
            Self::Determinism => "determinism",
            Self::UiCompile => "ui-compile",
        }
    }

    /// Whether the subcommand names a sample and forwards the rest of
    /// the line to it. These are the subcommands whose flags must come
    /// *before* the sample name.
    #[must_use]
    pub const fn takes_sample(self) -> bool {
        matches!(self, Self::Run | Self::Record | Self::Replay)
    }

    /// The sample-side flag this subcommand translates to, and the
    /// `renew` flag whose value fills it.
    ///
    /// The pairing is a convention the tool cannot verify — a sample is
    /// free not to honour it — which is why a rejection from the child
    /// is rewritten to name the flag the caller actually typed.
    #[must_use]
    pub const fn trace_flags(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Record => Some(("--output", "--record-trace")),
            Self::Replay => Some(("--input", "--replay-trace")),
            _ => None,
        }
    }

    /// One-line description shown in the usage text.
    #[must_use]
    pub fn summary(self) -> &'static str {
        match self {
            Self::Configure => "verify the toolchain and cargo are present and sane",
            Self::Build => "build the workspace",
            Self::Test => "run the workspace test suite",
            Self::Bench => "run the workspace benchmarks",
            Self::Run => "build and run a workspace sample",
            Self::Lint => "check formatting, then run clippy with warnings denied",
            Self::Check => "verify workspace crate manifests and dependencies",
            Self::Coverage => "hold a coverage report against the exemption manifest",
            Self::Modules => "list every module with its maturity, from the manifests",
            Self::AssetPack => "build an asset pack from a directory of files",
            Self::AssetInspect => "list an asset pack's entries, optionally verifying them",
            Self::Doctor => "check the development environment",
            Self::Record => "run a sample, writing the input it saw to a file",
            Self::Replay => "run a sample from a recorded input file",
            Self::Determinism => {
                "emit this target's simulation digests, or compare several targets'"
            }
            Self::UiCompile => "compile a text document into the binary form the engine loads",
        }
    }

    /// Look a subcommand up by its invocation name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|command| command.name() == name)
    }
}

/// A successfully parsed invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub command: Command,
    pub json: bool,
    /// Bench only (parse enforces): run each benchmark once, without
    /// statistics — the fast run-proof mode CI's bench stage uses.
    pub smoke: bool,
    /// Coverage only (parse enforces, and requires): the `llvm-cov` JSON
    /// export to read.
    pub report: Option<String>,
    /// Run, record and replay (parse enforces, and requires): the sample
    /// to start, named by its binary.
    pub sample: Option<String>,
    /// Run, record and replay: the sample's own command line, verbatim.
    pub sample_args: Vec<String>,
    /// Record and replay only (parse enforces, and requires): the trace
    /// file to write, or to read. One field for both because exactly one
    /// subcommand can be in play, and two would let a caller construct an
    /// invocation naming both.
    pub trace: Option<String>,
    /// Both asset subcommands (parse enforces, and requires): the pack
    /// file to write, or to read.
    pub pack: Option<String>,
    /// `asset-pack` and `ui-compile` (parse enforces, and requires):
    /// the directory whose files become the pack's entries, or the
    /// text document to compile.
    pub from: Option<String>,
    /// `ui-compile` only (parse enforces, and requires): where the
    /// compiled document is written.
    pub out: Option<String>,
    /// `asset-inspect` only (parse enforces): also check every payload
    /// against its recorded digest. Off by default because it reads every
    /// byte, where listing reads only the table.
    pub verify: bool,
    /// `determinism` only (parse enforces): the target reports to hold
    /// against each other.
    pub compare: Vec<String>,
    /// `determinism` only (parse enforces): where to write this target's
    /// report. Exactly one of this and [`Invocation::compare`] is set —
    /// the two modes are one subcommand because they are two halves of
    /// one claim, and parsing refuses both together and neither.
    pub emit: Option<String>,
    /// `determinism --emit` only: the target triple the pinned runs are
    /// built and executed for. Absent means the host, which is what
    /// every desktop leg uses.
    ///
    /// **The leg's identity comes from this when it is given**, because
    /// the emitting tool runs on the host while the runs it measures do
    /// not: a leg that read its own `env::consts` would label an
    /// Android run as the desktop that launched it. What the triple
    /// cannot check is that the runs truly executed there — that is the
    /// runner's job, and the lane prints the device's own architecture
    /// beside the leg so the two can be read together.
    pub target: Option<String>,
    /// Run, record and replay: cargo features to build the sample with,
    /// each occurrence kept.
    ///
    /// **Cargo's own vocabulary, showing through deliberately.** A
    /// sample's optional capabilities are cargo features, and the one
    /// windowed sample cannot be started at all without naming one — so
    /// the alternative to letting the word through was a per-sample table
    /// mapping invented names onto features, which the sample list is
    /// specifically built to avoid needing (samples are discovered, never
    /// written down).
    ///
    /// Repeating accumulates, on the same reasoning as
    /// [`Invocation::compare`]: two occurrences mean the union, and
    /// keeping the last is how a caller silently loses one.
    pub features: Vec<String>,
}

/// What parsing decided: run a subcommand, or show usage on request.
/// Help carries the `--json` flag so usage can honor the output contract.
///
/// **The size gap between the variants is deliberate, not overlooked.**
/// `Run` carries the whole parsed command line and `Help` carries one
/// bool, which is what the lint is for — an enum whose small variant is
/// common wastes the difference on every value. Exactly one `Parsed`
/// exists per process: it is built by `parse`, matched immediately, and
/// dropped. Boxing would trade a pointer chase and an allocation for
/// bytes that are never multiplied by anything, and would put a `Box::new`
/// in front of every construction in the tests, where the shape of the
/// value is the thing being read.
#[expect(
    clippy::large_enum_variant,
    reason = "one value per process, built and matched at once; boxing would cost more than the gap"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parsed {
    Run(Invocation),
    Help { json: bool },
}

/// Why parsing failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    NoCommand,
    UnknownCommand(String),
    UnexpectedArgument(String),
    /// An option that takes a value was given without one.
    MissingValue(&'static str),
    /// A subcommand was given without an option it requires.
    MissingOption {
        command: &'static str,
        option: &'static str,
    },
    /// A sample-taking subcommand was given without naming a sample.
    /// Carries the subcommand, so the message names what the caller
    /// typed rather than the first subcommand that ever needed one.
    MissingSample(&'static str),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCommand => write!(f, "no command given"),
            Self::UnknownCommand(name) => write!(f, "unknown command `{name}`"),
            Self::UnexpectedArgument(argument) => {
                write!(f, "unexpected argument `{argument}`")
            }
            Self::MissingValue(option) => write!(f, "`{option}` needs a value"),
            Self::MissingOption { command, option } => {
                write!(f, "`{command}` needs `{option} <path>`")
            }
            Self::MissingSample(command) => {
                write!(f, "`{command}` needs a sample to run")
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// Parse command-line arguments (excluding the program name).
///
/// Everything after `run <sample>` — or `record`'s or `replay`'s — is
/// the sample's own command line and is taken verbatim, so a flag this
/// binary also understands still reaches the sample. Flags meant for
/// `renew` itself therefore go *before* the sample name; putting
/// `--output` after it would forward the flag and produce an error
/// naming something the caller never typed. A single `--` may stand
/// between the two
/// halves for readers; it is the marker, not an argument, so only the
/// first one is dropped and a sample that wants a literal `--` gets it
/// by writing two.
///
/// # Errors
///
/// Returns a [`ParseError`] when no subcommand is given, the subcommand is
/// unknown, or an argument other than the known flags is present —
/// including `--smoke` with any subcommand other than `bench`, `--report`
/// with any subcommand other than `coverage`, `--report` without a path,
/// `coverage` without `--report`, a trace flag given to a subcommand that
/// does not answer to it (including `record --input` and `replay
/// --output`, each of which names the other's flag), `record` or `replay`
/// without one, and any sample-taking subcommand without a sample.
// The scanner is one long match over flag literals, and it is long
// because the flag set is. Splitting it would put half the flags in one
// place and half in another with nothing to say which half a new one
// belongs to — the linter's line count is measuring the size of the
// command line, not a structure problem, so the allowance is taken here
// with the reason rather than by raising the cap for the whole crate.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per flag; splitting the match would scatter the flag set"
)]
pub fn parse(arguments: &[String]) -> Result<Parsed, ParseError> {
    let mut command = None;
    let mut json = false;
    let mut smoke = false;
    let mut help = false;
    let mut report = None;
    let mut trace: Option<(&'static str, String)> = None;
    let mut pack: Option<String> = None;
    let mut from: Option<String> = None;
    let mut out: Option<String> = None;
    let mut verify = false;
    let mut compare: Vec<String> = Vec::new();
    let mut features: Vec<String> = Vec::new();
    let mut emit: Option<String> = None;
    let mut target: Option<String> = None;
    let mut sample: Option<String> = None;
    let mut sample_args: Vec<String> = Vec::new();
    // The separator, if it comes at all, comes immediately after the
    // sample name. Tracked rather than inferred from an empty tail, so
    // that a second `--` is an argument even when the first was consumed.
    let mut separator_due = false;
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        // Past the sample name nothing is this binary's business, so this
        // arm comes before every flag the binary knows.
        if sample.is_some() {
            let was_due = separator_due;
            separator_due = false;
            if was_due && argument == "--" {
                continue;
            }
            sample_args.push(argument.clone());
            continue;
        }
        match argument.as_str() {
            "--json" => json = true,
            "--smoke" => smoke = true,
            // Consumes its value even under `help`, so the path can never
            // be mistaken for a subcommand.
            "--report" => {
                let path = rest.next().ok_or(ParseError::MissingValue("--report"))?;
                report = Some(path.clone());
            }
            // Same reason as `--report`: the value is consumed here so a
            // path can never be mistaken for a subcommand or a sample.
            "--output" => {
                let path = rest.next().ok_or(ParseError::MissingValue("--output"))?;
                trace = Some(("--output", path.clone()));
            }
            "--input" => {
                let path = rest.next().ok_or(ParseError::MissingValue("--input"))?;
                trace = Some(("--input", path.clone()));
            }
            // The first repeatable value flag in this parser. Repeating
            // accumulates rather than overwriting, because the number of
            // reports IS the thing the comparison checks — a flag that
            // silently kept the last one would turn three targets into
            // one and report agreement.
            "--compare" => {
                let path = rest.next().ok_or(ParseError::MissingValue("--compare"))?;
                compare.push(path.clone());
            }
            // Repeatable for the same reason `--compare` is, and passed
            // to cargo verbatim: cargo already unions repeated
            // occurrences and already accepts comma- or space-separated
            // lists inside one, so splitting or joining here would be
            // this tool inventing a second grammar for a syntax that
            // already has one.
            "--features" => {
                let names = rest.next().ok_or(ParseError::MissingValue("--features"))?;
                features.push(names.clone());
            }
            // Its own flag rather than `--output`, which belongs to
            // `record` and means "write the input you saw here". This
            // writes a target's digests, and a reader who had to know the
            // subcommand to know what a flag meant would be right to
            // complain.
            "--emit" => {
                let path = rest.next().ok_or(ParseError::MissingValue("--emit"))?;
                emit = Some(path.clone());
            }
            // Passed through to every pinned run's cargo invocation, so
            // one code path produces every leg: what differs between a
            // desktop leg and a device one is where the binaries run,
            // not how they are measured.
            "--target" => {
                let triple = rest.next().ok_or(ParseError::MissingValue("--target"))?;
                target = Some(triple.clone());
            }
            // Same reason as `--report`: the value is consumed here so a
            // path can never be mistaken for a subcommand.
            "--pack" => {
                let path = rest.next().ok_or(ParseError::MissingValue("--pack"))?;
                pack = Some(path.clone());
            }
            "--out" => {
                let path = rest.next().ok_or(ParseError::MissingValue("--out"))?;
                out = Some(path.clone());
            }
            "--from" => {
                let path = rest.next().ok_or(ParseError::MissingValue("--from"))?;
                from = Some(path.clone());
            }
            "--verify" => verify = true,
            "help" | "--help" | "-h" => help = true,
            other => {
                if help {
                    // Help short-circuits; ignore anything after it.
                    continue;
                }
                if command.is_some_and(Command::takes_sample) {
                    // The first free argument names the sample; the rest
                    // of the line is the sample's, taken above.
                    sample = Some(other.to_string());
                    separator_due = true;
                    continue;
                }
                if command.is_some() {
                    return Err(ParseError::UnexpectedArgument(other.to_string()));
                }
                match Command::from_name(other) {
                    Some(found) => command = Some(found),
                    None => return Err(ParseError::UnknownCommand(other.to_string())),
                }
            }
        }
    }
    if help {
        return Ok(Parsed::Help { json });
    }
    check_determinism_ownership(command, emit.as_deref(), &compare, target.as_deref())?;
    check_combination(
        command,
        smoke,
        report.as_deref(),
        trace.as_ref().map(|(flag, _)| *flag),
        sample.as_deref(),
        &features,
    )?;
    check_file_combination(
        command,
        pack.as_deref(),
        from.as_deref(),
        out.as_deref(),
        verify,
    )?;
    check_determinism_mode(command, emit.as_deref(), &compare, target.as_deref())?;

    match command {
        Some(command) => Ok(Parsed::Run(Invocation {
            command,
            json,
            smoke,
            report,
            sample,
            sample_args,
            trace: trace.map(|(_, path)| path),
            pack,
            from,
            out,
            verify,
            compare,
            emit,
            target,
            features,
        })),
        None => Err(ParseError::NoCommand),
    }
}

/// `--emit`, `--compare` and `--target` belong to `determinism` and
/// nothing else.
///
/// Separate from [`check_combination`] for the reason its neighbour
/// already records: that function carries five parameters and reads as a
/// list of rules only while it does.
fn check_determinism_ownership(
    command: Option<Command>,
    emit: Option<&str>,
    compare: &[String],
    target: Option<&str>,
) -> Result<(), ParseError> {
    if command == Some(Command::Determinism) {
        return Ok(());
    }
    if emit.is_some() {
        return Err(ParseError::UnexpectedArgument("--emit".to_string()));
    }
    if !compare.is_empty() {
        return Err(ParseError::UnexpectedArgument("--compare".to_string()));
    }
    // `build --target` reads like cargo's flag of the same name and is
    // not one: nothing outside this subcommand passes it anywhere. Left
    // accepted, it would be a flag that looks like it cross-compiled.
    if target.is_some() {
        return Err(ParseError::UnexpectedArgument("--target".to_string()));
    }
    Ok(())
}

/// The determinism subcommand's requirement: exactly one mode.
///
/// Split from [`check_determinism_ownership`] and called after every
/// other ownership rule, because this module's ordering convention is
/// that "this flag is not yours" always precedes "you are missing one".
/// Together in one function, `determinism --smoke` reported the missing
/// mode instead of the stray flag, and `run --emit x` reported a missing
/// sample instead of a flag that is not `run`'s.
fn check_determinism_mode(
    command: Option<Command>,
    emit: Option<&str>,
    compare: &[String],
    target: Option<&str>,
) -> Result<(), ParseError> {
    if command != Some(Command::Determinism) {
        return Ok(());
    }
    // `--target` selects what to build and run; `--compare` builds and
    // runs nothing, reading reports that already exist and already say
    // where they ran. Accepting the pair silently is how somebody comes
    // to believe a comparison covered a target it never saw.
    if target.is_some() && !compare.is_empty() {
        return Err(ParseError::UnexpectedArgument(
            "--target with --compare: a comparison reads legs that already ran, and each \
             one names its own platform"
                .to_string(),
        ));
    }
    // Neither is a subcommand asked to do nothing; both is a subcommand
    // asked to do two things, and choosing one silently is how a lane
    // emits when it meant to compare.
    if emit.is_some() && !compare.is_empty() {
        return Err(ParseError::UnexpectedArgument(
            "--emit with --compare: one run either reports a target or holds several              against each other"
                .to_string(),
        ));
    }
    if emit.is_none() && compare.is_empty() {
        return Err(ParseError::MissingOption {
            command: "determinism",
            option: "--emit <path> or --compare <path>...",
        });
    }
    Ok(())
}

/// The asset subcommands' own flag rules.
///
/// Separate from [`check_combination`] rather than more parameters on
/// it: that function already carries five, and the four file flags
/// here would make the one thing it is good at — reading as a list of
/// rules — stop being true.
fn check_file_combination(
    command: Option<Command>,
    pack: Option<&str>,
    from: Option<&str>,
    out: Option<&str>,
    verify: bool,
) -> Result<(), ParseError> {
    let is_pack = command == Some(Command::AssetPack);
    let is_inspect = command == Some(Command::AssetInspect);
    let is_compile = command == Some(Command::UiCompile);

    // Stray flags first, matching the order the other rules use: a flag
    // on the wrong subcommand is as unexpected as any other argument.
    if pack.is_some() && !(is_pack || is_inspect) {
        return Err(ParseError::UnexpectedArgument("--pack".to_string()));
    }
    if from.is_some() && !(is_pack || is_compile) {
        return Err(ParseError::UnexpectedArgument("--from".to_string()));
    }
    if out.is_some() && !is_compile {
        return Err(ParseError::UnexpectedArgument("--out".to_string()));
    }
    if verify && !is_inspect {
        return Err(ParseError::UnexpectedArgument("--verify".to_string()));
    }

    // Then what each subcommand cannot work without. Both paths are the
    // whole input: guessing one would be worse than refusing.
    if (is_pack || is_inspect) && pack.is_none() {
        return Err(ParseError::MissingOption {
            command: if is_pack {
                "asset-pack"
            } else {
                "asset-inspect"
            },
            option: "--pack",
        });
    }
    if is_pack && from.is_none() {
        return Err(ParseError::MissingOption {
            command: "asset-pack",
            option: "--from",
        });
    }
    if is_compile && from.is_none() {
        return Err(ParseError::MissingOption {
            command: "ui-compile",
            option: "--from",
        });
    }
    if is_compile && out.is_none() {
        return Err(ParseError::MissingOption {
            command: "ui-compile",
            option: "--out",
        });
    }
    Ok(())
}

/// The rules that can only be decided once the whole line has been read:
/// which subcommand a flag belongs to, and which subcommands require one.
///
/// Split out of [`parse`] so the scan and the cross-argument rules can
/// each be read on their own — the scan says what was typed, this says
/// whether the combination means anything.
///
/// Order matters and is deliberate. Every "this flag is not yours" rule
/// comes before every "you are missing a flag" rule, so a caller who
/// typed a flag hears about the flag they typed rather than about a
/// different one they did not. `coverage --smoke` set that precedent.
fn check_combination(
    command: Option<Command>,
    smoke: bool,
    report: Option<&str>,
    trace: Option<&'static str>,
    sample: Option<&str>,
    features: &[String],
) -> Result<(), ParseError> {
    if !features.is_empty() && !command.is_some_and(Command::takes_sample) {
        // Features name how a *sample* is built. `renew build` builds the
        // whole workspace and `renew test` runs all of it, so a feature
        // there would have to mean something this tool has not decided —
        // refusing is what keeps the flag's meaning single.
        return Err(ParseError::UnexpectedArgument("--features".to_string()));
    }
    if smoke && command != Some(Command::Bench) {
        // The flag belongs to exactly one subcommand; anywhere else it is
        // as unexpected as any stray argument.
        return Err(ParseError::UnexpectedArgument("--smoke".to_string()));
    }
    if report.is_some() && command != Some(Command::Coverage) {
        return Err(ParseError::UnexpectedArgument("--report".to_string()));
    }
    // A trace flag the subcommand does not answer to, which covers three
    // cases at once: any subcommand that takes neither, no subcommand at
    // all, and each of record and replay handed the other's flag —
    // `record --input` names a real flag, but reading it as `--output`
    // would write over the file the caller meant to read.
    if let Some(given) = trace {
        let answers_to = command.and_then(Command::trace_flags);
        if answers_to.map(|(flag, _)| flag) != Some(given) {
            return Err(ParseError::UnexpectedArgument(given.to_string()));
        }
    }
    if command == Some(Command::Coverage) && report.is_none() {
        // The report is the whole input: coverage has nothing to read
        // without it, and guessing a path would be worse than refusing.
        return Err(ParseError::MissingOption {
            command: "coverage",
            option: "--report",
        });
    }
    // The path is the whole input for these two, exactly as `--report`
    // is for coverage: there is nothing to record to, or replay from.
    if let Some(named) = command
        && let Some((expected, _)) = named.trace_flags()
        && trace.is_none()
    {
        return Err(ParseError::MissingOption {
            command: named.name(),
            option: expected,
        });
    }
    if let Some(named) = command
        && named.takes_sample()
        && sample.is_none()
    {
        // Guessing a sample would be worse than refusing: which one runs
        // is the entire content of the command.
        return Err(ParseError::MissingSample(named.name()));
    }
    Ok(())
}

/// The usage text printed for `help` and for usage errors.
#[must_use]
pub fn usage() -> String {
    use core::fmt::Write as _;

    let mut text = String::from(concat!(
        "usage: renew <command> [options]\n",
        "       renew [options] run <sample> [--] [sample arguments...]\n",
        "       renew record --output <path> <sample> [--] [sample arguments...]\n",
        "       renew replay --input <path> <sample> [--] [sample arguments...]\n",
        "\ncommands:\n",
    ));
    for command in Command::ALL {
        let name = command.name();
        let summary = command.summary();
        let _ = writeln!(text, "  {name:<9}  {summary}");
    }
    text.push_str(concat!(
        "\noptions:\n",
        "  --json            emit one machine-readable JSON document on stdout\n",
        "  --report <path>   (coverage only, required) the llvm-cov JSON export to read\n",
        "  --smoke           (bench only) run each benchmark once, without statistics\n",
        "  --output <path>   (record only, required) the trace file to write\n",
        "  --input <path>    (replay only, required) the trace file to read\n",
        "  --pack <path>     (asset-pack, asset-inspect; required) the pack file\n",
        "  --from <path>     (asset-pack, ui-compile; required) the directory to\n",
        "                    pack, or the text document to compile\n",
        "  --out <path>      (ui-compile only, required) where the compiled document\n",
        "                    is written\n",
        "  --verify          (asset-inspect only) check each entry against its digest\n",
        "  --emit <path>     (determinism only) write this target's digests here\n",
        "  --compare <path>  (determinism only, repeatable) a target report to compare\n",
        "  --target <triple> (determinism --emit only) build and run the pinned\n",
        "                    simulations for this triple, through cargo's runner\n",
        "                    mechanism where one is configured\n",
        "  --features <list> (run, record, replay; repeatable) cargo features to build\n",
        "                    the sample with, e.g. `--features window` for a window\n",
        "  --help, -h        print this text; `renew help` does the same\n",
        "\nEverything after `run <sample>` goes to the sample untouched, including\n",
        "flags renew itself knows: `renew run hello_triangle --json` gives the sample\n",
        "`--json`, while `renew --json run hello_triangle` gives it to renew. One `--`\n",
        "after the sample name is an optional separator and is not passed on.\n",
        "\n`record` and `replay` are `run` with a trace file: their flag goes before\n",
        "the sample name for the same reason, and reaches the sample as\n",
        "`--record-trace <path>` or `--replay-trace <path>` at the front of its line.\n",
        "Recording and replaying are headless: a windowed replay is a live run\n",
        "wearing a replay's name. How a sample spells headless is the sample's own\n",
        "business — some take `--headless`, others are headless unless asked for a\n",
        "window — so its usage says which, and this tool assumes nothing.\n",
        "\n`--features` reaches cargo, not the sample. It builds the sample with those\n",
        "features on, which is how a sample's optional capabilities are named:\n",
        "`renew --features window run glide --window` builds the window in, then\n",
        "asks for it.\n",
    ));
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    /// The invocation a bare subcommand parses to.
    fn plain(command: Command) -> Invocation {
        Invocation {
            command,
            json: false,
            smoke: false,
            report: None,
            sample: None,
            sample_args: Vec::new(),
            trace: None,
            pack: None,
            from: None,
            out: None,
            verify: false,
            compare: Vec::new(),
            emit: None,
            target: None,
            features: Vec::new(),
        }
    }

    /// Every subcommand except the one a flag belongs to. Derived from
    /// `Command::ALL` rather than written out, so a subcommand added
    /// later cannot quietly escape the rejection tests below.
    fn all_except(owner: Command) -> impl Iterator<Item = Command> {
        Command::ALL.into_iter().filter(move |c| *c != owner)
    }

    /// What `run <sample>` with a given tail must parse to.
    fn running(sample: &str, sample_args: &[&str]) -> Parsed {
        Parsed::Run(Invocation {
            sample: Some(sample.to_string()),
            sample_args: sample_args.iter().map(ToString::to_string).collect(),
            ..plain(Command::Run)
        })
    }

    #[test]
    fn every_command_parses_by_name() {
        for command in Command::ALL {
            let name = command.name();
            // Four subcommands need more than their name: coverage takes
            // a required option, run takes the sample, and record and
            // replay take both. All still have to round-trip by name.
            let (line, expected) = match command {
                Command::Coverage => (
                    vec![name, "--report", "cov.json"],
                    Invocation {
                        report: Some("cov.json".to_string()),
                        ..plain(command)
                    },
                ),
                Command::AssetPack => (
                    vec![name, "--from", "assets", "--pack", "out.rpk"],
                    Invocation {
                        from: Some("assets".to_string()),
                        pack: Some("out.rpk".to_string()),
                        ..plain(command)
                    },
                ),
                Command::UiCompile => (
                    vec![name, "--from", "menu.ui", "--out", "menu.uib"],
                    Invocation {
                        from: Some("menu.ui".to_string()),
                        out: Some("menu.uib".to_string()),
                        ..plain(command)
                    },
                ),
                Command::AssetInspect => (
                    vec![name, "--pack", "out.rpk"],
                    Invocation {
                        pack: Some("out.rpk".to_string()),
                        ..plain(command)
                    },
                ),
                Command::Run => (
                    vec![name, "hello_triangle"],
                    Invocation {
                        sample: Some("hello_triangle".to_string()),
                        ..plain(command)
                    },
                ),
                // Emit rather than compare: both modes are covered by
                // their own tests below, and this one is about the name
                // round-tripping, so it takes the shorter line.
                Command::Determinism => (
                    vec![name, "--emit", "leg.json"],
                    Invocation {
                        emit: Some("leg.json".to_string()),
                        ..plain(command)
                    },
                ),
                Command::Record | Command::Replay => {
                    // Whichever flag this one owns, taken from the same
                    // table the parser consults, so the two cannot drift.
                    let (flag, _) = command
                        .trace_flags()
                        .expect("record and replay carry flags");
                    (
                        vec![name, flag, "walk.trace", "input_echo"],
                        Invocation {
                            sample: Some("input_echo".to_string()),
                            trace: Some("walk.trace".to_string()),
                            ..plain(command)
                        },
                    )
                }
                _ => (vec![name], plain(command)),
            };
            assert_eq!(
                parse(&arguments(&line)),
                Ok(Parsed::Run(expected)),
                "command `{name}` did not round-trip"
            );
        }
    }

    /// The two modes are one subcommand, and parsing is what keeps them
    /// from becoming three: neither is a run asked to do nothing, both is
    /// a run asked to do two things, and choosing one silently is how a
    /// lane emits when it meant to compare.
    #[test]
    fn determinism_takes_exactly_one_mode() {
        assert_eq!(
            parse(&arguments(&["determinism", "--emit", "leg.json"])),
            Ok(Parsed::Run(Invocation {
                emit: Some("leg.json".to_string()),
                ..plain(Command::Determinism)
            }))
        );

        // Repeating accumulates rather than overwrites: how many reports
        // there are IS what the comparison checks, and a flag that kept
        // only the last would turn three targets into one and report
        // agreement.
        assert_eq!(
            parse(&arguments(&[
                "determinism",
                "--compare",
                "a.json",
                "--compare",
                "b.json",
            ])),
            Ok(Parsed::Run(Invocation {
                compare: vec!["a.json".to_string(), "b.json".to_string()],
                ..plain(Command::Determinism)
            }))
        );

        let both = parse(&arguments(&[
            "determinism",
            "--emit",
            "leg.json",
            "--compare",
            "a.json",
        ]));
        assert!(
            matches!(both, Err(ParseError::UnexpectedArgument(_))),
            "{both:?}"
        );

        let neither = parse(&arguments(&["determinism"]));
        assert_eq!(
            neither,
            Err(ParseError::MissingOption {
                command: "determinism",
                option: "--emit <path> or --compare <path>...",
            })
        );
    }

    /// All three flags belong to `determinism` and to nothing else.
    /// Derived from `Command::ALL` like the other ownership tests here,
    /// so a subcommand added later cannot quietly escape the rejection.
    ///
    /// `--target` is the one worth stating twice: it is spelled exactly
    /// like cargo's flag, so `renew build --target aarch64-linux-android`
    /// is a plausible thing to type and nothing in this tool would have
    /// cross-compiled anything.
    #[test]
    fn the_determinism_flags_are_refused_on_every_other_subcommand() {
        for command in all_except(Command::Determinism) {
            for flag in ["--emit", "--compare", "--target"] {
                let line = vec![command.name(), flag, "leg.json"];
                let parsed = parse(&arguments(&line));
                let name = command.name();
                assert!(
                    matches!(&parsed, Err(ParseError::UnexpectedArgument(named)) if named == flag),
                    "`{name} {flag}` should be refused by name, got {parsed:?}"
                );
            }
        }
    }

    /// A flag that takes a path consumes it, so a path can never be read
    /// as a subcommand — and a flag given without one is named rather
    /// than silently ignored.
    #[test]
    fn the_determinism_flags_refuse_a_missing_value() {
        assert_eq!(
            parse(&arguments(&["determinism", "--emit"])),
            Err(ParseError::MissingValue("--emit"))
        );
        assert_eq!(
            parse(&arguments(&["determinism", "--compare"])),
            Err(ParseError::MissingValue("--compare"))
        );
        assert_eq!(
            parse(&arguments(&["determinism", "--target"])),
            Err(ParseError::MissingValue("--target"))
        );
    }

    /// The triple reaches the invocation, and only in the mode that can
    /// act on it.
    ///
    /// `--target` selects what to build and run. `--compare` builds and
    /// runs nothing — it reads reports that already exist, each naming
    /// the platform it ran on. Accepting the pair would leave a flag
    /// that appears to have widened a comparison and did not, which is
    /// the reading that turns an unexercised target into a green lane.
    #[test]
    fn a_target_triple_reaches_emit_and_is_refused_beside_compare() {
        assert_eq!(
            parse(&arguments(&[
                "determinism",
                "--emit",
                "leg.json",
                "--target",
                "x86_64-linux-android",
            ])),
            Ok(Parsed::Run(Invocation {
                emit: Some("leg.json".to_string()),
                target: Some("x86_64-linux-android".to_string()),
                ..plain(Command::Determinism)
            }))
        );

        let with_compare = parse(&arguments(&[
            "determinism",
            "--compare",
            "a.json",
            "--target",
            "x86_64-linux-android",
        ]));
        assert!(
            matches!(&with_compare, Err(ParseError::UnexpectedArgument(said)) if said.contains("--target") && said.contains("--compare")),
            "the refusal should name both flags, got {with_compare:?}"
        );
    }

    #[test]
    fn json_flag_parses_in_either_position() {
        let before = parse(&arguments(&["--json", "build"]));
        let after = parse(&arguments(&["build", "--json"]));
        let expected = Ok(Parsed::Run(Invocation {
            json: true,
            ..plain(Command::Build)
        }));
        assert_eq!(before, expected);
        assert_eq!(after, expected);
    }

    #[test]
    fn smoke_parses_with_bench_in_either_position_and_with_json() {
        let expected = Ok(Parsed::Run(Invocation {
            smoke: true,
            ..plain(Command::Bench)
        }));
        assert_eq!(parse(&arguments(&["bench", "--smoke"])), expected);
        assert_eq!(parse(&arguments(&["--smoke", "bench"])), expected);
        assert_eq!(
            parse(&arguments(&["bench", "--smoke", "--json"])),
            Ok(Parsed::Run(Invocation {
                json: true,
                smoke: true,
                ..plain(Command::Bench)
            }))
        );
    }

    #[test]
    fn smoke_with_any_other_subcommand_is_rejected() {
        for command in all_except(Command::Bench) {
            let name = command.name();
            assert_eq!(
                parse(&arguments(&[name, "--smoke"])),
                Err(ParseError::UnexpectedArgument("--smoke".to_string())),
                "`{name} --smoke` must be rejected"
            );
        }
        assert_eq!(
            parse(&arguments(&["--smoke"])),
            Err(ParseError::UnexpectedArgument("--smoke".to_string()))
        );
    }

    #[test]
    fn report_parses_with_coverage_in_either_position_and_with_json() {
        let expected = Ok(Parsed::Run(Invocation {
            report: Some("target/cov.json".to_string()),
            ..plain(Command::Coverage)
        }));
        assert_eq!(
            parse(&arguments(&["coverage", "--report", "target/cov.json"])),
            expected
        );
        assert_eq!(
            parse(&arguments(&["--report", "target/cov.json", "coverage"])),
            expected
        );
        assert_eq!(
            parse(&arguments(&["coverage", "--report", "cov.json", "--json"])),
            Ok(Parsed::Run(Invocation {
                json: true,
                report: Some("cov.json".to_string()),
                ..plain(Command::Coverage)
            }))
        );
    }

    #[test]
    fn report_with_any_other_subcommand_is_rejected() {
        for command in all_except(Command::Coverage) {
            let name = command.name();
            assert_eq!(
                parse(&arguments(&[name, "--report", "cov.json"])),
                Err(ParseError::UnexpectedArgument("--report".to_string())),
                "`{name} --report` must be rejected"
            );
        }
        assert_eq!(
            parse(&arguments(&["--report", "cov.json"])),
            Err(ParseError::UnexpectedArgument("--report".to_string()))
        );
    }

    #[test]
    fn coverage_without_a_report_is_rejected() {
        assert_eq!(
            parse(&arguments(&["coverage"])),
            Err(ParseError::MissingOption {
                command: "coverage",
                option: "--report",
            })
        );
    }

    #[test]
    fn a_report_flag_without_a_path_is_rejected() {
        assert_eq!(
            parse(&arguments(&["coverage", "--report"])),
            Err(ParseError::MissingValue("--report"))
        );
    }

    #[test]
    fn a_report_path_is_never_read_as_a_subcommand() {
        // `--report` consumes the token after it, so a path that happens to
        // spell a subcommand stays a path.
        assert_eq!(
            parse(&arguments(&["coverage", "--report", "build"])),
            Ok(Parsed::Run(Invocation {
                report: Some("build".to_string()),
                ..plain(Command::Coverage)
            }))
        );
    }

    /// Each of the two owns exactly one flag, and each must refuse the
    /// other's: they differ only in direction, so a swap that parsed
    /// would read the file the caller asked to write.
    #[test]
    fn record_and_replay_take_their_own_flag_and_refuse_the_others() {
        for command in [Command::Record, Command::Replay] {
            let name = command.name();
            let (mine, _) = command.trace_flags().expect("both carry a flag");
            assert_eq!(
                parse(&arguments(&[name, mine, "walk.trace", "input_echo"])),
                Ok(Parsed::Run(Invocation {
                    sample: Some("input_echo".to_string()),
                    trace: Some("walk.trace".to_string()),
                    ..plain(command)
                })),
                "`{name} {mine}` must parse"
            );
            // The flag before the subcommand name, as `--report` allows.
            assert_eq!(
                parse(&arguments(&[mine, "walk.trace", name, "input_echo"])),
                Ok(Parsed::Run(Invocation {
                    sample: Some("input_echo".to_string()),
                    trace: Some("walk.trace".to_string()),
                    ..plain(command)
                })),
                "`{mine} … {name}` must parse"
            );
            let (theirs, _) = all_except(command)
                .find_map(Command::trace_flags)
                .expect("the other subcommand carries the other flag");
            assert_eq!(
                parse(&arguments(&[name, theirs, "walk.trace", "input_echo"])),
                Err(ParseError::UnexpectedArgument(theirs.to_string())),
                "`{name} {theirs}` must be rejected"
            );
        }
    }

    /// The mirror of the `--smoke` and `--report` rejections: a trace
    /// flag anywhere it is not owned, derived from `ALL` so a subcommand
    /// added later cannot escape it.
    #[test]
    fn a_trace_flag_with_any_other_subcommand_is_rejected() {
        for owner in [Command::Record, Command::Replay] {
            let (flag, _) = owner.trace_flags().expect("both carry a flag");
            for command in all_except(owner) {
                let name = command.name();
                assert_eq!(
                    parse(&arguments(&[name, flag, "walk.trace"])),
                    Err(ParseError::UnexpectedArgument(flag.to_string())),
                    "`{name} {flag}` must be rejected"
                );
            }
            // And with no subcommand at all, rather than `NoCommand`.
            assert_eq!(
                parse(&arguments(&[flag, "walk.trace"])),
                Err(ParseError::UnexpectedArgument(flag.to_string()))
            );
        }
    }

    /// `--out` anywhere ui-compile is not: within the file-flag rules
    /// strays answer before missing flags, so even asset-pack hears
    /// about the `--out` it typed rather than the `--pack` it did not.
    /// Sample-taking subcommands answer from their own earlier rules
    /// first, which is why they are not looped here.
    #[test]
    fn the_out_flag_on_another_subcommand_is_rejected() {
        for command in [
            Command::Check,
            Command::Modules,
            Command::Lint,
            Command::AssetPack,
        ] {
            let name = command.name();
            assert_eq!(
                parse(&arguments(&[name, "--out", "menu.uib"])),
                Err(ParseError::UnexpectedArgument("--out".to_string())),
                "`{name} --out` must be rejected"
            );
        }
    }

    /// ui-compile without either of its two paths names the one that
    /// is missing — input first, matching the rule order.
    #[test]
    fn ui_compile_without_its_paths_is_rejected() {
        assert_eq!(
            parse(&arguments(&["ui-compile", "--out", "menu.uib"])),
            Err(ParseError::MissingOption {
                command: "ui-compile",
                option: "--from",
            })
        );
        assert_eq!(
            parse(&arguments(&["ui-compile", "--from", "menu.ui"])),
            Err(ParseError::MissingOption {
                command: "ui-compile",
                option: "--out",
            })
        );
    }

    #[test]
    fn a_trace_subcommand_without_its_flag_is_rejected() {
        for command in [Command::Record, Command::Replay] {
            let name = command.name();
            let (flag, _) = command.trace_flags().expect("both carry a flag");
            assert_eq!(
                parse(&arguments(&[name, "input_echo"])),
                Err(ParseError::MissingOption {
                    command: name,
                    option: flag,
                }),
                "`{name}` without {flag} must be rejected"
            );
        }
    }

    #[test]
    fn a_trace_flag_without_a_path_is_rejected() {
        for command in [Command::Record, Command::Replay] {
            let (flag, _) = command.trace_flags().expect("both carry a flag");
            assert_eq!(
                parse(&arguments(&[command.name(), flag])),
                Err(ParseError::MissingValue(flag))
            );
        }
    }

    /// The ordering rule these subcommands exist to respect: past the
    /// sample name, a flag `renew` knows is the sample's. Written out
    /// because it is the failure the design named — `--output` after the
    /// sample would be forwarded, and the sample would reject a flag the
    /// caller did type, blaming the wrong tool.
    #[test]
    fn a_trace_flag_after_the_sample_name_belongs_to_the_sample() {
        assert_eq!(
            parse(&arguments(&[
                "record",
                "--output",
                "a.trace",
                "input_echo",
                "--output",
                "b.trace",
            ])),
            Ok(Parsed::Run(Invocation {
                sample: Some("input_echo".to_string()),
                sample_args: vec!["--output".to_string(), "b.trace".to_string()],
                trace: Some("a.trace".to_string()),
                ..plain(Command::Record)
            }))
        );
    }

    #[test]
    fn a_sample_taking_subcommand_without_a_sample_is_rejected() {
        // Derived from `takes_sample` rather than listed, so a fourth
        // sample-taking subcommand cannot be added without an answer
        // here. Record and replay need their flag first, or they fail on
        // that instead and never reach the question being asked.
        for command in Command::ALL.into_iter().filter(|c| c.takes_sample()) {
            let name = command.name();
            let mut line = vec![name];
            if let Some((flag, _)) = command.trace_flags() {
                line.extend_from_slice(&[flag, "walk.trace"]);
            }
            assert_eq!(
                parse(&arguments(&line)),
                Err(ParseError::MissingSample(name)),
                "`{name}` without a sample must be rejected, naming itself"
            );
            line.insert(0, "--json");
            assert_eq!(
                parse(&arguments(&line)),
                Err(ParseError::MissingSample(name)),
                "`--json {name}` must be rejected the same way"
            );
        }
    }

    #[test]
    fn the_sample_command_line_is_taken_verbatim_with_or_without_a_separator() {
        // The two spellings CI and a person respectively use; the sample
        // must not be able to tell them apart.
        let expected = Ok(running(
            "hello_triangle",
            &["--headless", "--frames", "600"],
        ));
        assert_eq!(
            parse(&arguments(&[
                "run",
                "hello_triangle",
                "--headless",
                "--frames",
                "600"
            ])),
            expected
        );
        assert_eq!(
            parse(&arguments(&[
                "run",
                "hello_triangle",
                "--",
                "--headless",
                "--frames",
                "600"
            ])),
            expected
        );
    }

    #[test]
    fn flags_this_binary_knows_still_reach_the_sample() {
        // Nothing after the sample name is claimed here — otherwise a
        // sample could never own a flag whose name renew also uses.
        assert_eq!(
            parse(&arguments(&[
                "run",
                "hello_triangle",
                "--json",
                "--smoke",
                "help"
            ])),
            Ok(running("hello_triangle", &["--json", "--smoke", "help"]))
        );
        // Before the sample name, the same flag is renew's.
        assert_eq!(
            parse(&arguments(&[
                "run",
                "--json",
                "hello_triangle",
                "--headless"
            ])),
            Ok(Parsed::Run(Invocation {
                json: true,
                sample: Some("hello_triangle".to_string()),
                sample_args: vec!["--headless".to_string()],
                ..plain(Command::Run)
            }))
        );
    }

    #[test]
    fn only_the_first_separator_is_the_separator() {
        // A sample wanting a literal `--` writes two, exactly as it would
        // through `cargo run`.
        assert_eq!(
            parse(&arguments(&["run", "sample", "--", "--", "x"])),
            Ok(running("sample", &["--", "x"]))
        );
        // Later ones are ordinary arguments, in place.
        assert_eq!(
            parse(&arguments(&["run", "sample", "-a", "--", "b"])),
            Ok(running("sample", &["-a", "--", "b"]))
        );
        // A sample can also be run with nothing at all.
        assert_eq!(
            parse(&arguments(&["run", "sample"])),
            Ok(running("sample", &[]))
        );
        assert_eq!(
            parse(&arguments(&["run", "sample", "--"])),
            Ok(running("sample", &[]))
        );
    }

    /// Two occurrences mean the union, and both survive.
    ///
    /// The same rule `--compare` follows, for the same reason: keeping
    /// only the last is how a caller who asked for a window *and* sound
    /// silently gets one of them, with nothing anywhere saying so.
    #[test]
    fn features_accumulate_across_occurrences() {
        assert_eq!(
            parse(&arguments(&[
                "--features",
                "window",
                "--features",
                "audio",
                "run",
                "glide"
            ])),
            Ok(Parsed::Run(Invocation {
                sample: Some("glide".to_string()),
                features: vec!["window".to_string(), "audio".to_string()],
                ..plain(Command::Run)
            }))
        );
    }

    /// The flag is accepted after the subcommand too, since it only has
    /// to precede the sample name -- everything after that name is the
    /// sample's.
    #[test]
    fn features_may_sit_on_either_side_of_the_subcommand() {
        let before = parse(&arguments(&["--features", "window", "run", "glide"]));
        let after = parse(&arguments(&["run", "--features", "window", "glide"]));
        assert_eq!(before, after, "position before the sample name is free");
    }

    /// After the sample name it belongs to the sample, not to cargo.
    ///
    /// This is the pass-through contract, and it has to hold for a flag
    /// renew itself knows -- otherwise the rule "everything after the
    /// sample name is the sample's" would have exceptions a reader must
    /// memorise.
    #[test]
    fn features_after_the_sample_name_belong_to_the_sample() {
        assert_eq!(
            parse(&arguments(&["run", "glide", "--features", "window"])),
            Ok(Parsed::Run(Invocation {
                sample: Some("glide".to_string()),
                sample_args: vec!["--features".to_string(), "window".to_string()],
                ..plain(Command::Run)
            }))
        );
    }

    /// Features name how a sample is built, so a subcommand that builds
    /// no sample refuses them rather than ignoring them.
    #[test]
    fn features_are_refused_where_no_sample_is_built() {
        for command in ["build", "test", "lint", "coverage"] {
            assert_eq!(
                parse(&arguments(&["--features", "window", command])),
                Err(ParseError::UnexpectedArgument("--features".to_string())),
                "{command} builds no sample, so the flag has no meaning there"
            );
        }
    }

    #[test]
    fn features_without_a_value_says_which_flag() {
        assert_eq!(
            parse(&arguments(&["--features"])),
            Err(ParseError::MissingValue("--features"))
        );
    }

    #[test]
    fn a_sample_name_is_never_read_as_a_subcommand() {
        // A sample called `build` is still the sample, because the first
        // free argument after `run` is a name, not a command.
        assert_eq!(
            parse(&arguments(&["run", "build"])),
            Ok(Parsed::Run(Invocation {
                sample: Some("build".to_string()),
                ..plain(Command::Run)
            }))
        );
    }

    #[test]
    fn unknown_command_is_rejected() {
        assert_eq!(
            parse(&arguments(&["deploy"])),
            Err(ParseError::UnknownCommand("deploy".to_string()))
        );
    }

    #[test]
    fn trailing_argument_is_rejected() {
        assert_eq!(
            parse(&arguments(&["build", "extra"])),
            Err(ParseError::UnexpectedArgument("extra".to_string()))
        );
    }

    #[test]
    fn missing_command_is_rejected() {
        assert_eq!(parse(&arguments(&[])), Err(ParseError::NoCommand));
        assert_eq!(parse(&arguments(&["--json"])), Err(ParseError::NoCommand));
    }

    #[test]
    fn help_short_circuits_in_any_position() {
        for list in [&["help"][..], &["--help"], &["-h"], &["build", "--help"]] {
            assert_eq!(parse(&arguments(list)), Ok(Parsed::Help { json: false }));
        }
    }

    #[test]
    fn help_sees_the_json_flag_in_either_order() {
        for list in [&["--json", "help"][..], &["help", "--json"]] {
            assert_eq!(parse(&arguments(list)), Ok(Parsed::Help { json: true }));
        }
    }

    #[test]
    fn arguments_after_help_are_ignored() {
        assert_eq!(
            parse(&arguments(&["help", "nonsense"])),
            Ok(Parsed::Help { json: false })
        );
    }

    #[test]
    fn help_swallows_the_smoke_and_report_flags_like_any_other_argument() {
        // Deliberate: help short-circuits everything except `--json`
        // (same rule as `help nonsense`), so the subcommand-specific
        // validation never runs and help still prints. `--report` still
        // eats its value, which is why the path never reaches the
        // subcommand slot.
        for list in [
            &["help", "--smoke"][..],
            &["--smoke", "--help"],
            &["help", "--report", "cov.json"],
            &["--report", "cov.json", "--help"],
        ] {
            assert_eq!(parse(&arguments(list)), Ok(Parsed::Help { json: false }));
        }
    }

    /// The command half is derived from `Command::ALL`, so it cannot rot.
    ///
    /// **The option half used to be three hardcoded strings under a name
    /// promising every option**, and five flags went undocumented while
    /// this passed. The flags are now checked against the parser's own
    /// match arms by `the_usage_text_documents_every_flag_the_parser_
    /// accepts` in `tests/workspace_lists.rs`; the three kept here are a
    /// cheap smoke test in the same file as the text, not the guarantee.
    /// The name says what is actually checked.
    #[test]
    fn usage_lists_every_command_and_explains_the_pass_through() {
        let text = usage();
        for command in Command::ALL {
            let name = command.name();
            assert!(text.contains(name), "usage text is missing `{name}`");
        }
        for option in ["--json", "--report", "--smoke"] {
            assert!(text.contains(option), "usage text is missing `{option}`");
        }
        // The pass-through rule is the one thing about this command line
        // a reader cannot guess, so it is spelled out rather than implied.
        assert!(
            text.contains("run <sample> [--] [sample arguments...]"),
            "usage text does not show run's shape"
        );
        assert!(
            text.contains("goes to the sample untouched"),
            "usage text does not explain the pass-through"
        );
    }

    #[test]
    fn every_parse_error_says_what_went_wrong() {
        for error in [
            ParseError::NoCommand,
            ParseError::UnknownCommand("deploy".to_string()),
            ParseError::UnexpectedArgument("extra".to_string()),
            ParseError::MissingValue("--report"),
            ParseError::MissingOption {
                command: "coverage",
                option: "--report",
            },
            ParseError::MissingSample("record"),
        ] {
            assert!(!error.to_string().is_empty(), "{error:?}");
        }
    }
}
