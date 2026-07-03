pub mod coloring;
pub mod compare;
pub mod leg;
pub mod matching;

pub use coloring::{MetricRange, auto_range, color_for, quickness_color, segment_metric};

pub use compare::{LegRow, compare};

pub use leg::{fmt_duration, fmt_pace, legs_between};

pub use matching::{local_scale_px_per_m, match_controls};
