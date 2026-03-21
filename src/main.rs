mod agents;
mod core;
mod api;
mod utils;
mod cli;

use std::sync::Arc;
use axum::{
    routing::{get, post},
    Router,
};
use tower_http::services::ServeDir;
use tower_http::cors::CorsLayer;
use clap::Parser;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::utils::config;
use crate::cli::cli_args;
use crate::core::orchestrator;
use crate::core::providers_mod::{cli_provider, model_provider};
use crate::agents::{roles, judge::judge_agent};
use crate::api::session_store;
use crate::utils::logger_mod::session_logger;

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
    let app = Router::new()
        .route("/api/council", get(api::handle_council))
        .route("/api/browse", get(api::routes::browse_workspace))
        .route("/api/test_cli", post(api::routes::test_cli))
        .route("/api/apply", post(api::routes::apply_change))
        .route("/api/history/:session_id", get(api::routes::get_session_history))
        .route("/api/sessions", get(api::routes::list_sessions))
        .route("/api/sessions/:session_id", axum::routing::delete(api::routes::delete_session))
        .nest_service("/static", ServeDir::new("src/ui"))
        .route("/", get(|| async { axum::response::Html(include_str!("ui/index.html")) }))
        .with_state((cfg.clone(), store.clone()))
        .layer(CorsLayer::permissive());

    let addr = format!("{}:{}", cfg.host, cfg.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("server listening on http://{}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn run_cli(cfg: Arc<config>, args: cli_args) -> Result<(), anyhow::Error> {
    let query = match args.query {
        Some(q) => q,
        None => {
            println!("=== council agent cli (rust) ===");
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

    let strategist_provider: Arc<dyn model_provider> = Arc::new(cli_provider::new(cfg.strategist_binary.clone(), ws.clone()).with_logger(logger.clone()));
    let critic_provider: Arc<dyn model_provider> = Arc::new(cli_provider::new(cfg.critic_binary.clone(), ws.clone()).with_logger(logger.clone()));
    let optimizer_provider: Arc<dyn model_provider> = Arc::new(cli_provider::new(cfg.optimizer_binary.clone(), ws.clone()).with_logger(logger.clone()));
    let contrarian_provider: Arc<dyn model_provider> = Arc::new(cli_provider::new(cfg.contrarian_binary.clone(), ws.clone()).with_logger(logger.clone()));
    let judge_provider: Arc<dyn model_provider> = Arc::new(cli_provider::new(cfg.judge_binary.clone(), ws.clone()).with_logger(logger.clone()));

    let agents = vec![
        roles::strategist(strategist_provider, "cli", cfg.default_temperature, cfg.max_tokens_per_agent),
        roles::critic(critic_provider, "cli", cfg.default_temperature, cfg.max_tokens_per_agent),
        roles::optimizer(optimizer_provider, "cli", cfg.default_temperature, cfg.max_tokens_per_agent),
        roles::contrarian(contrarian_provider, "cli", cfg.default_temperature, cfg.max_tokens_per_agent),
    ];
    let judge = judge_agent::new(judge_provider, "cli", cfg.default_temperature, 1000);

    let mut orchestrator_inst = orchestrator::new(
        agents,
        judge,
        args.rounds.unwrap_or(cfg.max_rounds),
        20,
        ws
    );
    orchestrator_inst.logger = logger;

    println!("\n--- starting council session ---");
    println!("workspace: {}\n", cfg.workspace_dir.display());
    println!("query: {}\n", query);

    orchestrator_inst.run_council(&query, |resp| async move {
        println!("[{}] (round {}):", resp.agent, resp.round);
        println!("{}\n", resp.content);
        println!("{}", "-".repeat(40));
        Ok(())
    }).await?;

    Ok(())
}
