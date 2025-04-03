use crate::lib::AsyncGameList;
use crate::lib::IdStore;
use lib::GameMode;
use lib::{
    handlers::{accept_board, start_game, AsyncIdStore},
    host_connected, player_connected, Game, Round, RoundType, State,
};
use opentelemetry::trace::TracerProvider;
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    propagation::TraceContextPropagator,
    runtime,
    trace::{BatchConfig, Tracer},
    Resource,
};
use opentelemetry_semantic_conventions::resource::{
    DEPLOYMENT_ENVIRONMENT_NAME, SERVICE_NAME, SERVICE_VERSION,
};
use opentelemetry_semantic_conventions::SCHEMA_URL;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::Level;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use std::{error::Error, sync::Arc};
use tokio::sync::RwLock;

use warp::Filter;

pub mod lib;

#[derive(Deserialize)]
struct GameDefinition {
    rounds: Vec<RoundType>,
}

async fn end_game(games: AsyncGameList, lobby_id: String) -> String {
    let mut games = games.write().await;
    if let Some(Some(game)) = games.get(&lobby_id) {
        game.write().await.end();
        games.insert(lobby_id, None);
    }
    "Success".to_string()
}

#[derive(Serialize)]
struct Lobby {
    lobby_id: String,
    created: u128,
}

#[derive(Serialize)]
struct GameDetails {
    players: Vec<String>,
    categories: Vec<String>,
    mode: GameMode,
}

fn resource() -> Resource {
    Resource::from_schema_url(
        [
            KeyValue::new(SERVICE_NAME, env!("CARGO_PKG_NAME")),
            KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION")),
            KeyValue::new(DEPLOYMENT_ENVIRONMENT_NAME, "develop"),
        ],
        SCHEMA_URL,
    )
}
fn init_tracer() -> Tracer {
    // Allows you to pass along context (i.e., trace IDs) across services
    global::set_text_map_propagator(TraceContextPropagator::new());
    let provider = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_trace_config(opentelemetry_sdk::trace::Config::default().with_resource(resource()))
        .with_batch_config(BatchConfig::default())
        .with_exporter(
            opentelemetry_otlp::new_exporter()
                .tonic()
                .with_endpoint("http://otel-collector:4317"),
        )
        .install_batch(runtime::Tokio)
        .unwrap();

    global::set_tracer_provider(provider.clone());
    provider.tracer("rusty-jeopardy")
}

fn init_tracing_subscriber() {
    let tracer = init_tracer();
    tracing_subscriber::registry()
        .with(tracing_subscriber::filter::LevelFilter::from_level(
            Level::INFO,
        ))
        .with(tracing_subscriber::fmt::layer())
        .with(OpenTelemetryLayer::new(tracer))
        .init();
}

#[tokio::main]
async fn main() {
    init_tracing_subscriber();
    let id_store: AsyncIdStore = Arc::new(RwLock::new(IdStore::new()));

    let games: AsyncGameList = Arc::new(RwLock::new(HashMap::new()));
    let games_filter = warp::any().map(move || games.clone());

    let id_store_filter = warp::any().map(move || id_store.clone());
    let start_route = warp::post()
        .and(warp::path!("api" / "start" / usize))
        .and(games_filter.clone())
        .and(id_store_filter)
        .and(warp::body::content_length_limit(1024 * 32))
        .and(warp::filters::body::bytes())
        .and_then(start_game)
        .with(warp::trace::named("start_game"));

    let end_route = warp::post()
        .and(warp::path!("api" / "end" / String))
        .and(games_filter.clone())
        .and_then(|lobby_id, games| async move {
            Ok::<String, warp::Rejection>(end_game(games, lobby_id).await)
        });

    let games_route = warp::path!("api" / "games")
        .and(games_filter.clone())
        .and_then(|games: AsyncGameList| async move {
            let games = games.read().await;
            let mut resp: Vec<Lobby> = Vec::with_capacity(games.len());
            for (lobby_id, game) in games.iter() {
                if let Some(game) = game {
                    let game = game.read().await;
                    resp.push(Lobby {
                        lobby_id: lobby_id.to_string(),
                        created: game.created,
                    })
                }
            }

            match serde_json::to_string(&resp) {
                Ok(s) => Ok(s),
                Err(_) => Err(warp::reject()),
            }
        });

    let game_route = warp::path!("api" / "game" / String)
        .and(games_filter.clone())
        .and_then(|lobby_id: String, games: AsyncGameList| async move {
            let games = games.read().await;
            let game = match games.get(&lobby_id) {
                Some(Some(g)) => g,
                _ => return Err(warp::reject()),
            };

            let game = game.read().await;
            let round = &game.rounds[game.state.round_idx];
            let players = game.state.players.keys().map(|s| s.to_owned()).collect();
            let categories = round.get_categories();
            let mode = game.mode.clone();
            let resp = GameDetails {
                players,
                categories,
                mode,
            };
            match serde_json::to_string(&resp) {
                Ok(s) => Ok(s),
                Err(_) => Err(warp::reject()),
            }
        });

    let buzzer_route = warp::path!("api" / "ws" / String / "buzzer")
        .and(warp::ws())
        .and(games_filter.clone())
        .map(|lobby_id: String, ws: warp::ws::Ws, games: AsyncGameList| {
            ws.on_upgrade(move |ws| player_connected(games, lobby_id, ws))
        });

    let host_route = warp::path!("api" / "ws" / String / "host")
        .and(warp::ws())
        .and(games_filter.clone())
        .map(|lobby_id: String, ws: warp::ws::Ws, games: AsyncGameList| {
            ws.on_upgrade(move |ws| host_connected(games, lobby_id, ws))
        });

    let board_route = warp::path!("api" / "ws" / String / "board")
        .and(warp::ws())
        .and(games_filter.clone())
        .map(accept_board);

    let cors = warp::cors::cors().allow_any_origin();

    let http_routes = end_route
        .or(start_route)
        .or(games_route)
        .or(game_route)
        .with(warp::trace::request());

    warp::serve(
        buzzer_route
            .or(host_route)
            .or(board_route)
            .or(http_routes)
            .with(cors),
    )
    .run(([0, 0, 0, 0], 10001))
    .await;
}

enum JeopardyError {
    DeserializationError,
    ConnectionError,
}
