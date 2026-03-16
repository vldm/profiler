use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use tracing::Id;

use crate::{Metrics, ProfileEntry};

/// One node in the aggregated span tree, keyed by (name, parent_key).
pub mod json;
pub enum JsonFile {
    Snapshot,
    Aggregated,
}
impl JsonFile {
    pub fn filename(&self) -> &'static str {
        match self {
            JsonFile::Snapshot => "events.json",
            JsonFile::Aggregated => "run.json",
        }
    }
}
pub struct SpanNode {
    pub name: String,
    /// `samples[i][j]` = value of metric `j` in sample `i`.
    pub samples: Vec<Vec<f64>>,
    pub children: Vec<String>, // child keys (ordered, deduplicated)
}

/// Statistics for one metric across samples.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct MetricStats {
    pub mean: f64,
    pub stddev: f64,
    pub min: f64,
    pub max: f64,
    pub median: f64,
}

impl MetricStats {
    pub fn from_values(values: &[f64]) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        let n = values.len() as f64;
        let sum: f64 = values.iter().sum();
        let mean = sum / n;
        let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
        let stddev = variance.sqrt();

        let mut sorted = values.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let min = sorted[0];
        let max = *sorted.last().unwrap();
        let median = if sorted.len().is_multiple_of(2) {
            let m = sorted.len() / 2;
            (sorted[m - 1] + sorted[m]) / 2.0
        } else {
            sorted[sorted.len() / 2]
        };

        Self {
            mean,
            stddev,
            min,
            max,
            median,
        }
    }

    /// Relative spread: stddev / mean (as a fraction, e.g. 0.05 = 5%).
    fn spread(&self) -> f64 {
        if self.mean.abs() < f64::EPSILON {
            0.0
        } else {
            self.stddev / self.mean
        }
    }
}

// ── Report ─────────────────────────────────────────────────────

pub struct RawData<M: Metrics> {
    pub metrics: Arc<M>,
    pub group_name: Option<String>,
    pub bench_name: String,
    pub published: Vec<(Vec<String>, M::Result)>,
}

pub struct AnalyzedReport<M: Metrics> {
    pub data: RawData<M>,
    pub metric_names: Vec<String>,
    pub nodes: HashMap<String, SpanNode>,
    pub roots: Vec<String>,
}

const LABEL_W: usize = 34;
const COL_W: usize = 20;
const COL_GAP: usize = 5;

/// Width of the full table for `n_metrics` columns. Use this when printing
/// separators outside of `Report::print()` (e.g. in the bench runner).
pub fn table_width(n_metrics: usize) -> usize {
    LABEL_W + (COL_W + COL_GAP) * n_metrics
}

#[derive(Clone, Copy)]
struct PrintLayout {
    label_w: usize,
    col_w: usize,
    col_gap: usize,
}

impl<M: Metrics> AnalyzedReport<M> {
    /// Build a report from raw `ProfileEntry` events produced by the Collector.
    ///
    /// Entries with the same span name under the same parent name are merged
    /// into a single node, so 100 iterations of `iter → parse → process`
    /// become one tree with 100 samples per node.
    pub fn from_profile_entries(
        entries: &[ProfileEntry<M::Start, M::Result>],
        metrics: Arc<M>,
        group_name: Option<String>,
        bench_name: String,
    ) -> Self
    where
        M::Result: Debug,
        M::Start: Debug,
    {
        let metric_names: Vec<String> = M::metrics_names().iter().map(|s| s.to_string()).collect();

        fn compute_key(
            id: &Id,
            span_info: &HashMap<Id, (&'static str, Option<Id>)>,
        ) -> Vec<&'static str> {
            let (name, parent) = match span_info.get(id) {
                Some(v) => v,
                None => return vec!["?"],
            };
            match parent {
                Some(pid) => {
                    let parent_key = compute_key(&pid, span_info);
                    let mut key = parent_key;
                    key.push(name);
                    key
                }
                None => vec![*name],
            }
        }

        // Collect results for each span, underway converting from span id to path.
        let mut published = Vec::new();

        // list of spans currently "in flight"
        let mut span_frame: HashMap<Id, (&'static str, Option<Id>)> = HashMap::new();
        for entry in entries {
            match entry {
                // Phase 1: map span Id → (name, parent_id) from Register entries.
                ProfileEntry::Register {
                    id,
                    metadata,
                    parent,
                    ..
                } => {
                    span_frame.insert(
                        id.clone(),
                        (
                            metadata.map(|meta| meta.name()).unwrap_or("unknown"),
                            parent.clone(),
                        ),
                    );
                }
                // Phase 2: for each Id compute its aggregation key (name path).
                // Cache to avoid repeated traversal.
                ProfileEntry::Publish { id, result } => {
                    let path = compute_key(&id, &span_frame);
                    let owned_path: Vec<String> = path.into_iter().map(|s| s.to_string()).collect();
                    published.push((owned_path, result.clone()));
                    // it will be replaced anyway so we can free it early
                    span_frame.remove(id);
                }
            }
        }

        fn format_path(path: &[String]) -> String {
            path.join("/")
        }

        // Phase 3: aggregate Publish entries by key.
        let mut nodes: HashMap<String, SpanNode> = HashMap::new();
        let mut roots: Vec<String> = Vec::new();
        let mut root_set: HashSet<String> = HashSet::new();

        for (path, result) in &published {
            let values = metrics.result_to_f64s(result);

            let key = format_path(path);
            let name = path
                .last()
                .map(|s| s.clone())
                .unwrap_or_else(|| "?".to_string());

            nodes
                .entry(key.clone())
                .or_insert_with(|| SpanNode {
                    name: name.clone(),
                    samples: Vec::new(),
                    children: Vec::new(),
                })
                .samples
                .push(values);

            // Register parent→child relationship.
            let parts: Vec<&str> = key.rsplitn(2, '/').collect();
            if parts.len() == 2 {
                let parent_key = parts[1].to_string();
                let parent_name = parent_key
                    .rsplit('/')
                    .next()
                    .unwrap_or(parent_key.as_str())
                    .to_string();
                let pnode = nodes.entry(parent_key.clone()).or_insert_with(|| SpanNode {
                    name: parent_name,
                    samples: Vec::new(),
                    children: Vec::new(),
                });
                if !pnode.children.contains(&key) {
                    pnode.children.push(key.clone());
                }
            } else {
                // This is a root.
                if root_set.insert(key.clone()) {
                    roots.push(key.clone());
                }
            }
        }

        roots.sort();
        for node in nodes.values_mut() {
            node.children.sort();
        }

        Self {
            data: RawData {
                metrics,
                group_name,
                bench_name,
                published,
            },
            metric_names,
            nodes,
            roots,
        }
    }

    fn format_path(
        group_name: &Option<String>,
        bench_name: &String,
        json_file: JsonFile,
    ) -> PathBuf {
        let mut path = cargo_target_directory()
            .unwrap_or_else(|| PathBuf::from("target"))
            .join("profiler");
        if let Some(group) = &group_name {
            path = path.join(sanitize_path_segment(group));
        }
        path = path.join(sanitize_path_segment(bench_name));
        path.join(json_file.filename())
    }

    pub fn write_snapshot_to_default_path(&self) -> io::Result<PathBuf> {
        let path = Self::format_path(
            &self.data.group_name,
            &self.data.bench_name,
            JsonFile::Snapshot,
        );
        self.write_snapshot(&path)?;
        Ok(path)
    }

    pub fn write_snapshot(&self, path: impl AsRef<Path>) -> io::Result<()> {
        json::write_snapshot(self, path.as_ref())
    }

    pub fn write_aggregated_json_to_default_path(&self) -> io::Result<PathBuf> {
        let path = Self::format_path(
            &self.data.group_name,
            &self.data.bench_name,
            JsonFile::Aggregated,
        );
        self.write_aggregated_json(&path)?;
        Ok(path)
    }

    pub fn write_aggregated_json(&self, path: impl AsRef<Path>) -> io::Result<()> {
        json::write_aggregated_json(self, path.as_ref())
    }

    pub fn read_aggregated_json_from_default_path(&self) -> io::Result<json::JsonReport> {
        let path = Self::format_path(
            &self.data.group_name,
            &self.data.bench_name,
            JsonFile::Aggregated,
        );
        json::read_aggregated_json(&path)
    }
}

pub struct ReportPrinter<'a, M: Metrics> {
    pub report: &'a AnalyzedReport<M>,
    pub baseline: Option<&'a json::JsonReport>,
}

impl<'a, M: Metrics> ReportPrinter<'a, M> {
    pub fn print(&self) {
        if self.report.nodes.is_empty() {
            println!("No profiling data.");
            return;
        }

        let n_metrics = self.report.metric_names.len();
        let layout = PrintLayout {
            // Give flat path a bit more room maybe, but keep standard for now
            label_w: LABEL_W,
            col_w: COL_W,
            col_gap: COL_GAP,
        };
        let sep = "─".repeat(table_width(n_metrics));

        // Table header
        print!("{:<label_w$}", "", label_w = layout.label_w);
        for name in &self.report.metric_names {
            print!(
                "{:gap$}{:>w$}",
                "",
                name,
                gap = layout.col_gap,
                w = layout.col_w
            );
        }
        println!("\x1b[0m");
        println!("{}", sep);

        for root_key in &self.report.roots {
            self.print_node(root_key, layout);
        }
    }

    /// Print a collection of reports grouped, with group headers and separators.
    /// All output lives here — callers just pass in the reports.
    pub fn print_all(reports: &[(AnalyzedReport<M>, Option<json::JsonReport>)]) {
        if reports.is_empty() {
            return;
        }
        let n_metrics = reports[0].0.metric_names.len();
        let w = table_width(n_metrics);
        // let thick = "─".repeat(w);
        let thin = "- ".repeat(w / 2).trim_end().to_string();

        let mut current_group: Option<&Option<String>> = None;
        let mut bench_index_in_group: usize = 0;

        for (report, baseline) in reports {
            let group = &report.data.group_name;
            if current_group != Some(group) {
                if let Some(g) = group {
                    // println!("\n{}", thick);
                    println!("Group: \x1b[1;33m{}\x1b[0m", g);
                    // println!("{}", thick);
                }
                current_group = Some(group);
                bench_index_in_group = 0;
            } else if bench_index_in_group > 0 {
                println!("{}", thin);
            }
            let printer = ReportPrinter {
                report,
                baseline: baseline.as_ref(),
            };
            printer.print();
            bench_index_in_group += 1;
        }
    }

    fn format_flat_path(&self, key: &str, max_len: usize) -> String {
        let mut parts: Vec<&str> = key.split('/').collect();
        if !parts.is_empty() {
            parts[0] = &self.report.data.bench_name;
        }

        let n = parts.len();
        if n == 0 {
            return String::new();
        }

        let root = parts[0];
        let leaf = parts[n - 1];

        if n == 1 {
            if root.chars().count() > max_len {
                let trunc: String = root.chars().take(max_len.saturating_sub(3)).collect();
                return format!("{}...", trunc);
            }
            return root.to_string();
        }

        // Try full path first
        let full = parts.join("/");
        if full.chars().count() <= max_len {
            return full;
        }

        // Try collapsing intermediate parents: root/{depth}/leaf
        let depth_str = if n > 2 {
            format!("/{{{}}}/", n - 2)
        } else {
            "/".to_string()
        };

        let root_chars = root.chars().count();
        let leaf_chars = leaf.chars().count();
        let depth_chars = depth_str.chars().count();

        if root_chars + depth_chars + leaf_chars <= max_len {
            return format!("{}{}{}", root, depth_str, leaf);
        }

        // Truncate root if necessary, but keep at least a bit of it if possible
        let available_for_root = max_len.saturating_sub(depth_chars + leaf_chars);
        if available_for_root > 3 {
            let t_root: String = root.chars().take(available_for_root - 3).collect();
            return format!("{}...{}{}", t_root, depth_str, leaf);
        }

        // If leaf alone is too big (plus prefix), truncate leaf
        let prefix = format!("{}...", &root[..1.min(root.len())]); // Just "R..."
        let prefix_chars = prefix.chars().count() + depth_chars;

        let available_for_leaf = max_len.saturating_sub(prefix_chars);
        if available_for_leaf > 3 {
            let t_leaf: String = leaf.chars().take(available_for_leaf - 3).collect();
            return format!("{}{}{}...", prefix, depth_str, t_leaf);
        }

        // Extreme fallback
        let trunc: String = full.chars().take(max_len.saturating_sub(3)).collect();
        format!("{}...", trunc)
    }

    fn print_node(&self, key: &str, layout: PrintLayout) {
        let node = match self.report.nodes.get(key) {
            Some(n) => n,
            None => return,
        };

        if node.samples.is_empty() {
            return;
        }

        let stats = self.compute_metric_stats(node);
        let n_samples = node.samples.len();

        let suffix = format!("  ({})", n_samples);
        let suffix_len = suffix.chars().count();
        let max_path_len = layout.label_w.saturating_sub(suffix_len);

        let path_str = self.format_flat_path(key, max_path_len);
        let mut label = format!("{}{}", path_str, suffix);

        // Ensure the label does not exceed label_w (should be guaranteed by format_flat_path, but safety first)
        if label.chars().count() > layout.label_w {
            let trunc: String = label.chars().take(layout.label_w - 3).collect();
            label = format!("{}...", trunc);
        }

        let baseline_stats = self
            .baseline
            .and_then(|b| b.nodes.get(key).map(|n| &n.stats));

        // Row 1: name(count) and median ± spread%
        print!(
            "\x1b[1m{:<label_w$}\x1b[0m",
            label,
            label_w = layout.label_w
        );
        for (metric_idx, s) in stats.iter().enumerate() {
            if let Some(b) = baseline_stats.and_then(|bs| bs.get(metric_idx)) {
                let (val, unit) = self.report.data.metrics.format_value(metric_idx, b.median);
                let spread_pct = b.spread() * 100.0;
                let cell = format!("b: {}{} ± {:.0}%", val, unit, spread_pct);
                print!(
                    "\x1b[1m{:gap$}{:>w$}\x1b[0m",
                    "",
                    cell,
                    gap = layout.col_gap,
                    w = layout.col_w
                );
            } else {
                let (val, unit) = self.report.data.metrics.format_value(metric_idx, s.median);
                let spread_pct = s.spread() * 100.0;
                let cell = format!("{}{} ± {:.0}%", val, unit, spread_pct);
                print!(
                    "\x1b[1m{:gap$}{:>w$}\x1b[0m",
                    "",
                    cell,
                    gap = layout.col_gap,
                    w = layout.col_w
                );
            }
        }
        println!();

        // Row 2: Either difference to baseline or skip
        if baseline_stats.is_some() {
            let detail_tree = String::new();
            print!("{:<label_w$}", detail_tree, label_w = layout.label_w);
            for (metric_idx, s) in stats.iter().enumerate() {
                if let Some(b) = baseline_stats.and_then(|bs| bs.get(metric_idx)) {
                    let (val, unit) = self.report.data.metrics.format_value(metric_idx, s.median);
                    let spread_pct = s.spread() * 100.0;

                    let diff_pct = if b.median.abs() > f64::EPSILON {
                        (s.median - b.median) / b.median * 100.0
                    } else {
                        0.0
                    };

                    let color = if diff_pct < -1.5 {
                        "\x1b[32m"
                    } else if diff_pct > 1.5 {
                        "\x1b[31m"
                    } else {
                        ""
                    };
                    let reset = if color.is_empty() { "" } else { "\x1b[0m" };
                    let diff_str = if diff_pct > 0.0 {
                        format!("+{:.2}%", diff_pct)
                    } else {
                        format!("{:.2}%", diff_pct)
                    };

                    let raw_cell = format!("{}{} ± {:.0}% ({})", val, unit, spread_pct, diff_str);
                    let colored_cell = format!(
                        "{}{} ± {:.0}% ({}{}{})",
                        val, unit, spread_pct, color, diff_str, reset
                    );

                    let cell_len = raw_cell.chars().count();
                    let padding = layout.col_w.saturating_sub(cell_len);
                    print!(
                        "{:gap$}{:pad$}{}",
                        "",
                        "",
                        colored_cell,
                        gap = layout.col_gap,
                        pad = padding
                    );
                } else {
                    let (val, unit) = self.report.data.metrics.format_value(metric_idx, s.median);
                    let spread_pct = s.spread() * 100.0;
                    let raw_cell = format!("{}{} ± {:.0}%", val, unit, spread_pct);
                    let cell_len = raw_cell.chars().count();
                    let padding = layout.col_w.saturating_sub(cell_len);
                    print!(
                        "{:gap$}{:pad$}{}",
                        "",
                        "",
                        raw_cell,
                        gap = layout.col_gap,
                        pad = padding
                    );
                }
            }
            println!();
        }

        // Row 3 (or 2 if no baseline): compact [min..max]
        {
            let detail_tree = String::new();
            print!("{:<label_w$}", detail_tree, label_w = layout.label_w);
            for (metric_idx, s) in stats.iter().enumerate() {
                let range = self.format_compact_range(metric_idx, s.min, s.max);
                let cell = format!("\x1b[2m{}\x1b[0m", range);
                let padding = layout.col_w.saturating_sub(range.chars().count());
                print!(
                    "{:gap$}{:pad$}{}",
                    "",
                    "",
                    cell,
                    gap = layout.col_gap,
                    pad = padding
                );
            }
            println!();
        }

        for child_key in &node.children {
            self.print_node(child_key, layout);
        }
    }

    fn compute_metric_stats(&self, node: &SpanNode) -> Vec<MetricStats> {
        let n_metrics = self.report.metric_names.len();
        (0..n_metrics)
            .map(|metric_idx| {
                let vals: Vec<f64> = node
                    .samples
                    .iter()
                    .filter_map(|sample| sample.get(metric_idx).copied())
                    .collect();
                MetricStats::from_values(&vals)
            })
            .collect()
    }

    fn format_compact_range(&self, metric_idx: usize, min: f64, max: f64) -> String {
        let (lo, u1) = self.report.data.metrics.format_value(metric_idx, min);
        let (hi, u2) = self.report.data.metrics.format_value(metric_idx, max);

        if u1 == u2 {
            if u1.is_empty() {
                format!("[{} .. {}]", lo, hi)
            } else {
                format!("[{} .. {}{}]", lo, hi, u1)
            }
        } else {
            format!("[{}{} .. {}{}]", lo, u1, hi, u2)
        }
    }
}
fn sanitize_path_segment(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c,
            _ => '_',
        })
        .collect();
    if sanitized.is_empty() {
        "unnamed".to_string()
    } else {
        sanitized
    }
}

fn cargo_target_directory() -> Option<PathBuf> {
    #[derive(serde::Deserialize)]
    struct Metadata {
        target_directory: PathBuf,
    }

    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            let output =
                std::process::Command::new(std::env::var_os("CARGO").unwrap_or("cargo".into()))
                    .args(["metadata", "--format-version", "1"])
                    .output()
                    .ok()?;
            let metadata: Metadata = serde_json::from_slice(&output.stdout).ok()?;
            Some(metadata.target_directory)
        })
}
