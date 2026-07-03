pub mod coloring;
pub mod compare;
pub mod leg;
pub mod matching;
pub mod playback;

pub use coloring::{MetricRange, auto_range, color_for, quickness_color, segment_metric};

pub use compare::{LegRow, compare, leg_label};

pub use leg::{fmt_duration, fmt_pace};

pub use matching::{local_scale_px_per_m, match_controls};

pub use playback::{ClockMode, Window, build_timeline, index_at, position_at, total_span};
