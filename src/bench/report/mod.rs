use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Debug,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use tracing::Id;

use crate::{Metrics, ProfileEntry, metrics::MetricReportInfo};

type PathKey = Vec<String>;

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
#[derive(Debug)]
pub struct SpanNode {
    pub name: String,
    /// `samples[i][j]` = value of metric `j` in sample `i`.
    pub samples: Vec<Vec<f64>>,
}

#[derive(Clone)]
pub struct PublishedEvent<R> {
    pub path: Vec<String>,
    pub result: R,
}

#[derive(Clone)]
pub struct PublishedRoot<R> {
    pub root_id: Id,
    pub events: Vec<PublishedEvent<R>>,
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
    pub published: Vec<PublishedRoot<M::Result>>,
}

pub struct AnalyzedReport<M: Metrics> {
    pub data: RawData<M>,
    pub metrics_info: &'static [crate::metrics::MetricReportInfo],
    pub nodes: BTreeMap<PathKey, SpanNode>,
    pub roots: Vec<PathKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalysisPhase {
    FillPublished,
    AggregatePublished,
}

impl AnalysisPhase {
    pub fn label(self) -> &'static str {
        match self {
            AnalysisPhase::FillPublished => "raw spans",
            AnalysisPhase::AggregatePublished => "aggregate spans",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnalysisProgressState {
    pub phase: AnalysisPhase,
    pub completed: usize,
    pub total: usize,
}

pub trait AnalysisProgress {
    fn update(&mut self, state: AnalysisProgressState);
}

const PRIMARY_METRIC_IDX: usize = 0;
const LABEL_W: usize = 38;
const COL_W: usize = 24;
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
        Self::from_profile_entries_with_progress(entries, metrics, group_name, bench_name, None)
    }

    pub fn from_profile_entries_with_progress(
        entries: &[ProfileEntry<M::Start, M::Result>],
        metrics: Arc<M>,
        group_name: Option<String>,
        bench_name: String,
        mut progress: Option<&mut dyn AnalysisProgress>,
    ) -> Self
    where
        M::Result: Debug,
        M::Start: Debug,
    {
        fn entry_id<Start, Result>(entry: &ProfileEntry<Start, Result>) -> &Id {
            match entry {
                ProfileEntry::Register { id, .. } | ProfileEntry::Publish { id, .. } => id,
            }
        }

        fn collect_entry<M: Metrics>(
            entry: &ProfileEntry<M::Start, M::Result>,
            root_id: &Id,
            span_frame: &mut HashMap<Id, Vec<&'static str>>,
            current_group: &mut Vec<PublishedEvent<M::Result>>,
        ) -> bool
        where
            M::Result: Clone,
        {
            match entry {
                ProfileEntry::Register {
                    id,
                    metadata,
                    parent,
                    ..
                } => {
                    let mut parent_key = parent
                        .as_ref()
                        .map(|p| span_frame.get(p).cloned().unwrap_or_else(|| vec!["?"]))
                        .unwrap_or_default();
                    parent_key.push(metadata.map(|meta| meta.name()).unwrap_or("unknown"));

                    span_frame.insert(id.clone(), parent_key);
                    false
                }
                ProfileEntry::Publish { id, result } => {
                    let path = span_frame.get(id).cloned().unwrap_or_else(|| vec!["?"]);

                    current_group.push(PublishedEvent {
                        path: path.into_iter().map(|s| s.to_string()).collect(),
                        result: result.clone(),
                    });

                    id == root_id
                }
            }
        }

        // Collect results for each span, grouping outer iterations from the
        // root Register entry until the matching root Publish entry.
        let mut published = Vec::new();

        // list of spans currently "in flight"
        let mut span_frame: HashMap<Id, Vec<&'static str>> = HashMap::new();
        publish_progress(
            &mut progress,
            AnalysisPhase::FillPublished,
            0,
            entries.len(),
        );
        let mut entries_iter = entries.iter().enumerate();
        while let Some((_, root_entry)) = entries_iter.next() {
            let root_id = entry_id(root_entry).clone();

            let mut current_group = Vec::new();
            let publish =
                collect_entry::<M>(root_entry, &root_id, &mut span_frame, &mut current_group);
            assert!(
                !publish,
                "Expected root entry to be a Register, but got Publish: {:?}",
                root_entry
            );
            for (idx, entry) in &mut entries_iter {
                let root_closed =
                    collect_entry::<M>(entry, &root_id, &mut span_frame, &mut current_group);
                publish_progress(
                    &mut progress,
                    AnalysisPhase::FillPublished,
                    idx + 1,
                    entries.len(),
                );

                if root_closed {
                    published.push(PublishedRoot {
                        root_id: root_id.clone(),
                        events: current_group,
                    });
                    break;
                }
            }
        }

        Self::from_grouped_published_entries_with_progress(
            published, metrics, group_name, bench_name, progress,
        )
    }

    fn from_grouped_published_entries_with_progress(
        published: Vec<PublishedRoot<M::Result>>,
        metrics: Arc<M>,
        group_name: Option<String>,
        bench_name: String,
        mut progress: Option<&mut dyn AnalysisProgress>,
    ) -> Self {
        let mut nodes: BTreeMap<PathKey, SpanNode> = BTreeMap::new();
        let mut root_set: HashSet<PathKey> = HashSet::new();
        let total_published: usize = published.iter().map(|group| group.events.len()).sum();
        let total_outer_iters = published.len();
        let n_metrics = M::metrics_info().len();
        let metrics_info = M::metrics_info();

        publish_progress(
            &mut progress,
            AnalysisPhase::AggregatePublished,
            0,
            total_published,
        );
        let mut aggregated = 0usize;
        for (outer_iter_idx, group) in published.iter().enumerate() {
            for entry in &group.events {
                let values = metrics.result_to_f64s(&entry.result);

                let key = entry.path.clone();
                ensure_parent_chain(&mut nodes, &key);
                let node = nodes.entry(key.clone()).or_insert_with(|| SpanNode {
                    name: path_name(&key),
                    samples: Vec::new(),
                });
                merge_outer_iter_sample(node, outer_iter_idx, &values, n_metrics, metrics_info);

                if parent_path(&key).is_none() {
                    root_set.insert(key.clone());
                }

                aggregated += 1;
                publish_progress(
                    &mut progress,
                    AnalysisPhase::AggregatePublished,
                    aggregated,
                    total_published,
                );
            }
        }

        extend_all_samples_to_outer_iters(&mut nodes, total_outer_iters, n_metrics);

        let unknown_root = vec!["?".to_string()];
        if nodes.contains_key(unknown_root.as_slice()) {
            root_set.insert(unknown_root.clone()); // add "unknown" root for spans without Register entry
        }
        let mut roots = root_set.into_iter().collect::<Vec<_>>();

        roots.sort_by(|a, b| compare_node_keys_by_primary_metric(a, b, &nodes));

        Self {
            data: RawData {
                metrics,
                group_name,
                bench_name,
                published,
            },
            metrics_info: M::metrics_info(),
            nodes,
            roots,
        }
    }

    fn format_path(
        filename: &str,
        group_name: &Option<String>,
        bench_name: &str,
        json_file: JsonFile,
    ) -> PathBuf {
        let mut path = cargo_target_directory()
            .unwrap_or_else(|| PathBuf::from("target"))
            .join("profiler")
            .join(sanitize_path_segment(filename));
        if let Some(group) = &group_name {
            path = path.join(sanitize_path_segment(group));
        }
        path = path.join(sanitize_path_segment(bench_name));
        path.join(json_file.filename())
    }

    pub fn write_snapshot_to_default_path(&self, filename: &str) -> io::Result<PathBuf> {
        let path = Self::format_path(
            filename,
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

    pub fn write_aggregated_json_to_default_path(&self, filename: &str) -> io::Result<PathBuf> {
        let path = Self::format_path(
            filename,
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

    pub fn read_aggregated_json_from_default_path(
        &self,
        filename: &str,
    ) -> io::Result<json::JsonReport> {
        let path = Self::format_path(
            filename,
            &self.data.group_name,
            &self.data.bench_name,
            JsonFile::Aggregated,
        );
        json::read_aggregated_json(&path)
    }
}

fn publish_progress(
    progress: &mut Option<&mut dyn AnalysisProgress>,
    phase: AnalysisPhase,
    completed: usize,
    total: usize,
) {
    if let Some(progress) = progress.as_deref_mut() {
        progress.update(AnalysisProgressState {
            phase,
            completed,
            total,
        });
    }
}

fn merge_metric_samples(sample: &mut [f64], values: &[f64], metrics_info: &[MetricReportInfo]) {
    for (metric_idx, value) in values.iter().copied().enumerate() {
        match metrics_info
            .get(metric_idx)
            .map(|info| info.aggregation)
            .unwrap_or(crate::metrics::MetricAggregation::Sum)
        {
            crate::metrics::MetricAggregation::Sum => sample[metric_idx] += value,
            crate::metrics::MetricAggregation::Max => {
                sample[metric_idx] = sample[metric_idx].max(value);
            }
        }
    }
}

fn path_name(path: &[String]) -> String {
    path.last().cloned().unwrap_or_else(|| "?".to_string())
}

fn path_to_string(path: &[String]) -> String {
    path.join("/")
}

fn parent_path(path: &[String]) -> Option<PathKey> {
    (path.len() > 1).then(|| path[..path.len() - 1].to_vec())
}

fn ensure_parent_chain(nodes: &mut BTreeMap<PathKey, SpanNode>, path: &[String]) {
    for depth in 1..path.len() {
        let parent = path[..depth].to_vec();
        nodes.entry(parent.clone()).or_insert_with(|| SpanNode {
            name: path_name(&parent),
            samples: Vec::new(),
        });
    }
}

fn merge_outer_iter_sample(
    node: &mut SpanNode,
    outer_iter_idx: usize,
    values: &[f64],
    n_metrics: usize,
    metrics_info: &[MetricReportInfo],
) {
    // extend in case of skipped outer iterations.
    if node.samples.len() < outer_iter_idx {
        node.samples.resize(outer_iter_idx, vec![0.0; n_metrics]);
    }

    // push instead of merge to avoid affecting of Max in negative values
    if node.samples.len() == outer_iter_idx {
        node.samples.push(values.to_vec());
        return;
    }

    merge_metric_samples(&mut node.samples[outer_iter_idx], values, metrics_info);
}

fn extend_all_samples_to_outer_iters(
    nodes: &mut BTreeMap<PathKey, SpanNode>,
    total_outer_iters: usize,
    n_metrics: usize,
) {
    for node in nodes.values_mut() {
        if node.samples.len() < total_outer_iters {
            node.samples.resize(total_outer_iters, vec![0.0; n_metrics]);
        }
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

        let n_metrics = self.report.metrics_info.len();
        let layout = PrintLayout {
            // Give flat path a bit more room maybe, but keep standard for now
            label_w: LABEL_W,
            col_w: COL_W,
            col_gap: COL_GAP,
        };
        let sep = "─".repeat(table_width(n_metrics));

        // Table header
        print!("{:<label_w$}", "", label_w = layout.label_w);
        for info in self.report.metrics_info {
            print!(
                "{:gap$}{:>w$}",
                "",
                info.name,
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
        let n_metrics = reports[0].0.metrics_info.len();
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

    fn format_flat_path(&self, key: &[String], max_len: usize) -> String {
        let mut parts: Vec<&str> = key.iter().map(String::as_str).collect();

        match parts.len() {
            1.. if parts[0] == "?" => {} // skip
            1 => parts[0] = &self.report.data.bench_name,
            2.. => {
                parts.remove(0);
            } // remove common root
            _ => {}
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

    fn print_node(&self, key: &[String], layout: PrintLayout) {
        let node = match self.report.nodes.get(key) {
            Some(n) => n,
            None => return,
        };

        if node.samples.is_empty() {
            for child_key in self.report.sorted_child_keys(key) {
                self.print_node(&child_key, layout);
            }
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

        let key_str = path_to_string(key);
        let baseline_stats = self
            .baseline
            .and_then(|b| b.nodes.get(&key_str).map(|n| n.stats.as_slice()));

        // Row 1: name ... and baseline: median ± spread% for each metric
        print!(
            "\x1b[1m{:<label_w$}\x1b[0m",
            label,
            label_w = layout.label_w
        );

        for (metric_idx, _s) in stats.iter().enumerate() {
            let baseline_enabled = self
                .report
                .metrics_info
                .get(metric_idx)
                .is_some_and(|info| info.show_baseline);

            if let Some(b) = baseline_stats.and_then(|bs| bs.get(metric_idx))
                && baseline_enabled
            {
                let (val, unit) = self.report.data.metrics.format_value(metric_idx, b.mean);
                let spread_pct = b.spread() * 100.0;
                let cell = format!("baseline: {}{} ± {:.0}%", val, unit, spread_pct);
                print!(
                    "\x1b[1m{:gap$}{:>w$}\x1b[0m",
                    "",
                    cell,
                    gap = layout.col_gap,
                    w = layout.col_w
                );
            } else {
                print!("{:gap$}", "", gap = layout.col_gap + layout.col_w);
            }
        }
        println!();

        let parent_share_label = self
            .parent_share_text(key)
            .map(|text| format!("  {}", text));

        for (idx, row) in self
            .detail_rows_for_node(&stats, baseline_stats)
            .into_iter()
            .enumerate()
        {
            let label = if idx == 0 {
                parent_share_label.as_deref().unwrap_or("")
            } else {
                ""
            };
            self.print_labeled_detail_row(label, &row, layout);
        }

        for child_key in self.report.sorted_child_keys(key) {
            self.print_node(&child_key, layout);
        }
    }

    fn print_labeled_detail_row(&self, label: &str, row: &[String], layout: PrintLayout) {
        let label_padding = layout.label_w.saturating_sub(label.chars().count());

        print!("{}{:pad$}", label, "", pad = label_padding);

        for cell in row {
            let padding = layout.col_w.saturating_sub(visible_width(cell));
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

    fn detail_rows_for_node(
        &self,
        stats: &[MetricStats],
        baseline_stats: Option<&[MetricStats]>,
    ) -> Vec<Vec<String>> {
        vec![
            self.baseline_detail_cells(stats, baseline_stats),
            self.range_detail_cells(stats),
        ]
    }

    fn parent_share_text(&self, key: &[String]) -> Option<String> {
        let parent_key = parent_path(key)?;
        let node = self.report.nodes.get(key)?;
        let parent = self.report.nodes.get(parent_key.as_slice());
        let child_median = primary_metric_median(&node.samples);
        let metric_name = self
            .report
            .metrics_info
            .get(PRIMARY_METRIC_IDX)
            .map(|info| info.name)
            .unwrap_or("metric");

        let text = match parent.map(|parent| primary_metric_median(&parent.samples)) {
            Some(parent_median) if parent_median.abs() > f64::EPSILON => {
                format!(
                    "{:.0}% {} of parent",
                    child_median / parent_median * 100.0,
                    metric_name
                )
            }
            _ => format!("n/a {} of parent", metric_name),
        };

        Some(text)
    }

    fn baseline_detail_cells(
        &self,
        stats: &[MetricStats],
        baseline_stats: Option<&[MetricStats]>,
    ) -> Vec<String> {
        stats
            .iter()
            .enumerate()
            .map(|(metric_idx, s)| {
                let baseline_enabled = self
                    .report
                    .metrics_info
                    .get(metric_idx)
                    .is_some_and(|info| info.show_baseline);

                if let Some(b) = baseline_stats.and_then(|bs| bs.get(metric_idx))
                    && baseline_enabled
                {
                    let (val, unit) = self.report.data.metrics.format_value(metric_idx, s.mean);
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

                    format!(
                        "{}{} ± {:.0}% ({}{}{})",
                        val, unit, spread_pct, color, diff_str, reset
                    )
                } else {
                    let (val, unit) = self.report.data.metrics.format_value(metric_idx, s.mean);
                    let spread_pct = s.spread() * 100.0;
                    format!("{}{} ± {:.0}%", val, unit, spread_pct)
                }
            })
            .collect()
    }

    fn range_detail_cells(&self, stats: &[MetricStats]) -> Vec<String> {
        stats
            .iter()
            .enumerate()
            .map(|(metric_idx, s)| {
                let show_spread = self
                    .report
                    .metrics_info
                    .get(metric_idx)
                    .is_some_and(|info| info.show_spread);
                if show_spread {
                    let range = self.format_compact_range(metric_idx, s.min, s.max);
                    format!("\x1b[2m{}\x1b[0m", range)
                } else {
                    String::new()
                }
            })
            .collect()
    }

    fn compute_metric_stats(&self, node: &SpanNode) -> Vec<MetricStats> {
        compute_metric_stats_for_samples(&node.samples, self.report.metrics_info.len())
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

fn compute_metric_stats_for_samples(samples: &[Vec<f64>], n_metrics: usize) -> Vec<MetricStats> {
    (0..n_metrics)
        .map(|metric_idx| {
            let vals: Vec<f64> = samples
                .iter()
                .filter_map(|sample| sample.get(metric_idx).copied())
                .collect();
            MetricStats::from_values(&vals)
        })
        .collect()
}

fn primary_metric_median(samples: &[Vec<f64>]) -> f64 {
    compute_metric_stats_for_samples(samples, PRIMARY_METRIC_IDX + 1)
        .into_iter()
        .next()
        .unwrap_or_default()
        .median
}

fn compare_node_keys_by_primary_metric(
    a: &[String],
    b: &[String],
    nodes: &BTreeMap<PathKey, SpanNode>,
) -> std::cmp::Ordering {
    let a_metric = nodes
        .get(a)
        .map(|node| primary_metric_median(&node.samples))
        .unwrap_or_default();
    let b_metric = nodes
        .get(b)
        .map(|node| primary_metric_median(&node.samples))
        .unwrap_or_default();

    b_metric.total_cmp(&a_metric).then_with(|| a.cmp(b))
}

impl<M: Metrics> AnalyzedReport<M> {
    fn sorted_child_keys(&self, parent: &[String]) -> Vec<PathKey> {
        let mut children: Vec<PathKey> = self
            .nodes
            .range(parent.to_vec()..)
            .take_while(|(key, _)| key.starts_with(parent))
            .filter(|(key, _)| key.len() == parent.len() + 1)
            .map(|(key, _)| key.clone())
            .collect();
        children.sort_by(|a, b| compare_node_keys_by_primary_metric(a, b, &self.nodes));
        children
    }
}

fn visible_width(value: &str) -> usize {
    let mut width = 0;
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            let _ = chars.next();
            for esc in chars.by_ref() {
                if esc.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        width += 1;
    }

    width
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tracing::Id;

    use super::{
        AnalysisPhase, AnalysisProgress, AnalysisProgressState, AnalyzedReport, MetricStats,
        PathKey, PublishedEvent, PublishedRoot, ReportPrinter, SpanNode,
        compute_metric_stats_for_samples, json, path_to_string,
    };
    use crate::{Metrics, ProfileEntry};

    #[derive(Default)]
    struct TestMetrics;

    impl Metrics for TestMetrics {
        type Start = ();
        type Result = [f64; 2];

        fn start(&self) -> Self::Start {}

        fn end(&self, _start: Self::Start) -> Self::Result {
            [0.0, 0.0]
        }

        fn metrics_info() -> &'static [crate::metrics::MetricReportInfo] {
            &const {
                [
                    crate::metrics::MetricReportInfo::new("primary"),
                    crate::metrics::MetricReportInfo::new("secondary"),
                ]
            }
        }

        fn result_to_f64(&self, metric_idx: usize, result: &Self::Result) -> f64 {
            result[metric_idx]
        }

        fn format_value(&self, _metric_idx: usize, value: f64) -> (String, &'static str) {
            (format!("{value:.1}"), "")
        }
    }

    #[derive(Default)]
    struct PeakMetrics;

    impl Metrics for PeakMetrics {
        type Start = ();
        type Result = [f64; 2];

        fn start(&self) -> Self::Start {}

        fn end(&self, _start: Self::Start) -> Self::Result {
            [0.0, 0.0]
        }

        fn metrics_info() -> &'static [crate::metrics::MetricReportInfo] {
            &const {
                [
                    crate::metrics::MetricReportInfo::new("sum"),
                    crate::metrics::MetricReportInfo::new("peak")
                        .with_aggregation(crate::metrics::MetricAggregation::Max),
                ]
            }
        }

        fn result_to_f64(&self, metric_idx: usize, result: &Self::Result) -> f64 {
            result[metric_idx]
        }

        fn format_value(&self, _metric_idx: usize, value: f64) -> (String, &'static str) {
            (format!("{value:.1}"), "")
        }
    }

    fn report_from_outer_iters_with_metrics<Mt: Metrics<Result = [f64; 2], Start = ()>>(
        metrics: Arc<Mt>,
        outer_iters: Vec<Vec<(Vec<&str>, [f64; 2])>>,
    ) -> AnalyzedReport<Mt> {
        let published = outer_iters
            .into_iter()
            .enumerate()
            .map(|(outer_iter_idx, entries)| PublishedRoot {
                root_id: Id::from_u64(outer_iter_idx as u64 + 1),
                events: entries
                    .into_iter()
                    .map(|(path, result)| PublishedEvent {
                        path: path
                            .into_iter()
                            .map(|segment| segment.to_string())
                            .collect(),
                        result,
                    })
                    .collect(),
            })
            .collect();

        AnalyzedReport::from_grouped_published_entries_with_progress(
            published,
            metrics,
            None,
            "bench".to_string(),
            None,
        )
    }

    fn report_from_outer_iters(
        outer_iters: Vec<Vec<(Vec<&str>, [f64; 2])>>,
    ) -> AnalyzedReport<TestMetrics> {
        report_from_outer_iters_with_metrics(Arc::new(TestMetrics), outer_iters)
    }

    fn report_from_single_outer_iter(
        entries: Vec<(Vec<&str>, [f64; 2])>,
    ) -> AnalyzedReport<TestMetrics> {
        report_from_outer_iters(vec![entries])
    }

    fn stats(values: &[f64]) -> MetricStats {
        MetricStats::from_values(values)
    }

    fn path(segments: &[&str]) -> PathKey {
        segments
            .iter()
            .map(|segment| (*segment).to_string())
            .collect()
    }

    fn node<'a, M: Metrics>(report: &'a AnalyzedReport<M>, segments: &[&str]) -> &'a SpanNode {
        report.nodes.get(path(segments).as_slice()).unwrap()
    }

    fn child_paths<M: Metrics>(report: &AnalyzedReport<M>, segments: &[&str]) -> Vec<String> {
        report
            .sorted_child_keys(path(segments).as_slice())
            .into_iter()
            .map(|child| path_to_string(&child))
            .collect()
    }

    #[derive(Default)]
    struct CapturedProgress {
        updates: Vec<AnalysisProgressState>,
    }

    impl AnalysisProgress for CapturedProgress {
        fn update(&mut self, state: AnalysisProgressState) {
            self.updates.push(state);
        }
    }

    #[test]
    fn reports_analysis_progress_for_both_internal_phases() {
        let id = Id::from_u64(1);
        let entries = vec![
            ProfileEntry::Register {
                id: id.clone(),
                metadata: None,
                parent: None,
                start: (),
            },
            ProfileEntry::Publish {
                id,
                result: [10.0, 20.0],
            },
        ];
        let mut progress = CapturedProgress::default();

        let _ = AnalyzedReport::from_profile_entries_with_progress(
            &entries,
            Arc::new(TestMetrics),
            None,
            "bench".to_string(),
            Some(&mut progress),
        );

        assert_eq!(
            progress.updates.first().copied(),
            Some(AnalysisProgressState {
                phase: AnalysisPhase::FillPublished,
                completed: 0,
                total: 2,
            })
        );
        assert!(progress.updates.contains(&AnalysisProgressState {
            phase: AnalysisPhase::FillPublished,
            completed: 2,
            total: 2,
        }));
        assert!(progress.updates.contains(&AnalysisProgressState {
            phase: AnalysisPhase::AggregatePublished,
            completed: 0,
            total: 1,
        }));
    }

    #[test]
    fn sorts_roots_by_primary_metric_descending() {
        let report = report_from_single_outer_iter(vec![
            (vec!["alpha"], [10.0, 500.0]),
            (vec!["beta"], [30.0, 100.0]),
            (vec!["gamma"], [20.0, 900.0]),
        ]);

        assert_eq!(
            report.roots,
            vec![path(&["beta"]), path(&["gamma"]), path(&["alpha"])]
        );
    }

    #[test]
    fn sorts_siblings_by_primary_metric_not_secondary_metric() {
        let report = report_from_single_outer_iter(vec![
            (vec!["root"], [100.0, 0.0]),
            (vec!["root", "alpha"], [10.0, 900.0]),
            (vec!["root", "beta"], [20.0, 100.0]),
        ]);

        assert_eq!(
            child_paths(&report, &["root"]),
            vec!["root/beta", "root/alpha"]
        );
    }

    #[test]
    fn breaks_primary_metric_ties_by_path() {
        let report = report_from_single_outer_iter(vec![
            (vec!["root"], [100.0, 0.0]),
            (vec!["root", "beta"], [20.0, 1.0]),
            (vec!["root", "alpha"], [20.0, 9.0]),
        ]);

        assert_eq!(
            child_paths(&report, &["root"]),
            vec!["root/alpha", "root/beta"]
        );
    }

    #[test]
    fn computes_parent_share_from_primary_metric_median() {
        let report = report_from_outer_iters(vec![
            vec![
                (vec!["root"], [10.0, 0.0]),
                (vec!["root", "child"], [5.0, 0.0]),
            ],
            vec![
                (vec!["root"], [30.0, 0.0]),
                (vec!["root", "child"], [15.0, 0.0]),
            ],
        ]);
        let printer = ReportPrinter {
            report: &report,
            baseline: None,
        };

        assert_eq!(printer.parent_share_text(path(&["root"]).as_slice()), None);
        assert_eq!(
            printer
                .parent_share_text(path(&["root", "child"]).as_slice())
                .as_deref(),
            Some("50% primary of parent")
        );
    }

    #[test]
    fn renders_parent_share_as_na_when_parent_primary_metric_is_zero() {
        let report = report_from_single_outer_iter(vec![
            (vec!["root"], [0.0, 0.0]),
            (vec!["root", "child"], [10.0, 0.0]),
        ]);
        let printer = ReportPrinter {
            report: &report,
            baseline: None,
        };

        assert_eq!(
            printer
                .parent_share_text(path(&["root", "child"]).as_slice())
                .as_deref(),
            Some("n/a primary of parent")
        );
    }

    #[test]
    fn parent_share_is_moved_into_label_and_detail_rows_remain_compact() {
        let report = report_from_single_outer_iter(vec![
            (vec!["root"], [40.0, 4.0]),
            (vec!["root", "child"], [20.0, 2.0]),
        ]);
        let baseline = json::JsonReport {
            group: None,
            name: "bench".to_string(),
            metric_names: vec!["primary".to_string(), "secondary".to_string()],
            nodes: [(
                "root/child".to_string(),
                json::JsonSpanNode {
                    name: "child".to_string(),
                    samples: 1,
                    stats: vec![stats(&[10.0]), stats(&[1.0])],
                    children: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
            roots: vec!["root".to_string()],
        };
        let printer = ReportPrinter {
            report: &report,
            baseline: Some(&baseline),
        };

        let node_stats = printer.compute_metric_stats(node(&report, &["root", "child"]));
        let rows = printer.detail_rows_for_node(
            &node_stats,
            baseline
                .nodes
                .get("root/child")
                .map(|node| node.stats.as_slice()),
        );

        assert_eq!(
            printer
                .parent_share_text(path(&["root", "child"]).as_slice())
                .as_deref(),
            Some("50% primary of parent")
        );
        assert_eq!(rows.len(), 2);
        assert!(rows[0][0].contains("+100.00%"));
        assert!(rows[1][0].contains("[20.0 .. 20.0]"));
    }

    #[test]
    fn aggregates_multiple_inner_events_per_outer_iteration() {
        let root_iter_1 = Id::from_u64(1);
        let child_iter_1_first = Id::from_u64(2);
        let child_iter_1_second = Id::from_u64(3);
        let root_iter_2 = Id::from_u64(4);
        let child_iter_2 = Id::from_u64(5);

        let entries = vec![
            ProfileEntry::Register {
                id: root_iter_1.clone(),
                metadata: None,
                parent: None,
                start: (),
            },
            ProfileEntry::Register {
                id: child_iter_1_first.clone(),
                metadata: None,
                parent: Some(root_iter_1.clone()),
                start: (),
            },
            ProfileEntry::Publish {
                id: child_iter_1_first,
                result: [2.0, 20.0],
            },
            ProfileEntry::Register {
                id: child_iter_1_second.clone(),
                metadata: None,
                parent: Some(root_iter_1.clone()),
                start: (),
            },
            ProfileEntry::Publish {
                id: child_iter_1_second,
                result: [3.0, 30.0],
            },
            ProfileEntry::Publish {
                id: root_iter_1,
                result: [10.0, 100.0],
            },
            ProfileEntry::Register {
                id: root_iter_2.clone(),
                metadata: None,
                parent: None,
                start: (),
            },
            ProfileEntry::Register {
                id: child_iter_2.clone(),
                metadata: None,
                parent: Some(root_iter_2.clone()),
                start: (),
            },
            ProfileEntry::Publish {
                id: child_iter_2,
                result: [7.0, 70.0],
            },
            ProfileEntry::Publish {
                id: root_iter_2,
                result: [10.0, 100.0],
            },
        ];

        let report = AnalyzedReport::from_profile_entries(
            &entries,
            Arc::new(TestMetrics),
            None,
            "bench".to_string(),
        );
        let child = node(&report, &["unknown", "unknown"]);
        let stats = compute_metric_stats_for_samples(&child.samples, 2);

        assert_eq!(child.samples, vec![vec![5.0, 50.0], vec![7.0, 70.0]]);
        assert_eq!(stats[0].mean, 6.0);
        assert_eq!(stats[1].mean, 60.0);
    }

    #[test]
    fn uses_metric_aggregation_kind_for_repeated_inner_events() {
        let report = report_from_outer_iters_with_metrics(
            Arc::new(PeakMetrics),
            vec![
                vec![
                    (vec!["root"], [10.0, 100.0]),
                    (vec!["root", "child"], [2.0, 20.0]),
                    (vec!["root", "child"], [3.0, 30.0]),
                ],
                vec![
                    (vec!["root"], [10.0, 90.0]),
                    (vec!["root", "child"], [7.0, 70.0]),
                ],
            ],
        );

        let child = node(&report, &["root", "child"]);

        assert_eq!(child.samples, vec![vec![5.0, 30.0], vec![7.0, 70.0]]);
    }
}
