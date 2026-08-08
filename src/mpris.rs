//! Native MPRIS v2 pause/resume over the session D-Bus. Every player
//! (browsers, Spotify, VLC, MPV, KDE Connect) registers as
//! `org.mpris.MediaPlayer2.*`, so the session bus *is* the integration.
//! Failures degrade gracefully — a missing bus never blocks dictation.

use tracing::{debug, warn};

const PLAYER_PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";

/// Does this bus name identify a controllable MPRIS player?
pub fn is_mpris_player(name: &str) -> bool {
    name.starts_with("org.mpris.MediaPlayer2.")
}

/// Pause every MPRIS player; returns the paused names for later selective
/// resume. `Pause` is a no-op on already-paused or stopped players.
pub async fn pause_playing() -> Vec<String> {
    let Some(conn) = zbus::Connection::session().await.ok() else {
        debug!("no session bus — MPRIS media pause unavailable");
        return Vec::new();
    };
    let Some(dbus) = zbus::fdo::DBusProxy::new(&conn).await.ok() else {
        debug!("failed to create D-Bus proxy — MPRIS media pause unavailable");
        return Vec::new();
    };
    let Some(names) = dbus.list_names().await.ok() else {
        debug!("failed to list D-Bus names — MPRIS media pause unavailable");
        return Vec::new();
    };

    let mut paused = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);

    // Pre-filter to MPRIS names so the deadline only counts real players.
    let mpris_names: Vec<String> = names
        .into_iter()
        .filter_map(|n| {
            let s = n.to_string();
            is_mpris_player(&s).then_some(s)
        })
        .collect();

    for name in mpris_names {
        if tokio::time::Instant::now() >= deadline {
            warn!(
                "MPRIS pause timed out after pausing {} player(s)",
                paused.len()
            );
            break;
        }

        let Ok(proxy) = zbus::Proxy::new(&conn, name.as_str(), PLAYER_PATH, PLAYER_IFACE).await
        else {
            debug!("skipped {name}: failed to create MPRIS proxy");
            continue;
        };
        match proxy.call_method("Pause", &()).await {
            Ok(_) => {
                debug!("paused MPRIS player {name}");
                paused.push(name);
            }
            Err(e) => warn!("failed to pause MPRIS player {name}: {e}"),
        }
    }
    paused
}

/// Resume specific players by name.
pub async fn resume(names: &[String]) {
    let Ok(conn) = zbus::Connection::session().await else {
        return;
    };
    for name in names {
        let Ok(proxy) = zbus::Proxy::new(&conn, name.as_str(), PLAYER_PATH, PLAYER_IFACE).await
        else {
            continue;
        };
        let _ = proxy.call_method("Play", &()).await;
    }
}

/// Resume every MPRIS player on the session bus. Catches players whose bus
/// name changed during recording (e.g. browser tabs switching).
pub async fn resume_all() {
    let Ok(conn) = zbus::Connection::session().await else {
        return;
    };
    let Ok(dbus) = zbus::fdo::DBusProxy::new(&conn).await else {
        return;
    };
    let Ok(names) = dbus.list_names().await else {
        return;
    };

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    for name in names {
        if tokio::time::Instant::now() >= deadline {
            warn!("MPRIS resume_all timed out");
            break;
        }
        let name = name.to_string();
        if !is_mpris_player(&name) {
            continue;
        }
        let Ok(proxy) = zbus::Proxy::new(&conn, name.as_str(), PLAYER_PATH, PLAYER_IFACE).await
        else {
            continue;
        };
        match proxy.call_method("Play", &()).await {
            Ok(_) => debug!("resumed MPRIS player {name}"),
            Err(e) => debug!("failed to resume MPRIS player {name}: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpris_player_detection() {
        assert!(is_mpris_player("org.mpris.MediaPlayer2.spotify"));
        assert!(is_mpris_player(
            "org.mpris.MediaPlayer2.plasma-browser-integration"
        ));
        assert!(is_mpris_player(
            "org.mpris.MediaPlayer2.kdeconnect.qOo4rGkeX7g"
        ));
        assert!(!is_mpris_player("org.freedesktop.DBus"));
        assert!(!is_mpris_player("org.mpris.MediaPlayer2"));
        assert!(!is_mpris_player("com.spotify.Client"));
    }
}
