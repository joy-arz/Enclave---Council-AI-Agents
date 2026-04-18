mod agents;
mod api;
mod cli;
mod core;
mod utils;

use axum::{
    routing::{get, post, put},
    Router,
};
use clap::Parser;
use std::sync::Arc;
use std::net::SocketAddr;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::agents::{judge::judge_agent, roles};
use crate::api::rate_limit::IpRateLimiter;
use crate::api::session_store;
use crate::cli::cli_args;
use crate::core::orchestrator;
use crate::core::providers_mod::factory;
use crate::utils::config;
use crate::utils::constants::{RATE_LIMIT_MAX_TOKENS, RATE_LIMIT_REFILL_RATE};
use crate::utils::logger_mod::session_logger;
use crate::utils::config_manager::ConfigManager;
use crate::api::AppState;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    // initialize logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let args = cli_args::parse();
    let mut cfg = config::from_env()?;

    // override workspace if provided via cli
    if let Some(ws) = args.workspace.clone() {
        cfg.workspace_dir = ws;
    }

    let cfg_arc = Arc::new(cfg);
    let store = Arc::new(session_store::new(cfg_arc.workspace_dir.clone()));

    if args.server {
        run_server(cfg_arc, store).await
    } else {
        run_cli(cfg_arc, args).await
    }
}

async fn run_server(cfg: Arc<config>, store: Arc<session_store>) -> Result<(), anyhow::Error> {
    // Create rate limiter for API protection
    let rate_limiter = Arc::new(IpRateLimiter::new(
        RATE_LIMIT_MAX_TOKENS,
        RATE_LIMIT_REFILL_RATE,
    ));

    // Create ConfigManager for API config management
    let config_manager = Arc::new(ConfigManager::new(&cfg.workspace_dir));

    // Create shared application state
    let app_state = AppState {
        config: cfg.clone(),
        session_store: store.clone(),
        rate_limiter: rate_limiter.clone(),
        config_manager,
    };

    let app = Router::new()
        .route("/api/enclave", get(api::handle_enclave))
        .route("/api/browse", get(api::routes::browse_workspace))
        .route("/api/test_cli", post(api::routes::test_cli))
        .route("/api/apply", post(api::routes::apply_change))
        .route(
            "/api/history/:session_id",
            get(api::routes::get_session_history),
        )
        .route("/api/sessions", get(api::routes::list_sessions))
        .route(
            "/api/sessions/:session_id",
            axum::routing::delete(api::routes::delete_session),
        )
        .route("/api/config", get(api::config_routes::get_config))
        .route("/api/config", put(api::config_routes::update_config))
        .route("/api/config/validate", post(api::config_routes::validate_config))
        .nest_service("/static", ServeDir::new("src/ui"))
        .route(
            "/",
            get(|| async { axum::response::Html(include_str!("ui/index.html")) }),
        )
        .with_state(app_state)
        .layer(CorsLayer::permissive());

    let addr = format!("{}:{}", cfg.host, cfg.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("server listening on http://{}", addr);

    // graceful shutdown on SIGINT/SIGTERM with timeout
    let shutdown_signal = async {
        match tokio::signal::ctrl_c().await {
            Ok(_) => tracing::info!("shutdown signal received, stopping server..."),
            Err(e) => tracing::warn!(
                "failed to install signal handler: {}, continuing anyway...",
                e
            ),
        }
    };

    // Add timeout to graceful shutdown
    let shutdown_result = tokio::time::timeout(
        std::time::Duration::from_secs(crate::utils::constants::SHUTDOWN_TIMEOUT_SECS),
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal),
    )
    .await;

    match shutdown_result {
        Ok(Ok(())) => tracing::info!("server stopped gracefully"),
        Ok(Err(e)) => tracing::error!("server error: {}", e),
        Err(_) => tracing::warn!(
            "shutdown timed out after {} seconds, forcing exit",
            crate::utils::constants::SHUTDOWN_TIMEOUT_SECS
        ),
    }

    tracing::info!("server stopped");
    Ok(())
}

async fn run_cli(cfg: Arc<config>, args: cli_args) -> Result<(), anyhow::Error> {
    let query = match args.query {
        Some(q) => q,
        None => {
            println!("=== enclave cli (rust) ===");
            print!("enter your query: ");
            use std::io::{self, Write};
            io::stdout().flush()?;
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            input.trim().to_string()
        }
    };

    if query.is_empty() {
        anyhow::bail!("query cannot be empty");
    }

    // setup providers with workspace context
    let ws = cfg.workspace_dir.clone();
    let logger = Arc::new(session_logger::new(ws.clone()));

    let strategist_provider = factory::create_provider(
        &cfg.strategist_binary,
        ws.clone(),
        cfg.minimax_api_key.clone(),
        cfg.openai_api_key.clone(),
        cfg.anthropic_api_key.clone(),
        cfg.openrouter_api_key.clone(),
        Some(cfg.minimax_model.clone()),
        Some(cfg.minimax_base_url.clone()),
        Some(cfg.openrouter_model.clone()),
        Some(cfg.openrouter_base_url.clone()),
        cfg.autonomous_mode,
    );
    let critic_provider = factory::create_provider(
        &cfg.critic_binary,
        ws.clone(),
        cfg.minimax_api_key.clone(),
        cfg.openai_api_key.clone(),
        cfg.anthropic_api_key.clone(),
        cfg.openrouter_api_key.clone(),
        Some(cfg.minimax_model.clone()),
        Some(cfg.minimax_base_url.clone()),
        Some(cfg.openrouter_model.clone()),
        Some(cfg.openrouter_base_url.clone()),
        cfg.autonomous_mode,
    );
    let optimizer_provider = factory::create_provider(
        &cfg.optimizer_binary,
        ws.clone(),
        cfg.minimax_api_key.clone(),
        cfg.openai_api_key.clone(),
        cfg.anthropic_api_key.clone(),
        cfg.openrouter_api_key.clone(),
        Some(cfg.minimax_model.clone()),
        Some(cfg.minimax_base_url.clone()),
        Some(cfg.openrouter_model.clone()),
        Some(cfg.openrouter_base_url.clone()),
        cfg.autonomous_mode,
    );
    let contrarian_provider = factory::create_provider(
        &cfg.contrarian_binary,
        ws.clone(),
        cfg.minimax_api_key.clone(),
        cfg.openai_api_key.clone(),
        cfg.anthropic_api_key.clone(),
        cfg.openrouter_api_key.clone(),
        Some(cfg.minimax_model.clone()),
        Some(cfg.minimax_base_url.clone()),
        Some(cfg.openrouter_model.clone()),
        Some(cfg.openrouter_base_url.clone()),
        cfg.autonomous_mode,
    );
    let judge_provider = factory::create_provider(
        &cfg.judge_binary,
        ws.clone(),
        cfg.minimax_api_key.clone(),
        cfg.openai_api_key.clone(),
        cfg.anthropic_api_key.clone(),
        cfg.openrouter_api_key.clone(),
        Some(cfg.minimax_model.clone()),
        Some(cfg.minimax_base_url.clone()),
        Some(cfg.openrouter_model.clone()),
        Some(cfg.openrouter_base_url.clone()),
        cfg.autonomous_mode,
    );

    let mut agents = vec![
        roles::strategist(
            strategist_provider,
            "cli",
            cfg.default_temperature,
            cfg.max_tokens_per_agent,
        ),
        roles::critic(
            critic_provider,
            "cli",
            cfg.default_temperature,
            cfg.max_tokens_per_agent,
        ),
        roles::optimizer(
            optimizer_provider,
            "cli",
            cfg.default_temperature,
            cfg.max_tokens_per_agent,
        ),
        roles::contrarian(
            contrarian_provider,
            "cli",
            cfg.default_temperature,
            cfg.max_tokens_per_agent,
        ),
    ];

    // Set autonomous mode on all agents based on config
    for agent in &mut agents {
        agent.set_autonomous(cfg.autonomous_mode);
    }
    let judge = judge_agent::new(judge_provider, "cli", cfg.default_temperature, 1000);

    let mut orchestrator_inst = orchestrator::new(
        agents,
        judge,
        args.rounds.unwrap_or(cfg.max_rounds),
        cfg.autonomous_mode, // auto_rounds based on autonomous mode
        20,
        ws,
    );
    orchestrator_inst.logger = logger;

    println!("\n--- starting enclave session ---");
    println!("workspace: {}\n", cfg.workspace_dir.display());
    println!("query: {}\n", query);

    orchestrator_inst
        .run_council(&query, |resp| async move {
            println!("[{}] (round {}):", resp.agent, resp.round);
            println!("{}\n", resp.content);
            println!("{}", "-".repeat(40));
            Ok(())
        })
        .await?;

    Ok(())
}
