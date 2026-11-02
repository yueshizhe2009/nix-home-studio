//! Headless check for CI and for the acceptance script.
//!
//! It runs exactly the same discovery path as the window and prints the state the
//! UI would render. No window is opened and nothing is written: discovery is the
//! only intent this build executes, and it only reads.

use nhs_adapter::Session;
use nhs_app::Event;

use crate::bridge::BridgeError;
use crate::service::Service;

/// # Errors
/// Returns [`BridgeError`] when discovery fails or the state cannot be serialised.
pub fn run() -> Result<String, BridgeError> {
    let service = Service::new(Session::new(None, None));
    let state = service.dispatch(Event::DiscoverRequested)?;
    serde_json::to_string_pretty(&state)
        .map_err(|error| BridgeError::internal(format!("Could not serialise the state: {error}")))
}
