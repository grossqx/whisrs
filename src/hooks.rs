//! Hooks fired when a recording session starts/stops. Driven by the state
//! broadcast channel (see [`crate::daemon::hooks::hook_dispatch_loop`]).

use crate::State;
use tracing::{debug, warn};

/// Which hook set fires for a state change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    /// A recording session began.
    RecordStart,
    /// A recording session ended.
    RecordStop,
}

/// Map a state change to the hook that fires; `None` for no-op transitions.
///
/// `RecordStop` fires at the first non-`Recording` state. For dictation that
/// is `Transcribing` (MPRIS resumes while transcription runs); for command
/// mode it is `Idle` (finalize broadcasts after the full pipeline completes).
pub fn hook_event_for(prev: State, new: State) -> Option<HookEvent> {
    match (prev, new) {
        (prev, State::Recording) if prev != State::Recording => Some(HookEvent::RecordStart),
        (State::Recording, new) if new != State::Recording => Some(HookEvent::RecordStop),
        _ => None,
    }
}

/// Run a shell command fire-and-forget via `sh -c`. Never blocks recording.
/// Empty commands are skipped. Hangs are killed after 30 seconds.
pub fn run_hook(cmd: &str) {
    if cmd.trim().is_empty() {
        return;
    }
    match tokio::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .kill_on_drop(true)
        .spawn()
    {
        Ok(mut child) => {
            debug!("running hook: {cmd}");
            let cmd = cmd.to_string();
            tokio::spawn(async move {
                match tokio::time::timeout(std::time::Duration::from_secs(30), child.wait()).await {
                    Ok(Ok(status)) => {
                        if !status.success() {
                            warn!("hook `{cmd}` exited with {status}");
                        }
                    }
                    Ok(Err(e)) => warn!("hook `{cmd}` wait error: {e}"),
                    Err(_) => {
                        warn!("hook `{cmd}` timed out after 30s, killing");
                        let _ = child.kill().await;
                    }
                }
            });
        }
        Err(e) => warn!("failed to run record hook `{cmd}`: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::State;

    #[test]
    fn idle_to_recording_fires_start() {
        assert_eq!(
            hook_event_for(State::Idle, State::Recording),
            Some(HookEvent::RecordStart)
        );
    }

    #[test]
    fn transcribing_to_recording_fires_start() {
        // Never occurs today, but entering Recording from any non-Recording
        // state is a start.
        assert_eq!(
            hook_event_for(State::Transcribing, State::Recording),
            Some(HookEvent::RecordStart)
        );
    }

    #[test]
    fn recording_to_transcribing_fires_stop() {
        assert_eq!(
            hook_event_for(State::Recording, State::Transcribing),
            Some(HookEvent::RecordStop)
        );
    }

    #[test]
    fn recording_to_idle_fires_stop() {
        assert_eq!(
            hook_event_for(State::Recording, State::Idle),
            Some(HookEvent::RecordStop)
        );
    }

    #[test]
    fn duplicate_broadcasts_are_ignored() {
        assert_eq!(hook_event_for(State::Recording, State::Recording), None);
        assert_eq!(hook_event_for(State::Idle, State::Idle), None);
        assert_eq!(
            hook_event_for(State::Transcribing, State::Transcribing),
            None
        );
    }

    #[test]
    fn read_aloud_transitions_are_ignored() {
        assert_eq!(hook_event_for(State::Idle, State::Synthesizing), None);
        assert_eq!(hook_event_for(State::Synthesizing, State::Speaking), None);
        assert_eq!(hook_event_for(State::Speaking, State::Idle), None);
    }

    #[test]
    fn transcribing_to_idle_is_ignored() {
        assert_eq!(hook_event_for(State::Transcribing, State::Idle), None);
    }

    #[tokio::test]
    async fn empty_hook_is_noop() {
        // Should not panic or spawn any child.
        run_hook("");
        run_hook("   ");
    }
}
