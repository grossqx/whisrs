//! Native MPRIS v2 pause/resume over the session D-Bus. Every player
//! (browsers, Spotify, VLC, MPV, KDE Connect) registers as
//! `org.mpris.MediaPlayer2.*`, so the session bus *is* the integration.
//! Failures degrade gracefully — a missing bus never blocks dictation.

use tracing::{debug, warn};

const PLAYER_PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";

/// Pure: does this bus name identify a controllable MPRIS player?
pub fn is_mpris_player(name: &str) -> bool {
    name.starts_with("org.mpris.MediaPlayer2.")
}

/// Pause every playing, pausable MPRIS player; returns the paused names for
/// later selective resume. Empty on failure — recording always proceeds.
pub async fn pause_playing() -> Vec<String> {
    match tokio::time::timeout(std::time::Duration::from_secs(5), pause_playing_inner()).await {
        Ok(v) => v,
        Err(_) => {
            warn!("MPRIS pause timed out");
            Vec::new()
        }
    }
}

async fn pause_playing_inner() -> Vec<String> {
    let Ok(conn) = zbus::Connection::session().await else {
        debug!("no session bus — MPRIS media pause unavailable");
        return Vec::new();
    };
    let Ok(dbus) = zbus::fdo::DBusProxy::new(&conn).await else {
        return Vec::new();
    };
    let Ok(names) = dbus.list_names().await else {
        return Vec::new();
    };
    let mut paused = Vec::new();
    for name in names {
        let name = name.to_string();
        if !is_mpris_player(&name) {
            continue;
        }
        let Ok(proxy) = zbus::Proxy::new(&conn, name.as_str(), PLAYER_PATH, PLAYER_IFACE).await
        else {
            continue;
        };
        // Only pause what is actually playing and pausable.
        let Ok(status) = proxy.get_property::<String>("PlaybackStatus").await else {
            continue;
        };
        if status != "Playing" {
            continue;
        }
        let Ok(can_pause) = proxy.get_property::<bool>("CanPause").await else {
            continue;
        };
        if !can_pause {
            continue;
        }
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

/// Resume exactly the players we paused. Players that quit or moved to a
/// different playback state since pause are left alone; failures are
/// logged, never fatal.
pub async fn resume(names: &[String]) {
    if tokio::time::timeout(std::time::Duration::from_secs(5), resume_inner(names))
        .await
        .is_err()
    {
        warn!("MPRIS resume timed out");
    }
}

async fn resume_inner(names: &[String]) {
    let Ok(conn) = zbus::Connection::session().await else {
        return;
    };
    for name in names {
        let Ok(proxy) = zbus::Proxy::new(&conn, name.as_str(), PLAYER_PATH, PLAYER_IFACE).await
        else {
            continue;
        };
        let Ok(status) = proxy.get_property::<String>("PlaybackStatus").await else {
            continue;
        };
        if status != "Paused" {
            continue;
        }
        match proxy.call_method("Play", &()).await {
            Ok(_) => debug!("resumed MPRIS player {name}"),
            Err(e) => warn!("failed to resume MPRIS player {name}: {e}"),
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
        // Exact match without trailing suffix — not a player.
        assert!(!is_mpris_player("org.mpris.MediaPlayer2"));
        assert!(!is_mpris_player("com.spotify.Client"));
    }
}
