use std::sync::Arc;

use rmcp::ServiceExt;
use rmcp::transport::stdio;

use super::{OwnsearchHandler, shutdown_signal};
use crate::config::Settings;
use crate::error::{Error, Result};

pub async fn serve_stdio(settings: Settings) -> Result<()> {
    let mcp_handler = OwnsearchHandler::new(settings)?;
    mcp_handler.context().ensure_bootstrap()?;
    let cancellation = mcp_handler.cancellation_token();
    let context = mcp_handler.context();
    crate::browser::reap_orphans(&context.settings.state_dir().join("browser"));
    let _hang_watchdog = super::start_hang_watchdog(&context, cancellation.clone());
    super::spawn_cleanup(Arc::clone(&context), cancellation.clone());
    super::spawn_scanner(Arc::clone(&context), cancellation.clone());
    let mut shutdown = std::pin::pin!(shutdown_signal());
    let service = tokio::select! {
        result = mcp_handler.serve(stdio()) => result
            .map_err(|error| session_error("starting the stdio session", error))?,
        () = &mut shutdown => return Ok(()),
    };
    let token = service.cancellation_token();
    let mut joined = tokio::spawn(service.waiting());
    tokio::select! {
        result = &mut joined => match result {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => Err(session_error("running the stdio session", error)),
            Err(error) => Err(Error::Io {
                context: "waiting for the stdio session",
                source: std::io::Error::other(error),
            }),
        },
        () = &mut shutdown => {
            token.cancel();
            cancellation.cancel();
            context.heartbeat.stop();
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                context.browser.shutdown(),
            )
            .await;
            match tokio::time::timeout(std::time::Duration::from_secs(10), &mut joined).await {
                Ok(_) => {}
                Err(_) => {
                    tracing::warn!("the stdio session did not drain within 10 seconds");
                    joined.abort();
                }
            }
            Ok(())
        }
    }
}

fn session_error(context: &'static str, error: impl std::fmt::Display) -> Error {
    Error::Io {
        context,
        source: std::io::Error::other(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_failure_is_an_io_error_not_a_config_error() {
        let error = session_error("running the stdio session", "the pipe closed");
        assert_eq!(error.code(), "ownsearch.internal_error");
        assert!(error.to_string().contains("running the stdio session"));
    }
}
