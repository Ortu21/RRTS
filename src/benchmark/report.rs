use std::{
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

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
    pub workload: &'static str,
    pub repeat: usize,
    pub mode: &'static str,
    pub samples: Vec<Sample>,
    pub order_ms: f64,
    pub planned: usize,
    pub failed: usize,
    pub arrived: usize,
    pub kills: usize,
    pub pass: bool,
    pub valid_timing: bool,
    pub checksum: u64,
}
#[derive(Default)]
pub struct Stats {
    pub mean: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}
impl Stats {
    pub fn new(values: impl Iterator<Item = f64>) -> Self {
        let mut values: Vec<_> = values.collect();
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
pub struct Report {
    pub directory: PathBuf,
    pub runs: Vec<Run>,
}
impl Report {
    pub fn new(output: Option<PathBuf>) -> io::Result<Self> {
        let directory = output.unwrap_or_else(|| {
            PathBuf::from(format!(
                "benchmark-results/run-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
            ))
        });
        if let Some(parent) = directory.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        fs::create_dir(&directory)?;
        Ok(Self {
            directory,
            runs: Vec::new(),
        })
    }
    pub fn write(&self) -> io::Result<()> {
        let mut summary = BufWriter::new(File::create(self.directory.join("summary.csv"))?);
        let mut samples = BufWriter::new(File::create(self.directory.join("samples.csv"))?);
        let mut markdown = BufWriter::new(File::create(self.directory.join("report.md"))?);
        writeln!(
            summary,
            "version,mode,per_team,total_units,workload,repeat,samples,mean_update_ms,p50_update_ms,p95_update_ms,p99_update_ms,max_update_ms,mean_frame_ms,p95_frame_ms,order_ms,planning_total_ms,planned,failed,arrived,kills,correctness_pass,timing_valid,checksum"
        )?;
        writeln!(
            samples,
            "mode,per_team,workload,repeat,tick,update_ms,frame_ms,planning_ms,moving,pending,engaging,projectiles"
        )?;
        writeln!(
            markdown,
            "# RTS v{} benchmark\n\nProfile: **{}** · OS: {} · architecture: {}\n\nHeadless `update_ms` measures `App::update` CPU wall time at a fixed simulation step of 1/60 s. Graphical `update_ms` measures the main schedule only; `frame_ms` includes rendering/presentation waits. Headless values are not graphical FPS.\n\n| Mode | Units/team | Workload | Repeat | Mean CPU ms | p95 CPU ms | p99 CPU ms | Max CPU ms | Arrived/Survivors | Kills | Correct | Timing valid |\n|---|---:|---|---:|---:|---:|---:|---:|---:|---:|---|---|---|",
            env!("CARGO_PKG_VERSION"),
            profile(),
            std::env::consts::OS,
            std::env::consts::ARCH
        )?;
        for run in &self.runs {
            let stats = Stats::new(run.samples.iter().map(|s| s.update_ms));
            let frames = Stats::new(
                run.samples
                    .iter()
                    .filter(|s| s.frame_ms > 0.0)
                    .map(|s| s.frame_ms),
            );
            let planning: f64 = run.samples.iter().map(|s| s.planning_ms).sum();
            writeln!(
                summary,
                "{},{},{},{},{},{},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{},{},{},{},{},{},{:016x}",
                env!("CARGO_PKG_VERSION"),
                run.mode,
                run.per_team,
                run.per_team * 2,
                run.workload,
                run.repeat,
                run.samples.len(),
                stats.mean,
                stats.p50,
                stats.p95,
                stats.p99,
                stats.max,
                frames.mean,
                frames.p95,
                run.order_ms,
                planning,
                run.planned,
                run.failed,
                run.arrived,
                run.kills,
                run.pass,
                run.valid_timing,
                run.checksum
            )?;
            for sample in &run.samples {
                writeln!(
                    samples,
                    "{},{},{},{},{},{:.6},{:.6},{:.6},{},{},{},{}",
                    run.mode,
                    run.per_team,
                    run.workload,
                    run.repeat,
                    sample.tick,
                    sample.update_ms,
                    sample.frame_ms,
                    sample.planning_ms,
                    sample.moving,
                    sample.pending,
                    sample.engaging,
                    sample.projectiles
                )?;
            }
            writeln!(
                markdown,
                "| {} | {} | {} | {} | {:.3} | {:.3} | {:.3} | {:.3} | {}/{} | {} | {} | {} |",
                run.mode,
                run.per_team,
                run.workload,
                run.repeat,
                stats.mean,
                stats.p95,
                stats.p99,
                stats.max,
                run.arrived,
                run.per_team * 2,
                run.kills,
                run.pass,
                run.valid_timing
            )?;
        }
        writeln!(
            markdown,
            "\nWarmup is excluded. Order generation is timed separately. Crossing correctness requires every unit at its assigned free slot, no pending routes, no path failures, finite in-bounds final positions and obstacle clearance. Idle correctness requires unchanged positions. Skirmish correctness requires at least one kill, no path failures, finite in-bounds positions, matching checksums across repeats (health included) and full accounting of survivors plus kills; obstacle clearance is waived because combat steering is local. Unit-to-unit collisions are intentionally absent from crossing/idle runs. Graphical timing is invalidated by unfocused/occluded frames or user input. Compare runs using the same profile, machine and mode. A short run can fail arrival checks simply because units have not finished.\n\nSee `samples.csv` for raw samples and `metadata.txt` for configuration."
        )?;
        summary.flush()?;
        samples.flush()?;
        markdown.flush()?;
        let mut metadata = File::create(self.directory.join("metadata.txt"))?;
        writeln!(
            metadata,
            "version={}\nbevy=0.19.1\nprofile={}\nos={}\narch={}\nlogical_cpus={}\nheadless_step_seconds=0.0166666667\nheadless_warmup_ticks=120\ngraphical_warmup_seconds=3\npaths_per_frame={}\ncell_size={}\nunit_speed=7\nformation_spacing=2.5\nacquire_stride={}\npercentiles=nearest_rank\nrendering_in_headless=false\ngraphical_vsync=AutoNoVsync\ngraphical_resolution_logical=1280x800\n",
            env!("CARGO_PKG_VERSION"),
            profile(),
            std::env::consts::OS,
            std::env::consts::ARCH,
            std::thread::available_parallelism().map_or(1, usize::from),
            crate::navigation::PATHS_PER_FRAME,
            crate::navigation::CELL_SIZE,
            crate::combat::ACQUIRE_STRIDE
        )?;
        Ok(())
    }
}
pub fn profile() -> &'static str {
    if cfg!(debug_assertions) {
        "dev (crate opt-level=1, dependencies opt-level=3)"
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
}
