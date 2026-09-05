use std::path::PathBuf;

#[derive(Clone)]
pub struct Config {
    pub benchmark: bool,
    pub suite: bool,
    pub headless: bool,
    pub per_team: usize,
    pub ticks: usize,
    pub repeats: usize,
    pub seconds: f64,
    pub output: Option<PathBuf>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            benchmark: false,
            suite: false,
            headless: false,
            per_team: 1000,
            ticks: 1800,
            repeats: 3,
            seconds: 30.0,
            output: None,
        }
    }
}
impl Config {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Option<Self>, String> {
        let mut config = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--help" | "-h" => return Ok(None),
                "--benchmark" => config.benchmark = true,
                "--benchmark-suite" => {
                    config.benchmark = true;
                    config.suite = true;
                    config.headless = true;
                }
                "--headless" => config.headless = true,
                "--units-per-team" | "--ticks" | "--repeats" | "--seconds" | "--output" => {
                    let value = args
                        .next()
                        .ok_or_else(|| format!("Missing value for {arg}"))?;
                    match arg.as_str() {
                        "--units-per-team" => {
                            config.per_team = value.parse().map_err(|_| "Invalid unit count")?
                        }
                        "--ticks" => {
                            config.ticks = value.parse().map_err(|_| "Invalid tick count")?
                        }
                        "--repeats" => {
                            config.repeats = value.parse().map_err(|_| "Invalid repeat count")?
                        }
                        "--seconds" => {
                            config.seconds = value.parse().map_err(|_| "Invalid duration")?
                        }
                        _ => config.output = Some(value.into()),
                    }
                }
                _ => return Err(format!("Unknown option: {arg}")),
            }
        }
        if !(1..=1000).contains(&config.per_team) {
            return Err("Units per team must be 1..1000".into());
        }
        if !(60..=36000).contains(&config.ticks) {
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
        Ok(Some(config))
    }
}
pub const HELP: &str = "Rust RTS v0.0.2\n\
  cargo run                                      Playground with obstacles\n\
  cargo run -- --benchmark                        Graphical 1000 vs 1000 crossing\n\
  cargo run -- --benchmark --headless             One CPU simulation benchmark\n\
  cargo run -- --benchmark-suite                  100/500/1000 per team, idle/crossing, 3 repeats\n\
Options: --units-per-team 1..1000, --ticks 1800 (headless), --seconds 30 (graphical),\n\
         --repeats 3 (suite), --output <new-directory>\n\
Headless uses fixed 1/60 s simulation steps and excludes rendering.\n\
Graphical runs include 3 s warmup and exit after saving results. No combat.";

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_arguments_and_enables_headless_suite() {
        for args in [
            vec!["--units-per-team", "0"],
            vec!["--seconds", "NaN"],
            vec!["--ticks"],
            vec!["--unknown"],
            vec!["--headless"],
        ] {
            assert!(Config::parse(args.into_iter().map(String::from)).is_err());
        }
        let config = Config::parse(["--benchmark-suite".into()])
            .unwrap()
            .unwrap();
        assert!(config.headless && config.suite && config.benchmark);
    }
}
