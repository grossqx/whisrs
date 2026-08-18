//! D-Bus window tracking stub for GNOME and KDE.
//!
//! Neither desktop exposes focus tracking to an unprivileged daemon out of the
//! box: GNOME needs a shell extension (issue #72), and KDE would need the
//! `org_kde_plasma_window_management` Wayland protocol, which whisrs does not
//! speak. This module provides a stub that returns clear error messages.

use tracing::{debug, warn};

use super::WindowTracker;

/// Stub window tracker for GNOME/KDE desktops via D-Bus.
pub struct DbusTracker {
    desktop: String,
}

impl DbusTracker {
    pub fn new(desktop: &str) -> Self {
        Self {
            desktop: desktop.to_string(),
        }
    }
}

impl WindowTracker for DbusTracker {
    fn get_focused_window(&self) -> anyhow::Result<String> {
        warn!(
            "{} window tracking not yet supported — text will be typed at current cursor",
            self.desktop
        );
        // Return a placeholder so the flow doesn't break.
        Ok("dbus-stub".to_string())
    }

    fn focus_window(&self, _id: &str) -> anyhow::Result<()> {
        warn!(
            "{} window focus restoration not yet supported — skipping",
            self.desktop
        );
        // Don't fail — graceful degradation.
        Ok(())
    }

    /// Always `None`: GNOME needs a shell extension (issue #72) and KDE would
    /// need the `org_kde_plasma_window_management` Wayland protocol, so neither
    /// can report the focused window class here yet.
    ///
    /// Logged at `debug!` rather than the `warn!` the other two methods use:
    /// this is queried on every injection, and the other backends log their
    /// class lookups at `debug!` too.
    fn get_focused_window_class(&self) -> Option<String> {
        debug!(
            "{} focused window class not yet supported; terminal detection stays off",
            self.desktop
        );
        None
    }
}
