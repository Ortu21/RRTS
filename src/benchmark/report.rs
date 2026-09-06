use super::cli::Workload;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use sysinfo::System;

pub const REPORT_SCHEMA_VERSION: u32 = 1;
pub const SUITE_SCHEMA_VERSION: u32 = 1;
pub const HISTORY_ROOT: &str = "benchmarks/history";
pub const TELEMETRY_INTERVAL_TICKS: usize = 60;

#[derive(Clone)]
pub struct Sample {
    pub tick: usize,
    pub update_ms: f64,
    pub frame_ms: f64,
    pub planning_ms: f64,
    pub moving: usize,
    pub pending: usize,
    pub engaging: usize,
    pub projectiles: usize,
}

pub struct Run {
    pub per_team: usize,
    pub workload: Workload,
    pub repeat: usize,
    pub mode: &'static str,
    pub ticks: usize,
    pub samples: Vec<Sample>,
    pub order_ms: f64,
    pub planned: usize,
    pub failed: usize,
    pub arrived: usize,
    pub kills: usize,
    pub correctness_pass: bool,
    pub timing_valid: bool,
    pub checksum: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Stats {
    pub mean: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

impl Stats {
    pub fn new(values: impl Iterator<Item = f64>) -> Self {
        let mut values: Vec<_> = values.filter(|value| value.is_finite()).collect();
        if values.is_empty() {
            return Self::default();
        }
        values.sort_by(f64::total_cmp);
        let percentile =
            |p: f64| values[((values.len() as f64 * p).ceil() as usize).saturating_sub(1)];
        Self {
            mean: values.iter().sum::<f64>() / values.len() as f64,
            p50: percentile(0.5),
            p95: percentile(0.95),
            p99: percentile(0.99),
            max: *values.last().unwrap(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Metadata {
    pub package_version: String,
    pub generated_unix_ms: u64,
    pub git_commit: String,
    pub git_dirty: bool,
    pub rustc: String,
    pub bevy: String,
    pub cargo_profile: String,
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub cpu: String,
    pub physical_cpus: usize,
    pub logical_cpus: usize,
    pub memory_bytes: u64,
    pub machine_key: String,
}

impl Metadata {
    pub fn collect() -> Self {
        let mut system = System::new_all();
        system.refresh_all();
        let cpu = system
            .cpus()
            .first()
            .map(|cpu| cpu.brand().trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "unknown-cpu".into());
        let physical_cpus = System::physical_core_count().unwrap_or(0);
        let logical_cpus = std::thread::available_parallelism().map_or(1, usize::from);
        let memory_bytes = system.total_memory();
        let os = System::name().unwrap_or_else(|| std::env::consts::OS.into());
        let os_version = System::os_version().unwrap_or_else(|| "unknown".into());
        let machine_key = sanitize(&format!(
            "{}-{}-{}-{}c-{}gb",
            os,
            std::env::consts::ARCH,
            cpu,
            physical_cpus,
            memory_bytes / 1024 / 1024 / 1024
        ));
        Self {
            package_version: env!("CARGO_PKG_VERSION").into(),
            generated_unix_ms: now_ms(),
            git_commit: command_output("git", &["rev-parse", "HEAD"]),
            git_dirty: !command_output("git", &["status", "--porcelain"]).is_empty(),
            rustc: command_output("rustc", &["--version"]),
            bevy: "0.19.1".into(),
            cargo_profile: std::env::var("RRTS_RUN_PROFILE").unwrap_or_else(|_| profile().into()),
            os,
            os_version,
            arch: std::env::consts::ARCH.into(),
            cpu,
            physical_cpus,
            logical_cpus,
            memory_bytes,
            machine_key,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RunSummary {
    pub mode: String,
    pub workload: Workload,
    pub per_team: usize,
    pub total_units: usize,
    pub repeat: usize,
    pub ticks: usize,
    pub update_ms: Stats,
    pub frame_ms: Stats,
    pub order_ms: f64,
    pub planning_total_ms: f64,
    pub planned: usize,
    pub failed: usize,
    pub arrived: usize,
    pub kills: usize,
    pub correctness_pass: bool,
    pub timing_valid: bool,
    pub checksum: String,
}

impl From<&Run> for RunSummary {
    fn from(run: &Run) -> Self {
        Self {
            mode: run.mode.into(),
            workload: run.workload,
            per_team: run.per_team,
            total_units: run.per_team * 2,
            repeat: run.repeat,
            ticks: run.ticks,
            update_ms: Stats::new(run.samples.iter().map(|sample| sample.update_ms)),
            frame_ms: Stats::new(
                run.samples
                    .iter()
                    .filter(|sample| sample.frame_ms > 0.0)
                    .map(|sample| sample.frame_ms),
            ),
            order_ms: run.order_ms,
            planning_total_ms: run.samples.iter().map(|sample| sample.planning_ms).sum(),
            planned: run.planned,
            failed: run.failed,
            arrived: run.arrived,
            kills: run.kills,
            correctness_pass: run.correctness_pass,
            timing_valid: run.timing_valid,
            checksum: format!("{:016x}", run.checksum),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Aggregate {
    pub case_id: String,
    pub mode: String,
    pub workload: Workload,
    pub per_team: usize,
    pub ticks: usize,
    pub repeats: usize,
    pub median_mean_update_ms: f64,
    pub mad_mean_update_ms: f64,
    pub min_mean_update_ms: f64,
    pub max_mean_update_ms: f64,
    pub median_p95_update_ms: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BenchmarkArtifact {
    pub kind: String,
    pub schema_version: u32,
    pub suite_schema_version: u32,
    pub preset: String,
    pub metadata: Metadata,
    pub runs: Vec<RunSummary>,
    pub aggregates: Vec<Aggregate>,
}

pub struct Report {
    pub directory: PathBuf,
    pub runs: Vec<Run>,
    preset: String,
    metadata: Metadata,
}

impl Report {
    pub fn new(output: Option<PathBuf>, preset: impl Into<String>) -> io::Result<Self> {
        let directory = new_output_directory(output, "run")?;
        Ok(Self {
            directory,
            runs: Vec::new(),
            preset: preset.into(),
            metadata: Metadata::collect(),
        })
    }

    pub fn artifact(&self) -> BenchmarkArtifact {
        let runs: Vec<_> = self.runs.iter().map(RunSummary::from).collect();
        BenchmarkArtifact {
            kind: "benchmark".into(),
            schema_version: REPORT_SCHEMA_VERSION,
            suite_schema_version: SUITE_SCHEMA_VERSION,
            preset: self.preset.clone(),
            metadata: self.metadata.clone(),
            aggregates: aggregates(&runs),
            runs,
        }
    }

    pub fn write(&self) -> io::Result<BenchmarkArtifact> {
        let artifact = self.artifact();
        serde_json::to_writer_pretty(
            BufWriter::new(File::create(self.directory.join("run.json"))?),
            &artifact,
        )?;
        self.write_csv_and_markdown(&artifact)?;
        self.write_metadata()?;
        Ok(artifact)
    }

    pub fn record_history(&self, artifact: &BenchmarkArtifact) -> io::Result<PathBuf> {
        record_artifact("benchmark", &artifact.metadata, &artifact.preset, artifact)
    }

    fn write_csv_and_markdown(&self, artifact: &BenchmarkArtifact) -> io::Result<()> {
        let mut summary = BufWriter::new(File::create(self.directory.join("summary.csv"))?);
        let mut samples = BufWriter::new(File::create(self.directory.join("samples.csv"))?);
        let mut markdown = BufWriter::new(File::create(self.directory.join("report.md"))?);
        writeln!(
            summary,
            "version,profile,mode,workload,per_team,total_units,repeat,ticks,mean_update_ms,p50_update_ms,p95_update_ms,p99_update_ms,max_update_ms,mean_frame_ms,p95_frame_ms,order_ms,planning_total_ms,planned,failed,arrived,kills,correctness_pass,timing_valid,checksum,suite_schema_version"
        )?;
        writeln!(
            samples,
            "mode,workload,per_team,repeat,tick,update_ms,frame_ms,planning_ms,moving,pending,engaging,projectiles"
        )?;
        writeln!(
            markdown,
            "# RTS v{} benchmark\n\nPreset: **{}** · profile: **{}** · machine: `{}`\n\nPerformance values are descriptive only and never produce a regression verdict. Functional correctness and timing validity are reported separately.\n\n| Workload | Units/team | Repeat | Ticks | Mean ms | p95 ms | p99 ms | Max ms | Arrived/Survivors | Kills | Correct | Timing valid |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|",
            artifact.metadata.package_version,
            artifact.preset,
            artifact.metadata.cargo_profile,
            artifact.metadata.machine_key,
        )?;
        for (run, run_summary) in self.runs.iter().zip(&artifact.runs) {
            writeln!(
                summary,
                "{},{},{},{},{},{},{},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{},{},{},{},{},{},{},{}",
                artifact.metadata.package_version,
                artifact.metadata.cargo_profile,
                run_summary.mode,
                run_summary.workload.as_str(),
                run_summary.per_team,
                run_summary.total_units,
                run_summary.repeat,
                run_summary.ticks,
                run_summary.update_ms.mean,
                run_summary.update_ms.p50,
                run_summary.update_ms.p95,
                run_summary.update_ms.p99,
                run_summary.update_ms.max,
                run_summary.frame_ms.mean,
                run_summary.frame_ms.p95,
                run_summary.order_ms,
                run_summary.planning_total_ms,
                run_summary.planned,
                run_summary.failed,
                run_summary.arrived,
                run_summary.kills,
                run_summary.correctness_pass,
                run_summary.timing_valid,
                run_summary.checksum,
                SUITE_SCHEMA_VERSION,
            )?;
            for sample in &run.samples {
                writeln!(
                    samples,
                    "{},{},{},{},{},{:.6},{:.6},{:.6},{},{},{},{}",
                    run.mode,
                    run.workload.as_str(),
                    run.per_team,
                    run.repeat,
                    sample.tick,
                    sample.update_ms,
                    sample.frame_ms,
                    sample.planning_ms,
                    sample.moving,
                    sample.pending,
                    sample.engaging,
                    sample.projectiles,
                )?;
            }
            writeln!(
                markdown,
                "| {} | {} | {} | {} | {:.3} | {:.3} | {:.3} | {:.3} | {}/{} | {} | {} | {} |",
                run_summary.workload.as_str(),
                run_summary.per_team,
                run_summary.repeat,
                run_summary.ticks,
                run_summary.update_ms.mean,
                run_summary.update_ms.p95,
                run_summary.update_ms.p99,
                run_summary.update_ms.max,
                run_summary.arrived,
                run_summary.total_units,
                run_summary.kills,
                run_summary.correctness_pass,
                run_summary.timing_valid,
            )?;
        }
        writeln!(
            markdown,
            "\n## Aggregate stability\n\n| Case | Repeats | Median mean ms | MAD ms | Min–max ms | Median p95 ms |\n|---|---:|---:|---:|---:|---:|"
        )?;
        for aggregate in &artifact.aggregates {
            writeln!(
                markdown,
                "| {} | {} | {:.3} | {:.3} | {:.3}–{:.3} | {:.3} |",
                aggregate.case_id,
                aggregate.repeats,
                aggregate.median_mean_update_ms,
                aggregate.mad_mean_update_ms,
                aggregate.min_mean_update_ms,
                aggregate.max_mean_update_ms,
                aggregate.median_p95_update_ms,
            )?;
        }
        writeln!(
            markdown,
            "\nTelemetry state is sampled every {TELEMETRY_INTERVAL_TICKS} ticks outside the timed `App::update`; raw timing remains per tick. Compare only matching machine, profile, suite schema and case configuration."
        )?;
        Ok(())
    }

    fn write_metadata(&self) -> io::Result<()> {
        let mut metadata = BufWriter::new(File::create(self.directory.join("metadata.txt"))?);
        writeln!(metadata, "schema_version={REPORT_SCHEMA_VERSION}")?;
        writeln!(metadata, "suite_schema_version={SUITE_SCHEMA_VERSION}")?;
        writeln!(metadata, "version={}", self.metadata.package_version)?;
        writeln!(metadata, "git_commit={}", self.metadata.git_commit)?;
        writeln!(metadata, "git_dirty={}", self.metadata.git_dirty)?;
        writeln!(metadata, "rustc={}", self.metadata.rustc)?;
        writeln!(metadata, "bevy={}", self.metadata.bevy)?;
        writeln!(metadata, "profile={}", self.metadata.cargo_profile)?;
        writeln!(metadata, "os={}", self.metadata.os)?;
        writeln!(metadata, "os_version={}", self.metadata.os_version)?;
        writeln!(metadata, "arch={}", self.metadata.arch)?;
        writeln!(metadata, "cpu={}", self.metadata.cpu)?;
        writeln!(metadata, "physical_cpus={}", self.metadata.physical_cpus)?;
        writeln!(metadata, "logical_cpus={}", self.metadata.logical_cpus)?;
        writeln!(metadata, "memory_bytes={}", self.metadata.memory_bytes)?;
        writeln!(metadata, "machine_key={}", self.metadata.machine_key)?;
        writeln!(metadata, "headless_step_seconds=0.0166666667")?;
        writeln!(metadata, "headless_warmup_ticks=120")?;
        writeln!(
            metadata,
            "telemetry_interval_ticks={TELEMETRY_INTERVAL_TICKS}"
        )?;
        writeln!(
            metadata,
            "paths_per_frame={}",
            crate::navigation::PATHS_PER_FRAME
        )?;
        writeln!(metadata, "cell_size={}", crate::navigation::CELL_SIZE)?;
        writeln!(metadata, "map_half_size={}", crate::navigation::HALF_SIZE)?;
        writeln!(metadata, "map_seed={}", crate::navigation::MAP_SEED)?;
        writeln!(metadata, "acquire_stride={}", crate::combat::ACQUIRE_STRIDE)?;
        Ok(())
    }
}

pub fn write_history_report(output: Option<PathBuf>) -> io::Result<PathBuf> {
    let directory = new_output_directory(output, "history")?;
    let mut artifacts = Vec::new();
    visit_json(Path::new(HISTORY_ROOT), &mut |path| {
        let file = File::open(path)?;
        if let Ok(artifact) = serde_json::from_reader::<_, BenchmarkArtifact>(file)
            && artifact.kind == "benchmark"
        {
            artifacts.push(artifact);
        }
        Ok(())
    })?;
    artifacts.sort_by_key(|artifact| artifact.metadata.generated_unix_ms);

    let mut csv = BufWriter::new(File::create(directory.join("history.csv"))?);
    let mut markdown = BufWriter::new(File::create(directory.join("history.md"))?);
    writeln!(
        csv,
        "timestamp_ms,version,commit,machine,profile,suite_schema,preset,case_id,median_mean_ms,mad_ms,previous_median_ms,delta_percent"
    )?;
    writeln!(
        markdown,
        "# Benchmark history\n\nPassive historical comparison; deltas are descriptive and never produce a regression verdict.\n\n| Version | Commit | Machine | Profile | Case | Median mean ms | MAD ms | Previous ms | Delta |\n|---|---|---|---|---|---:|---:|---:|---:|"
    )?;
    let mut previous: BTreeMap<String, f64> = BTreeMap::new();
    for artifact in &artifacts {
        for aggregate in &artifact.aggregates {
            let key = format!(
                "{}|{}|{}|{}|{}",
                artifact.metadata.machine_key,
                artifact.metadata.cargo_profile,
                artifact.suite_schema_version,
                artifact.preset,
                aggregate.case_id
            );
            let old = previous.insert(key, aggregate.median_mean_update_ms);
            let delta = old
                .filter(|value| *value > 0.0)
                .map(|value| (aggregate.median_mean_update_ms / value - 1.0) * 100.0);
            writeln!(
                csv,
                "{},{},{},{},{},{},{},{},{:.6},{:.6},{},{}",
                artifact.metadata.generated_unix_ms,
                artifact.metadata.package_version,
                artifact.metadata.git_commit,
                artifact.metadata.machine_key,
                artifact.metadata.cargo_profile,
                artifact.suite_schema_version,
                artifact.preset,
                aggregate.case_id,
                aggregate.median_mean_update_ms,
                aggregate.mad_mean_update_ms,
                old.map_or_else(String::new, |value| format!("{value:.6}")),
                delta.map_or_else(String::new, |value| format!("{value:.3}")),
            )?;
            writeln!(
                markdown,
                "| {} | {} | `{}` | {} | {} | {:.3} | {:.3} | {} | {} |",
                artifact.metadata.package_version,
                short_commit(&artifact.metadata.git_commit),
                artifact.metadata.machine_key,
                artifact.metadata.cargo_profile,
                aggregate.case_id,
                aggregate.median_mean_update_ms,
                aggregate.mad_mean_update_ms,
                old.map_or_else(|| "—".into(), |value| format!("{value:.3}")),
                delta.map_or_else(|| "—".into(), |value| format!("{value:+.2}%")),
            )?;
        }
    }
    Ok(directory)
}

pub fn record_json_artifact<T: Serialize>(
    kind: &str,
    metadata: &Metadata,
    identity: &str,
    value: &T,
) -> io::Result<PathBuf> {
    record_artifact(kind, metadata, identity, value)
}

fn aggregates(runs: &[RunSummary]) -> Vec<Aggregate> {
    let mut groups: BTreeMap<(String, String, usize, usize), Vec<&RunSummary>> = BTreeMap::new();
    for run in runs {
        groups
            .entry((
                run.mode.clone(),
                run.workload.as_str().into(),
                run.per_team,
                run.ticks,
            ))
            .or_default()
            .push(run);
    }
    groups
        .into_iter()
        .map(|((mode, workload, per_team, ticks), runs)| {
            let means: Vec<_> = runs.iter().map(|run| run.update_ms.mean).collect();
            let p95s: Vec<_> = runs.iter().map(|run| run.update_ms.p95).collect();
            let median = Stats::new(means.iter().copied()).p50;
            let mad = Stats::new(means.iter().map(|value| (value - median).abs())).p50;
            Aggregate {
                case_id: format!("{mode}/{workload}/{per_team}x2/{ticks}t"),
                mode,
                workload: match workload.as_str() {
                    "idle" => Workload::Idle,
                    "crossing" => Workload::Crossing,
                    "crowd" => Workload::Crowd,
                    _ => Workload::Skirmish,
                },
                per_team,
                ticks,
                repeats: runs.len(),
                median_mean_update_ms: median,
                mad_mean_update_ms: mad,
                min_mean_update_ms: means.iter().copied().fold(f64::INFINITY, f64::min),
                max_mean_update_ms: means.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                median_p95_update_ms: Stats::new(p95s.into_iter()).p50,
            }
        })
        .collect()
}

fn record_artifact<T: Serialize>(
    kind: &str,
    metadata: &Metadata,
    identity: &str,
    value: &T,
) -> io::Result<PathBuf> {
    if metadata.git_commit.is_empty() || metadata.git_dirty {
        return Err(io::Error::other(
            "history recording requires a clean Git worktree and a known commit",
        ));
    }
    let directory = PathBuf::from(HISTORY_ROOT)
        .join(kind)
        .join(&metadata.machine_key);
    fs::create_dir_all(&directory)?;
    let prefix = format!(
        "{}-{}-{}",
        metadata.package_version,
        short_commit(&metadata.git_commit),
        sanitize(identity)
    );
    if fs::read_dir(&directory)?
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{prefix}-"))
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "an equivalent history snapshot already exists",
        ));
    }
    let path = directory.join(format!("{prefix}-{}.json", metadata.generated_unix_ms));
    serde_json::to_writer_pretty(BufWriter::new(File::create(&path)?), value)?;
    Ok(path)
}

fn visit_json(
    directory: &Path,
    callback: &mut impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    if !directory.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            visit_json(&path, callback)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            callback(&path)?;
        }
    }
    Ok(())
}

fn new_output_directory(output: Option<PathBuf>, prefix: &str) -> io::Result<PathBuf> {
    let directory =
        output.unwrap_or_else(|| PathBuf::from(format!("benchmark-results/{prefix}-{}", now_ms())));
    if let Some(parent) = directory
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir(&directory)?;
    Ok(directory)
}

fn command_output(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|output| output.trim().to_owned())
        .unwrap_or_default()
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn short_commit(commit: &str) -> &str {
    commit.get(..commit.len().min(8)).unwrap_or(commit)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn profile() -> &'static str {
    if cfg!(debug_assertions) {
        "dev"
    } else {
        "release"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_statistics_use_nearest_rank() {
        let stats = Stats::new([1.0, 100.0, 2.0].into_iter());
        assert!((stats.mean - 103.0 / 3.0).abs() < 1e-8);
        assert_eq!(
            (stats.p50, stats.p95, stats.p99, stats.max),
            (2.0, 100.0, 100.0, 100.0)
        );
        assert_eq!(Stats::new(std::iter::empty()).mean, 0.0);
    }

    #[test]
    fn machine_keys_are_path_safe() {
        assert_eq!(sanitize("macOS / Apple M1 Pro (8)"), "macos-apple-m1-pro-8");
    }
}
