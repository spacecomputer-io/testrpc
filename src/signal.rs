use tokio::{select, signal};

use crate::common::TestrpcError;

pub async fn wait_exit_signals() -> Result<(), TestrpcError> {
    tracing::debug!("Signal handler initialized, waiting for termination signals...");
    
    let mut terminate = signal::unix::signal(signal::unix::SignalKind::terminate())
        .map_err(|e| TestrpcError::TerminationError(e.to_string()))?;
    let mut interrupt = signal::unix::signal(signal::unix::SignalKind::interrupt())
        .map_err(|e| TestrpcError::TerminationError(e.to_string()))?;
    let mut quit = signal::unix::signal(signal::unix::SignalKind::quit())
        .map_err(|e| TestrpcError::TerminationError(e.to_string()))?;

    select! {
        _ = terminate.recv() => {
            tracing::warn!("🛑 SIGTERM received - shutting down gracefully");
        }
        _ = interrupt.recv() => {
            tracing::warn!("🛑 SIGINT (Ctrl+C) received - shutting down gracefully");
        }
        _ = quit.recv() => {
            tracing::warn!("🛑 SIGQUIT received - shutting down gracefully");
        }
    }

    Ok(())
}
