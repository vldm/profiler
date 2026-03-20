use std::collections::HashMap;
use std::fs::{self, File};
use std::io;

use serde::{Deserialize, Serialize};

use super::{MetricStats, path_to_string};

#[derive(Serialize, Deserialize)]
pub struct ReportSnapshot {
    pub schema_version: u32,
    pub group: Option<String>,
    pub name: String,
    pub metric_names: Vec<String>,
    pub paths: Vec<Vec<String>>,
    pub outer_iters: Vec<Vec<(usize, Vec<f64>)>>,
}

#[derive(Serialize, Deserialize)]
pub struct JsonSpanNode {
    pub name: String,
    pub samples: usize,
    pub stats: Vec<MetricStats>,
    pub children: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct JsonReport {
    pub group: Option<String>,
    pub name: String,
    pub metric_names: Vec<String>,
    pub nodes: HashMap<String, JsonSpanNode>,
    pub roots: Vec<String>,
}

pub fn write_snapshot<M: crate::Metrics>(
    report: &super::AnalyzedReport<M>,
    path: &std::path::Path,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut paths = Vec::new();
    let mut path_to_index = HashMap::new();
    let mut outer_iters = Vec::with_capacity(report.data.published.len());

    for group in &report.data.published {
        let mut group_events = Vec::with_capacity(group.events.len());
        for event in &group.events {
            let idx = *path_to_index.entry(event.path.clone()).or_insert_with(|| {
                let i = paths.len();
                paths.push(event.path.clone());
                i
            });
            let values = report.data.metrics.result_to_f64s(&event.result);
            group_events.push((idx, values));
        }
        outer_iters.push(group_events);
    }

    let snapshot = ReportSnapshot {
        schema_version: 2,
        group: report.data.group_name.clone(),
        name: report.data.bench_name.clone(),
        metric_names: report
            .metrics_info
            .iter()
            .map(|info| info.name.to_string())
            .collect(),
        paths,
        outer_iters,
    };
    let file = std::io::BufWriter::new(File::create(path)?);
    serde_json::to_writer(file, &snapshot).map_err(io::Error::other)
}

pub fn write_aggregated_json<M: crate::Metrics>(
    report: &super::AnalyzedReport<M>,
    path: &std::path::Path,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut json_nodes = HashMap::new();
    let n_metrics = report.metrics_info.len();
    for (key, node) in &report.nodes {
        let mut stats = Vec::with_capacity(n_metrics);
        for metric_idx in 0..n_metrics {
            let vals: Vec<f64> = node
                .samples
                .iter()
                .filter_map(|s| s.get(metric_idx).copied())
                .collect();
            stats.push(MetricStats::from_values(&vals));
        }
        let key_str = path_to_string(key);
        json_nodes.insert(
            key_str,
            JsonSpanNode {
                name: node.name.clone(),
                samples: node.samples.len(),
                stats,
                children: report
                    .sorted_child_keys(key)
                    .into_iter()
                    .map(|child| path_to_string(&child))
                    .collect(),
            },
        );
    }

    let json_report = JsonReport {
        group: report.data.group_name.clone(),
        name: report.data.bench_name.clone(),
        metric_names: report
            .metrics_info
            .iter()
            .map(|info| info.name.to_string())
            .collect(),
        nodes: json_nodes,
        roots: report.roots.iter().map(|root| path_to_string(root)).collect(),
    };

    let file = File::create(path)?;
    serde_json::to_writer_pretty(file, &json_report).map_err(io::Error::other)
}

pub fn read_aggregated_json(path: &std::path::Path) -> io::Result<JsonReport> {
    let file = File::open(path)?;
    serde_json::from_reader(file).map_err(io::Error::other)
}
