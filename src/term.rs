//! Shared terminal/process termination signals.

use anyhow::Result;

/// Handle both common termination signals, including while waiting for a prompt.
pub(crate) async fn termination() -> Result<i32> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => { result?; Ok(130) }
            _ = terminate.recv() => Ok(143),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await?;
        Ok(130)
    }
}
