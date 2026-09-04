//! Shared formatting helpers for view models.
//!
//! Kept here (not in `now_playing`) so all per-view modules can reach
//! them without forking copies.

/// Two-digit minutes, optional leading hours.
pub fn format_duration(secs: f32) -> String {
    let total = secs.max(0.0) as u64;
    if total >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            total / 3600,
            (total % 3600) / 60,
            total % 60
        )
    } else {
        format!("{:02}:{:02}", total / 60, total % 60)
    }
}
