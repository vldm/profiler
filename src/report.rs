use std::{collections::HashMap, fmt::Debug};
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
#[derive(Clone, Default)]
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
        let median = if sorted.len() % 2 == 0 {
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
    metric_names: Vec<String>,
    /// Aggregated nodes keyed by path string ("root/child/grandchild").
    nodes: HashMap<String, SpanNode>,
    /// Root keys in insertion order.
    roots: Vec<String>,
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

        dbg!(&entries);
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
                span_info
                    .entry(id.clone())
                    .or_insert((metadata.unwrap().name().to_string(), parent.clone()));
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
        // Maintain insertion-order for children.
        let mut root_set: HashMap<String, ()> = HashMap::new();

        for entry in entries {
            if let ProfileEntry::Publish { id, result } = entry {
                let key = match key_cache.get(id) {
                    Some(k) => k.clone(),
                    None => continue,
                };
                let (name, _) = span_info.get(id).unwrap();
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
                    if let Some(pnode) = nodes.get_mut(&parent_key) {
                        if !pnode.children.contains(&key) {
                            pnode.children.push(key.clone());
                        }
                    }
                } else {
                    // This is a root.
                    if !root_set.contains_key(&key) {
                        root_set.insert(key.clone(), ());
                        roots.push(key.clone());
                    }
                }
            }
        }

        Self {
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
        let col_w = 26;

        // Header
        print!("\x1b[1m{:<30}", "span");
        for name in &self.metric_names {
            print!("{:>w$}", name, w = col_w);
        }
        println!("\x1b[0m");
        println!("{}", "─".repeat(30 + col_w * n_metrics));

        for root_key in &self.roots {
            self.print_node(root_key, 0, n_metrics, col_w);
        }
    }

    fn print_node(&self, key: &str, depth: usize, n_metrics: usize, col_w: usize) {
        let node = match self.nodes.get(key) {
            Some(n) => n,
            None => return,
        };

        if node.samples.is_empty() {
            return;
        }

        let stats: Vec<MetricStats> = (0..n_metrics)
            .map(|j| {
                let vals: Vec<f64> = node.samples.iter().map(|s| s[j]).collect();
                MetricStats::from_values(&vals)
            })
            .collect();

        let indent = "  ".repeat(depth);
        let n_samples = node.samples.len();

        // Row 1: name(count)  mean ± spread%
        {
            let label = format!("{}{}  ({})", indent, node.name, n_samples);
            print!("\x1b[1m{:<30}\x1b[0m", label);
            for s in &stats {
                let (val, unit) = format_auto(s.mean);
                let spread_pct = s.spread() * 100.0;
                let cell = format!("{}{} ± {:.0}%", val, unit, spread_pct);
                print!("{:>w$}", cell, w = col_w);
            }
            println!();
        }

        // Row 2: [min .. median .. max]
        {
            print!("{:<30}", "");
            for s in &stats {
                let (lo, u1) = format_auto(s.min);
                let (md, u2) = format_auto(s.median);
                let (hi, u3) = format_auto(s.max);
                let range = format!("[{}{} .. {}{} .. {}{}]", lo, u1, md, u2, hi, u3);
                let cell = format!("\x1b[2m{}\x1b[0m", range);
                let padding = col_w.saturating_sub(range.len());
                print!("{:>pad$}{}", "", cell, pad = padding);
            }
            println!();
        }

        // Children
        for child_key in &node.children {
            self.print_node(child_key, depth + 1, n_metrics, col_w);
        }
    }
}

// ── Formatting helpers ─────────────────────────────────────────

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
