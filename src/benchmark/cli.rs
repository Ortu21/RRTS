use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Workload {
    Idle,
    #[default]
    Crossing,
    Crowd,
    Skirmish,
    Guard,
}

impl Workload {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Crossing => "crossing",
            Self::Crowd => "crowd",
            Self::Skirmish => "skirmish",
            Self::Guard => "guard",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "idle" => Ok(Self::Idle),
            "crossing" => Ok(Self::Crossing),
            "crowd" => Ok(Self::Crowd),
            "skirmish" => Ok(Self::Skirmish),
            "guard" => Ok(Self::Guard),
            _ => Err(format!(
                "Unknown workload '{value}'; use idle, crossing, crowd, skirmish or guard"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SuitePreset {
    Quick,
    #[default]
    Full,
}

impl SuitePreset {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Full => "full",
        }
    }
}

#[derive(Clone)]
pub struct Config {
    pub benchmark: bool,
    pub suite: bool,
    pub preset: SuitePreset,
    pub headless: bool,
    pub workload: Workload,
    pub profile_capture: bool,
    pub record_history: bool,
    pub history_report: bool,
    pub profile_report: Option<PathBuf>,
    pub profile_run: Option<PathBuf>,
    pub per_team: usize,
    pub ticks: usize,
    pub repeats: usize,
    pub ticks_overridden: bool,
    pub repeats_overridden: bool,
    pub seconds: f64,
    pub output: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            benchmark: false,
            suite: false,
            preset: SuitePreset::Full,
            headless: false,
            workload: Workload::Crossing,
            profile_capture: false,
            record_history: false,
            history_report: false,
            profile_report: None,
            profile_run: None,
            per_team: 1000,
            ticks: 1800,
            repeats: 3,
            ticks_overridden: false,
            repeats_overridden: false,
            seconds: 30.0,
            output: None,
        }
    }
}

impl Config {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Option<Self>, String> {
        let mut config = Self::default();
        let mut args = args.into_iter().peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--help" | "-h" => return Ok(None),
                "--benchmark" => config.benchmark = true,
                "--benchmark-suite" => {
                    config.benchmark = true;
                    config.suite = true;
                    config.headless = true;
                    if args.peek().is_some_and(|value| !value.starts_with('-')) {
                        config.preset = match args.next().as_deref() {
                            Some("quick") => SuitePreset::Quick,
                            Some("full") => SuitePreset::Full,
                            Some(value) => {
                                return Err(format!(
                                    "Unknown suite preset '{value}'; use quick or full"
                                ));
                            }
                            None => unreachable!(),
                        };
                    }
                }
                "--headless" => config.headless = true,
                "--skirmish" => config.workload = Workload::Skirmish,
                "--record-history" => config.record_history = true,
                "--history-report" => config.history_report = true,
                "--profile-capture" => config.profile_capture = true,
                "--profile-systems" => {
                    return Err(
                        "--profile-systems was replaced by scripts/profile.sh (Chrome Trace)"
                            .into(),
                    );
                }
                "--workload" | "--units-per-team" | "--ticks" | "--repeats" | "--seconds"
                | "--output" | "--profile-report" | "--profile-run" => {
                    let value = args
                        .next()
                        .ok_or_else(|| format!("Missing value for {arg}"))?;
                    match arg.as_str() {
                        "--workload" => config.workload = Workload::parse(&value)?,
                        "--units-per-team" => {
                            config.per_team = value.parse().map_err(|_| "Invalid unit count")?
                        }
                        "--ticks" => {
                            config.ticks = value.parse().map_err(|_| "Invalid tick count")?;
                            config.ticks_overridden = true;
                        }
                        "--repeats" => {
                            config.repeats = value.parse().map_err(|_| "Invalid repeat count")?;
                            config.repeats_overridden = true;
                        }
                        "--seconds" => {
                            config.seconds = value.parse().map_err(|_| "Invalid duration")?
                        }
                        "--profile-report" => config.profile_report = Some(value.into()),
                        "--profile-run" => config.profile_run = Some(value.into()),
                        _ => config.output = Some(value.into()),
                    }
                }
                _ => return Err(format!("Unknown option: {arg}")),
            }
        }

        if config.history_report || config.profile_report.is_some() {
            if config.profile_report.is_some() != config.profile_run.is_some() {
                return Err("Use --profile-report and --profile-run together".into());
            }
            return Ok(Some(config));
        }
        if !(1..=10_000).contains(&config.per_team) {
            return Err("Units per team must be 1..10000".into());
        }
        if !(60..=36_000).contains(&config.ticks) {
            return Err("Ticks must be 60..36000".into());
        }
        if !(1..=10).contains(&config.repeats) {
            return Err("Repeats must be 1..10".into());
        }
        if !config.seconds.is_finite() || !(1.0..=600.0).contains(&config.seconds) {
            return Err("Seconds must be 1..600".into());
        }
        if config.headless && !config.benchmark {
            return Err("Use --headless with --benchmark or --benchmark-suite".into());
        }
        if config.profile_capture && (!config.benchmark || config.suite) {
            return Err(
                "Profile capture requires one benchmark case (headless or graphical)".into(),
            );
        }
        if config.record_history && !config.suite {
            return Err("Benchmark history can only record a complete suite".into());
        }
        Ok(Some(config))
    }
}
pub const HELP: &str = "Rust RTS v0.0.10 benchmark and profiler\n\
  cargo run                                      Playground skirmish\n\
  cargo run -- --benchmark                        Graphical crossing\n\
  cargo run -- --benchmark --headless --workload skirmish\n\
  cargo run --profile benchmark -- --benchmark-suite quick\n\
  cargo run --profile benchmark -- --benchmark-suite full\n\
  cargo run --profile benchmark -- --benchmark-suite full --record-history\n\
  cargo run --profile benchmark -- --history-report\n\
  ./scripts/profile.sh --workload skirmish --units-per-team 1000 --ticks 600\n\
  ./scripts/profile-visual.sh --units-per-team 2500 --seconds 60\n\
Options: --workload idle|crossing|crowd|skirmish|guard, --units-per-team 1..10000,\n\
         --ticks 60..36000, --repeats 1..10, --seconds 1..600,\n\
         --output <new-directory>. --skirmish remains an alias.\n\
Performance values are descriptive only; only correctness can fail a run.";

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Config, String> {
        Config::parse(args.iter().map(|arg| (*arg).to_owned())).map(|value| value.unwrap())
    }

    #[test]
    fn parses_presets_workloads_and_legacy_alias() {
        let quick = parse(&["--benchmark-suite", "quick"]).unwrap();
        assert!(quick.headless && quick.suite && quick.benchmark);
        assert_eq!(quick.preset, SuitePreset::Quick);

        assert_eq!(
            parse(&["--benchmark", "--headless", "--workload", "guard"])
                .unwrap()
                .workload,
            Workload::Guard
        );
        let legacy = parse(&["--benchmark", "--headless", "--skirmish"]).unwrap();
        assert_eq!(legacy.workload, Workload::Skirmish);
        assert_eq!(
            parse(&["--benchmark", "--headless", "--workload", "crowd"])
                .unwrap()
                .workload,
            Workload::Crowd
        );
    }

    #[test]
    fn rejects_invalid_combinations() {
        for args in [
            &["--units-per-team", "0"][..],
            &["--seconds", "NaN"],
            &["--ticks"],
            &["--unknown"],
            &["--headless"],
            &["--profile-systems"],
            &["--benchmark-suite", "unknown"],
            &[
                "--benchmark",
                "--headless",
                "--profile-capture",
                "--record-history",
            ][..],
            &[
                "--benchmark",
                "--headless",
                "--profile-capture",
                "--benchmark-suite",
                "quick",
            ][..],
            &["--profile-report", "trace.json"],
        ] {
            assert!(parse(args).is_err(), "{args:?} should fail");
        }
        // Capture works headless and graphical, but never for a suite.
        assert!(parse(&["--benchmark", "--headless", "--profile-capture"]).is_ok());
        assert!(parse(&["--benchmark", "--profile-capture"]).is_ok());
    }
}
