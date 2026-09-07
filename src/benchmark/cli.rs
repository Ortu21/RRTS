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
    AiTest,
}

impl Workload {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Crossing => "crossing",
            Self::Crowd => "crowd",
            Self::Skirmish => "skirmish",
            Self::Guard => "guard",
            Self::AiTest => "ai-test",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "idle" => Ok(Self::Idle),
            "crossing" => Ok(Self::Crossing),
            "crowd" => Ok(Self::Crowd),
            "skirmish" => Ok(Self::Skirmish),
            "guard" => Ok(Self::Guard),
            "ai-test" => Ok(Self::AiTest),
            _ => Err(format!(
                "Unknown workload '{value}'; use idle, crossing, crowd, skirmish, guard or ai-test"
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
    pub economy_benchmark: bool,
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
    pub ai: String,
    /// 0 = blu, 1 = rosso, 2 = entrambi (demo 1v1 AI-vs-AI).
    pub ai_team: u8,
    /// Personalità team rosso (o team singolo). Team blu in demo usa `ai_personality2`.
    pub ai_personality: String,
    /// Personalità team blu quando `ai_team == 2` / `ai == both`.
    pub ai_personality2: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            benchmark: false,
            economy_benchmark: false,
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
            ai: "off".to_owned(),
            ai_team: 1,
            ai_personality: "turtle".to_owned(),
            ai_personality2: "rusher".to_owned(),
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
                "--economy-benchmark" => config.economy_benchmark = true,
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
                | "--output" | "--profile-report" | "--profile-run" | "--ai" | "--ai-team"
                | "--ai-personality" | "--ai-personality2" => {
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
                        "--ai" => config.ai = value,
                        "--ai-team" => {
                            config.ai_team = value.parse().map_err(|_| "Invalid AI team")?
                        }
                        "--ai-personality" => config.ai_personality = value,
                        "--ai-personality2" => config.ai_personality2 = value,
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
        if !["off", "test", "skirmish", "both"].contains(&config.ai.as_str()) {
            return Err("AI mode must be off|test|skirmish|both".into());
        }
        if config.ai_team > 2 {
            return Err("AI team must be 0, 1 or 2 (both)".into());
        }
        if !["turtle", "rusher"].contains(&config.ai_personality.as_str()) {
            return Err("AI personality must be turtle|rusher".into());
        }
        if !["turtle", "rusher"].contains(&config.ai_personality2.as_str()) {
            return Err("AI personality2 must be turtle|rusher".into());
        }
        Ok(Some(config))
    }
}
pub const HELP: &str = "Rust RTS v0.0.14 benchmark and profiler\n\
  cargo run                                      Economy playground\n\
  cargo run -- --ai skirmish                       You (blue) vs AI (red turtle)\n\
  cargo run -- --ai both                           Demo 1v1: AI blue (rusher) vs AI red (turtle)\n\
  cargo run -- --ai both --ai-personality turtle --ai-personality2 rusher\n\
  Demo keys: +/- speed, 0 reset 1x, Space pause. Score panel top-right.\n\
  Match: commander down = defeat. R restarts. Panel VIEW & FOG: play as BLUE/RED, toggle fog.\n\
  cargo run -- --benchmark                        Graphical crossing\n\
  cargo run -- --benchmark --headless --workload skirmish\n\
  cargo run -- --benchmark --headless --workload ai-test --ticks 1800\n\
  cargo run --profile benchmark -- --benchmark-suite quick\n\
  cargo run --profile benchmark -- --benchmark-suite full\n\
  cargo run --profile benchmark -- --benchmark-suite full --record-history\n\
  cargo run --profile benchmark -- --history-report\n\
  ./scripts/profile.sh --workload skirmish --units-per-team 1000 --ticks 600\n\
  ./scripts/profile-visual.sh --units-per-team 2500 --seconds 60\n\
Separate industry measurement: --economy-benchmark --ticks 1800 --repeats 3\n\
Options: --workload idle|crossing|crowd|skirmish|guard|ai-test, --units-per-team 1..10000,\n\
         --ticks 60..36000, --repeats 1..10, --seconds 1..600,\n\
         --ai off|test|skirmish|both, --ai-team 0|1|2 (2=both),\n\
         --ai-personality turtle|rusher (red), --ai-personality2 turtle|rusher (blue),\n\
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

    #[test]
    fn parses_ai_workload_and_options() {
        let ai = parse(&["--benchmark", "--headless", "--workload", "ai-test"]).unwrap();
        assert_eq!(ai.workload, Workload::AiTest);
        assert_eq!(ai.ai, "off");
        let skirmish = parse(&[
            "--ai",
            "skirmish",
            "--ai-team",
            "1",
            "--ai-personality",
            "rusher",
        ])
        .unwrap();
        assert_eq!(skirmish.ai, "skirmish");
        assert_eq!(skirmish.ai_team, 1);
        assert_eq!(skirmish.ai_personality, "rusher");
        // Demo 1v1: both teams AI con personalità distinte.
        let both = parse(&[
            "--ai",
            "both",
            "--ai-personality",
            "turtle",
            "--ai-personality2",
            "rusher",
        ])
        .unwrap();
        assert_eq!(both.ai, "both");
        assert_eq!(both.ai_personality2, "rusher");
        let both_team = parse(&["--ai", "both", "--ai-team", "2"]).unwrap();
        assert_eq!(both_team.ai_team, 2);
        for args in [
            &["--ai", "neural"][..],
            &["--ai-team", "3"][..],
            &["--ai-personality", "random"][..],
            &["--ai-personality2", "random"][..],
        ] {
            assert!(parse(args).is_err(), "{args:?} should fail");
        }
    }
}
