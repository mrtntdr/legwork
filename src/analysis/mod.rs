pub mod coloring;
pub mod leg;

pub use coloring::{MetricRange, auto_range, color_for, quickness_color, segment_metric};

pub use leg::{control_indices, fmt_duration, fmt_pace, legs};
