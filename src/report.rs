use std::{
    char::MAX,
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Debug,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

use serde::Serialize;
use tracing::Id;

use crate::{Metrics, ProfileEntry};

// ── Aggregated span node ───────────────────────────────────────

/// One node in the aggregated span tree, keyed by (name, parent_key).
struct SpanNode {
    name: String,
    /// `samples[i][j]` = value of metric `j` in sample `i`.
    samples: Vec<Vec<f64>>,
    children: Vec<String>, // child keys (ordered, deduplicated)
}

/// Statistics for one metric across samples.
#[derive(Clone, Default, Serialize)]
pub struct MetricStats {
    pub mean: f64,
    pub stddev: f64,
    pub min: f64,
    pub max: f64,
    pub median: f64,
}

impl MetricStats {
    fn from_values(values: &[f64]) -> Self {
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

pub struct Report {
    group_name: Option<String>,
    bench_name: String,
    metric_names: Vec<String>,
    /// Aggregated nodes keyed by path string ("root/child/grandchild").
    nodes: HashMap<String, SpanNode>,
    /// Root keys in insertion order.
    roots: Vec<String>,
}

#[derive(Serialize)]
struct JsonReport {
    schema_version: u32,
    group: Option<String>,
    name: String,
    metric_names: Vec<String>,
    roots: Vec<JsonSpanNode>,
}

#[derive(Serialize)]
struct JsonSpanNode {
    name: String,
    samples: usize,
    metrics: BTreeMap<String, MetricStats>,
    children: Vec<JsonSpanNode>,
}

const LABEL_W: usize = 34;
const COL_W: usize = 20;
const COL_GAP: usize = 5;
const MAX_TREE_DEPTH: usize = 3;

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

impl Report {
    /// Build a report from raw `ProfileEntry` events produced by the Collector.
    ///
    /// Entries with the same span name under the same parent name are merged
    /// into a single node, so 100 iterations of `iter → parse → process`
    /// become one tree with 100 samples per node.
    pub fn from_profile_entries<M: Metrics>(
        entries: &[ProfileEntry<M::Start, M::Result>],
        metrics: &M,
        group_name: Option<String>,
        bench_name: String,
    ) -> Self
    where
        M::Result: Debug,
        M::Start: Debug,
    {
        let metric_names: Vec<String> = metrics
            .metric_names()
            .iter()
            .map(|s| s.to_string())
            .collect();

        // Phase 1: map span Id → (name, parent_id) from Register entries.
        let mut span_info: HashMap<Id, (String, Option<Id>)> = HashMap::new();
        for entry in entries {
            if let ProfileEntry::Register {
                id,
                metadata,
                parent,
                ..
            } = entry
            {
                span_info.entry(id.clone()).or_insert((
                    metadata
                        .map(|meta| meta.name().to_string())
                        .unwrap_or_else(|| "unknown".to_string()),
                    parent.clone(),
                ));
            }
        }

        // Phase 2: for each Id compute its aggregation key (name path).
        // Cache to avoid repeated traversal.
        let mut key_cache: HashMap<Id, String> = HashMap::new();
        fn compute_key(
            id: &Id,
            span_info: &HashMap<Id, (String, Option<Id>)>,
            cache: &mut HashMap<Id, String>,
        ) -> String {
            if let Some(cached) = cache.get(id) {
                return cached.clone();
            }
            let (name, parent) = match span_info.get(id) {
                Some(v) => v,
                None => return "?".to_string(),
            };
            let key = match parent {
                Some(pid) => {
                    let parent_key = compute_key(pid, span_info, cache);
                    format!("{}/{}", parent_key, name)
                }
                None => name.clone(),
            };
            cache.insert(id.clone(), key.clone());
            key
        }

        for id in span_info.keys().cloned().collect::<Vec<_>>() {
            compute_key(&id, &span_info, &mut key_cache);
        }

        // Phase 3: aggregate Publish entries by key.
        let mut nodes: HashMap<String, SpanNode> = HashMap::new();
        let mut roots: Vec<String> = Vec::new();
        let mut root_set: HashSet<String> = HashSet::new();

        for entry in entries {
            if let ProfileEntry::Publish { id, result } = entry {
                let key = match key_cache.get(id) {
                    Some(k) => k.clone(),
                    None => continue,
                };
                let (name, _) = match span_info.get(id) {
                    Some(v) => v,
                    None => continue,
                };
                let values = metrics.result_to_f64s(result);

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
        }

        roots.sort();
        for node in nodes.values_mut() {
            node.children.sort();
        }

        Self {
            group_name,
            bench_name,
            metric_names,
            nodes,
            roots,
        }
    }

    pub fn print(&self) {
        if self.nodes.is_empty() {
            println!("No profiling data.");
            return;
        }

        let n_metrics = self.metric_names.len();
        let layout = PrintLayout {
            label_w: LABEL_W,
            col_w: COL_W,
            col_gap: COL_GAP,
        };
        let sep = "─".repeat(table_width(n_metrics));

        // Table header
        print!("{:<label_w$}", "", label_w = layout.label_w);
        for name in &self.metric_names {
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

        for (idx, root_key) in self.roots.iter().enumerate() {
            self.print_node(root_key, "", idx + 1 == self.roots.len(), true, 0, layout);
        }
    }

    /// Print a collection of reports grouped, with group headers and separators.
    /// All output lives here — callers just pass in the reports.
    pub fn print_all(reports: &[Self]) {
        if reports.is_empty() {
            return;
        }
        let n_metrics = reports[0].metric_names.len();
        let w = table_width(n_metrics);
        // let thick = "─".repeat(w);
        let thin = "- ".repeat(w / 2).trim_end().to_string();

        let mut current_group: Option<&Option<String>> = None;
        let mut bench_index_in_group: usize = 0;

        for report in reports {
            let group = &report.group_name;
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
            report.print();
            bench_index_in_group += 1;
        }
    }

    pub fn write_json_to_default_path(&self) -> io::Result<PathBuf> {
        let mut path = PathBuf::from("target").join("profiler");
        if let Some(group) = &self.group_name {
            path = path.join(sanitize_path_segment(group));
        }
        path = path
            .join(sanitize_path_segment(&self.bench_name))
            .join("run.json");
        self.write_json(&path)?;
        Ok(path)
    }

    pub fn write_json(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let report = self.to_json();
        let file = File::create(path)?;
        serde_json::to_writer_pretty(file, &report).map_err(io::Error::other)
    }

    fn to_json(&self) -> JsonReport {
        let roots = self
            .roots
            .iter()
            .filter_map(|root| self.to_json_node(root))
            .collect();

        JsonReport {
            schema_version: 1,
            group: self.group_name.clone(),
            name: self.bench_name.clone(),
            metric_names: self.metric_names.clone(),
            roots,
        }
    }

    fn to_json_node(&self, key: &str) -> Option<JsonSpanNode> {
        let node = self.nodes.get(key)?;

        if node.samples.is_empty() {
            return None;
        }

        let stats = self.compute_metric_stats(node);
        let mut metrics = BTreeMap::new();
        for (metric_name, stat) in self.metric_names.iter().zip(stats.iter()) {
            metrics.insert(metric_name.clone(), stat.clone());
        }

        let children = node
            .children
            .iter()
            .filter_map(|child_key| self.to_json_node(child_key))
            .collect();

        Some(JsonSpanNode {
            name: node.name.clone(),
            samples: node.samples.len(),
            metrics,
            children,
        })
    }

    fn print_node(
        &self,
        key: &str,
        prefix: &str,
        is_last: bool,
        is_root: bool,
        depth: usize,
        layout: PrintLayout,
    ) {
        let node = match self.nodes.get(key) {
            Some(n) => n,
            None => return,
        };

        if node.samples.is_empty() {
            return;
        }

        let stats = self.compute_metric_stats(node);

        let n_samples = node.samples.len();
        let branch = if is_root {
            ""
        } else if is_last {
            "└── "
        } else {
            "├── "
        };
        let node_display_name = if is_root {
            self.bench_name.as_str()
        } else {
            node.name.as_str()
        };
        let is_compact = depth >= MAX_TREE_DEPTH;
        let label = if is_compact {
            format!("{}  └─{}  ({})", prefix, node_display_name, n_samples)
        } else {
            format!("{}{}{}  ({})", prefix, branch, node_display_name, n_samples)
        };

        // Row 1: name(count) and mean ± spread%
        print!(
            "\x1b[1m{:<label_w$}\x1b[0m",
            label,
            label_w = layout.label_w
        );
        for s in &stats {
            let (val, unit) = format_auto(s.mean);
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
        println!();

        // Row 2: compact [min..max]
        {
            let detail_tree = if is_root {
                String::new()
            } else {
                let parent_continuation = if is_last { "    " } else { "│   " };
                if node.children.is_empty() {
                    let own_continuation = "    ";
                    format!("{}{}{}", prefix, parent_continuation, own_continuation)
                } else if depth + 1 >= MAX_TREE_DEPTH {
                    let child_prefix = if depth < MAX_TREE_DEPTH { "    " } else { "" };
                    // child is compact add prefix
                    format!("{} {} ({})=>", prefix, child_prefix, node_display_name)
                } else {
                    let own_continuation = "│   ";
                    format!("{}{}{}", prefix, parent_continuation, own_continuation)
                }
            };
            // dbg!(&detail_tree);
            print!("{:<label_w$}", detail_tree, label_w = layout.label_w);
            for s in &stats {
                let range = format_compact_range(s.min, s.max);
                let cell = format!("\x1b[2m{}\x1b[0m", range);
                let padding = layout.col_w.saturating_sub(range.len());
                print!(
                    "{:gap$}{:>pad$}{}",
                    "",
                    "",
                    cell,
                    gap = layout.col_gap,
                    pad = padding
                );
            }
            println!();
        }

        let next_prefix = if is_root {
            String::new()
        } else if is_compact {
            prefix.to_string()
        } else {
            format!("{}{}", prefix, if is_last { "    " } else { "│   " })
        };

        for (idx, child_key) in node.children.iter().enumerate() {
            self.print_node(
                child_key,
                &next_prefix,
                idx + 1 == node.children.len(),
                false,
                depth + 1,
                layout,
            );
        }
    }

    fn compute_metric_stats(&self, node: &SpanNode) -> Vec<MetricStats> {
        let n_metrics = self.metric_names.len();
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
}

// ── Formatting helpers ─────────────────────────────────────────

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

fn format_auto(value: f64) -> (String, &'static str) {
    if value >= 1_000_000_000.0 {
        (format!("{:.3}", value / 1_000_000_000.0), "s")
    } else if value >= 1_000_000.0 {
        (format!("{:.3}", value / 1_000_000.0), "ms")
    } else if value >= 1_000.0 {
        (format!("{:.2}", value / 1_000.0), "K")
    } else if value >= 1.0 {
        (format!("{:.1}", value), "")
    } else {
        (format!("{:.3}", value), "")
    }
}

fn format_compact_range(min: f64, max: f64) -> String {
    let (lo, u1) = format_auto(min);
    let (hi, u2) = format_auto(max);

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
