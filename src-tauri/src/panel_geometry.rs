use crate::settings::PanelSizePreference;

pub const DEFAULT_PANEL_WIDTH: f64 = 520.0;
pub const DEFAULT_PANEL_HEIGHT: f64 = 600.0;
pub const WIDE_PANEL_WIDTH: f64 = 760.0;
pub const WIDE_PANEL_HEIGHT: f64 = 600.0;
pub const MIN_PANEL_WIDTH: f64 = 420.0;
pub const MIN_PANEL_HEIGHT: f64 = 420.0;

// A hand-edited or damaged preference must not ask the window manager for an
// absurd allocation. This is deliberately far above ordinary ultrawide
// desktops: it is a corruption guard, not a product width cap.
const MAX_PREFERENCE_DIMENSION: f64 = 10_000.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogicalPanelSize {
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogicalWorkArea {
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicalWorkArea {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

pub fn default_preference() -> PanelSizePreference {
    PanelSizePreference {
        width: DEFAULT_PANEL_WIDTH,
        height: DEFAULT_PANEL_HEIGHT,
    }
}

pub fn wide_preference() -> PanelSizePreference {
    PanelSizePreference {
        width: WIDE_PANEL_WIDTH,
        height: WIDE_PANEL_HEIGHT,
    }
}

/// Resolve a persisted size into a finite, usable 100%-UI-scale preference.
/// Invalid/non-positive dimensions fall back independently so a damaged width
/// does not discard an otherwise valid height.
pub fn sanitize_preference(preference: Option<PanelSizePreference>) -> PanelSizePreference {
    let fallback = default_preference();
    let preference = preference.unwrap_or(fallback);
    PanelSizePreference {
        width: sanitize_dimension(preference.width, fallback.width, MIN_PANEL_WIDTH),
        height: sanitize_dimension(preference.height, fallback.height, MIN_PANEL_HEIGHT),
    }
}

fn sanitize_dimension(value: f64, fallback: f64, minimum: f64) -> f64 {
    if !value.is_finite() || value <= 0.0 {
        return fallback;
    }
    // Window APIs may expose the DPI scale with a tiny floating-point error
    // (for example 1.50000002). Keep enough precision for manual resizing,
    // while preventing stable values such as 520 from drifting in settings.
    let clamped = value.clamp(minimum, MAX_PREFERENCE_DIMENSION);
    (clamped * 100.0).round() / 100.0
}

fn usable_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

fn usable_extent(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        1.0
    }
}

/// Convert the normalized preference into the logical window size for the
/// current app UI scale, clamping only the actual allocation to the current
/// monitor work area. The caller keeps the original preference unchanged so a
/// temporary small monitor never erases the user's large-display intent.
pub fn target_logical_size(
    preference: PanelSizePreference,
    ui_scale: f64,
    work_area: LogicalWorkArea,
) -> LogicalPanelSize {
    let preference = sanitize_preference(Some(preference));
    let scale = usable_scale(ui_scale);
    let available_width = usable_extent(work_area.width);
    let available_height = usable_extent(work_area.height);
    let minimum_width = (MIN_PANEL_WIDTH * scale).min(available_width);
    let minimum_height = (MIN_PANEL_HEIGHT * scale).min(available_height);

    LogicalPanelSize {
        width: (preference.width * scale)
            .min(available_width)
            .max(minimum_width),
        height: (preference.height * scale)
            .min(available_height)
            .max(minimum_height),
    }
}

/// Dynamic logical minimum for the current app scale. On a display smaller
/// than the normal minimum, reachability wins and the OS is allowed to fit the
/// panel inside the available work area.
pub fn minimum_logical_size(ui_scale: f64, work_area: LogicalWorkArea) -> LogicalPanelSize {
    let scale = usable_scale(ui_scale);
    LogicalPanelSize {
        width: (MIN_PANEL_WIDTH * scale).min(usable_extent(work_area.width)),
        height: (MIN_PANEL_HEIGHT * scale).min(usable_extent(work_area.height)),
    }
}

/// Turn a Tauri client-area physical size into the user's 100%-scale
/// preference. OS DPI and gitwink UI scale are independent divisors.
pub fn preference_from_physical_size(
    physical_width: u32,
    physical_height: u32,
    os_scale: f64,
    ui_scale: f64,
) -> PanelSizePreference {
    let divisor = usable_scale(os_scale) * usable_scale(ui_scale);
    sanitize_preference(Some(PanelSizePreference {
        width: physical_width as f64 / divisor,
        height: physical_height as f64 / divisor,
    }))
}

/// Keep a normal window fully reachable inside one monitor's work area. If
/// the window is larger than the area, pin that axis to the area's origin;
/// size reconciliation is handled separately.
pub fn clamp_physical_position(
    x: i32,
    y: i32,
    window_width: u32,
    window_height: u32,
    work_area: PhysicalWorkArea,
) -> (i32, i32) {
    let max_x = work_area
        .x
        .saturating_add(work_area.width.saturating_sub(window_width) as i32);
    let max_y = work_area
        .y
        .saturating_add(work_area.height.saturating_sub(window_height) as i32);
    (
        x.clamp(work_area.x, max_x.max(work_area.x)),
        y.clamp(work_area.y, max_y.max(work_area.y)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_size_round_trips_across_ui_and_os_scales() {
        let preferences = [
            default_preference(),
            wide_preference(),
            PanelSizePreference {
                width: 1200.0,
                height: 800.0,
            },
        ];
        for preference in preferences {
            for ui_scale in [1.0, 1.3, 1.6] {
                for os_scale in [1.0, 1.25, 1.5, 2.0] {
                    let physical_width = (preference.width * ui_scale * os_scale).round() as u32;
                    let physical_height = (preference.height * ui_scale * os_scale).round() as u32;
                    let restored = preference_from_physical_size(
                        physical_width,
                        physical_height,
                        os_scale,
                        ui_scale,
                    );
                    assert!((restored.width - preference.width).abs() <= 0.5);
                    assert!((restored.height - preference.height).abs() <= 0.5);
                }
            }
        }
    }

    #[test]
    fn dpi_float_noise_does_not_leak_into_the_preference() {
        let restored = preference_from_physical_size(1248, 1440, 1.500000022, 1.6);

        assert_eq!(restored.width, DEFAULT_PANEL_WIDTH);
        assert_eq!(restored.height, DEFAULT_PANEL_HEIGHT);
    }

    #[test]
    fn monitor_clamp_does_not_mutate_the_preference() {
        let preferred = PanelSizePreference {
            width: 800.0,
            height: 600.0,
        };
        let target = target_logical_size(
            preferred,
            1.25,
            LogicalWorkArea {
                width: 900.0,
                height: 700.0,
            },
        );
        assert_eq!(
            target,
            LogicalPanelSize {
                width: 900.0,
                height: 700.0
            }
        );
        assert_eq!(preferred.width, 800.0);
        assert_eq!(preferred.height, 600.0);
    }

    #[test]
    fn corrupt_dimensions_fall_back_or_clamp_safely() {
        let invalid = sanitize_preference(Some(PanelSizePreference {
            width: -10.0,
            height: 500_000.0,
        }));
        assert_eq!(invalid.width, DEFAULT_PANEL_WIDTH);
        assert_eq!(invalid.height, MAX_PREFERENCE_DIMENSION);

        let too_small = sanitize_preference(Some(PanelSizePreference {
            width: 100.0,
            height: 200.0,
        }));
        assert_eq!(too_small.width, MIN_PANEL_WIDTH);
        assert_eq!(too_small.height, MIN_PANEL_HEIGHT);
    }

    #[test]
    fn minimum_yields_to_a_tiny_work_area() {
        let minimum = minimum_logical_size(
            1.6,
            LogicalWorkArea {
                width: 390.0,
                height: 410.0,
            },
        );
        assert_eq!(
            minimum,
            LogicalPanelSize {
                width: 390.0,
                height: 410.0
            }
        );
    }

    #[test]
    fn position_is_fully_clamped_to_the_work_area() {
        let area = PhysicalWorkArea {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1040,
        };
        assert_eq!(
            clamp_physical_position(-100, 900, 800, 600, area),
            (-800, 440)
        );
        assert_eq!(
            clamp_physical_position(-2500, -100, 2500, 1200, area),
            (-1920, 0)
        );
    }
}
