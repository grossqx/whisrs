//! Recording-lifecycle hooks: MPRIS pause + shell commands.

#[cfg(feature = "hooks")]
use whisrs::hooks::{hook_event_for, run_hook, HookEvent};
#[cfg(feature = "hooks")]
use whisrs::{HooksConfig, State};

/// Watches daemon state broadcasts and fires recording-lifecycle hooks.
#[cfg(feature = "hooks")]
pub(crate) async fn hook_dispatch_loop(
    mut state_rx: tokio::sync::watch::Receiver<State>,
    hooks: HooksConfig,
) {
    let mut prev = *state_rx.borrow();
    let mut paused_players: Vec<String> = Vec::new();
    while state_rx.changed().await.is_ok() {
        let new = *state_rx.borrow();
        match hook_event_for(prev, new) {
            Some(HookEvent::RecordStart) => {
                if hooks.media_auto_pause {
                    paused_players = whisrs::mpris::pause_playing().await;
                }
                if let Some(cmd) = hooks.on_record_start.as_deref() {
                    run_hook(cmd);
                }
            }
            Some(HookEvent::RecordStop) => {
                if hooks.media_auto_pause && !paused_players.is_empty() {
                    whisrs::mpris::resume(&paused_players).await;
                    paused_players.clear();
                }
                if let Some(cmd) = hooks.on_record_stop.as_deref() {
                    run_hook(cmd);
                }
            }
            None => {}
        }
        prev = new;
    }
}
