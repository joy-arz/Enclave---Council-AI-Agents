pub mod routes;
pub mod sessions_mod;
pub mod rate_limit;
pub mod config_routes;

use std::sync::Arc;
use crate::utils::config;
use crate::api::rate_limit::IpRateLimiter;
use crate::utils::config_manager::ConfigManager;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<config>,
    pub session_store: Arc<session_store>,
    pub rate_limiter: Arc<IpRateLimiter>,
    pub config_manager: Arc<ConfigManager>,
}

pub use routes::handle_enclave;
pub use sessions_mod::session_store;
