//! Chrome Trace post-processing for profiler runs.
//!
//! Bevy emits the actual ECS-system spans. This module never registers marker
//! systems and therefore cannot add ECS dependencies or reorder simulation.

use super::report::{
    BenchmarkArtifact, Metadata, REPORT_SCHEMA_VERSION, Stats, record_json_artifact,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    io::{self, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SystemProfile {
    pub system: String,
    pub calls: usize,
    pub total_ms: f64,
    pub durations_ms: Stats,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProfileArtifact {
    pub kind: String,
    pub schema_version: u32,
    pub metadata: Metadata,
    pub case_id: String,
    pub measurement_wall_ms: f64,
    pub systems: Vec<SystemProfile>,
}

#[derive(Clone)]
struct Span {
    name: String,
    start_us: f64,
    duration_us: f64,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TraceRoot {
    Events(Vec<TraceEvent>),
    Wrapped {
        #[serde(rename = "traceEvents")]
        trace_events: Vec<TraceEvent>,
    },
}

#[derive(Deserialize)]
struct TraceEvent {
    #[serde(default)]
    name: String,
    #[serde(default)]
    ph: String,
    #[serde(default)]
    ts: f64,
    dur: Option<f64>,
    #[serde(default)]
    pid: i64,
    #[serde(default)]
    tid: i64,
}

pub fn write_profile_report(
    trace: &Path,
    benchmark_run: &Path,
    output: Option<PathBuf>,
    record_history: bool,
) -> io::Result<PathBuf> {
    let benchmark: BenchmarkArtifact =
        serde_json::from_reader(BufReader::new(File::open(benchmark_run)?))?;
    let parsed_trace: TraceRoot = serde_json::from_reader(BufReader::new(File::open(trace)?))?;
    let spans = parse_spans(parsed_trace);
    let measurement = spans
        .iter()
        .filter(|span| span.name.starts_with("benchmark_measurement"))
        .max_by(|left, right| left.duration_us.total_cmp(&right.duration_us))
        .ok_or_else(|| io::Error::other("trace has no benchmark_measurement span"))?;

    let mut durations: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let measurement_end = measurement.start_us + measurement.duration_us;
    for span in &spans {
        let end = span.start_us + span.duration_us;
        if span.start_us < measurement.start_us || end > measurement_end {
            continue;
        }
        if let Some(system) = system_name(&span.name) {
            durations
                .entry(system)
                .or_default()
                .push(span.duration_us / 1000.0);
        }
    }
    if durations.is_empty() {
        return Err(io::Error::other(
            "measurement contains no Bevy system spans; ensure profile-chrome is enabled",
        ));
    }
    let mut systems: Vec<_> = durations
        .into_iter()
        .map(|(system, values)| SystemProfile {
            system,
            calls: values.len(),
            total_ms: values.iter().sum(),
            durations_ms: Stats::new(values.into_iter()),
        })
        .collect();
    systems.sort_by(|left, right| right.total_ms.total_cmp(&left.total_ms));

    let run = benchmark
        .runs
        .first()
        .ok_or_else(|| io::Error::other("benchmark run contains no case"))?;
    let case_id = format!(
        "{}/{}/{}x2/{}t",
        run.mode,
        run.workload.as_str(),
        run.per_team,
        run.ticks
    );
    let artifact = ProfileArtifact {
        kind: "profile".into(),
        schema_version: REPORT_SCHEMA_VERSION,
        metadata: benchmark.metadata,
        case_id: case_id.clone(),
        measurement_wall_ms: measurement.duration_us / 1000.0,
        systems,
    };

    let directory = output.unwrap_or_else(|| {
        trace
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    });
    fs::create_dir_all(&directory)?;
    serde_json::to_writer_pretty(
        BufWriter::new(File::create(directory.join("profile-summary.json"))?),
        &artifact,
    )?;
    write_markdown(&directory, &artifact)?;
    if record_history {
        let path = record_json_artifact("profile", &artifact.metadata, &case_id, &artifact)?;
        println!("Profile history: {}", path.display());
    }
    Ok(directory)
}

fn write_markdown(directory: &Path, artifact: &ProfileArtifact) -> io::Result<()> {
    let mut markdown = BufWriter::new(File::create(directory.join("profile-report.md"))?);
    writeln!(
        markdown,
        "# RRTS system profile\n\nCase: `{}` · measurement wall time: {:.3} ms · machine: `{}`\n\nThese instrumented values describe where time was spent; they are not benchmark timings and never produce a regression verdict. Parallel system totals may overlap and must not be added to obtain frame wall time.\n\n| System | Calls | Total ms | Mean ms | p50 ms | p95 ms | Max ms |\n|---|---:|---:|---:|---:|---:|---:|",
        artifact.case_id, artifact.measurement_wall_ms, artifact.metadata.machine_key,
    )?;
    for system in &artifact.systems {
        writeln!(
            markdown,
            "| `{}` | {} | {:.3} | {:.4} | {:.4} | {:.4} | {:.4} |",
            system.system,
            system.calls,
            system.total_ms,
            system.durations_ms.mean,
            system.durations_ms.p50,
            system.durations_ms.p95,
            system.durations_ms.max,
        )?;
    }
    Ok(())
}

fn parse_spans(trace: TraceRoot) -> Vec<Span> {
    let events = match trace {
        TraceRoot::Events(events)
        | TraceRoot::Wrapped {
            trace_events: events,
        } => events,
    };
    let mut open: HashMap<(i64, i64), Vec<(String, f64)>> = HashMap::new();
    let mut spans = Vec::new();
    for event in events {
        let key = (event.pid, event.tid);
        match event.ph.as_str() {
            "X" => {
                if let Some(duration) = event.dur {
                    spans.push(Span {
                        name: event.name,
                        start_us: event.ts,
                        duration_us: duration,
                    });
                }
            }
            "B" => open.entry(key).or_default().push((event.name, event.ts)),
            "E" => {
                if let Some((name, start_us)) = open.get_mut(&key).and_then(Vec::pop) {
                    spans.push(Span {
                        name,
                        start_us,
                        duration_us: (event.ts - start_us).max(0.0),
                    });
                }
            }
            _ => {}
        }
    }
    spans
}

fn system_name(name: &str) -> Option<String> {
    let value = name.strip_prefix("system:")?.trim();
    let value = value.strip_prefix("name=").unwrap_or(value).trim();
    Some(value.trim_matches('"').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_threaded_and_complete_spans() {
        let trace: TraceRoot = serde_json::from_value(serde_json::json!([
            {"name":"benchmark_measurement: workload=skirmish", "ph":"B", "ts":10.0, "pid":1, "tid":1},
            {"name":"system: name=acquire_targets", "ph":"X", "ts":20.0, "dur":5.0, "pid":1, "tid":2},
            {"name":"system: name=move_units", "ph":"B", "ts":30.0, "pid":1, "tid":3},
            {"name":"system: name=move_units", "ph":"E", "ts":37.0, "pid":1, "tid":3},
            {"name":"benchmark_measurement", "ph":"E", "ts":50.0, "pid":1, "tid":1}
        ]))
        .unwrap();
        let spans = parse_spans(trace);
        assert_eq!(spans.len(), 3);
        assert_eq!(
            system_name("system: name=move_units").as_deref(),
            Some("move_units")
        );
    }
}
