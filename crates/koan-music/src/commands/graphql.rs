use koan_core::config;
use koan_core::player::Player;

use super::open_db;
use crate::graphql::build_schema;

/// Entry point for `koan graphql` — starts a headless player with a GraphQL HTTP server.
pub fn cmd_graphql(port_override: Option<u16>, playground_override: bool) {
    // Validate DB is accessible before starting.
    let _db = open_db();
    let db_path = config::db_path();
    let cfg = config::Config::load().unwrap_or_default();

    let port = port_override.unwrap_or(cfg.graphql.port);
    let playground = playground_override || cfg.graphql.playground;

    // Spawn the player engine (headless — no TUI).
    let (state, _timeline, _viz, cmd_tx) = Player::spawn();

    let schema = build_schema(state, cmd_tx, db_path);

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async move {
        use axum::{Extension, Router, routing::post};

        let app = if playground {
            use async_graphql::http::{GraphQLPlaygroundConfig, playground_source};
            use axum::response::Html;

            let playground_handler =
                || async { Html(playground_source(GraphQLPlaygroundConfig::new("/graphql"))) };

            Router::new()
                .route("/graphql", post(graphql_handler).get(playground_handler))
                .layer(Extension(schema))
        } else {
            Router::new()
                .route("/graphql", post(graphql_handler))
                .layer(Extension(schema))
        };

        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        eprintln!("koan graphql server listening on http://{addr}/graphql");
        if playground {
            eprintln!("GraphQL Playground: http://{addr}/graphql");
        }

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .expect("failed to bind");
        axum::serve(listener, app).await.expect("server error");
    });
}

async fn graphql_handler(
    schema: axum::Extension<crate::graphql::KoanSchema>,
    req: async_graphql_axum::GraphQLRequest,
) -> async_graphql_axum::GraphQLResponse {
    schema.execute(req.into_inner()).await.into()
}
