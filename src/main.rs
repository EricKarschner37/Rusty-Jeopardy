use lib::{
    handlers::{accept_board, start_game},
    host_connected, player_connected, Game, Round, RoundType, State,
};
use opentelemetry::global;
use opentelemetry_otlp::ExportConfig;
use opentelemetry_otlp::TonicConfig;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::TracerProvider;
use opentelemetry_stdout::SpanExporter;
use serde::{Deserialize, Serialize};

use std::{error::Error, sync::Arc};
use tokio::sync::RwLock;

use warp::Filter;

pub mod lib;

#[derive(Deserialize)]
struct GameDefinition {
    rounds: Vec<RoundType>,
}

async fn end_game(games: Arc<RwLock<Vec<Option<Arc<RwLock<Game>>>>>>, game_idx: usize) -> String {
    let mut games = games.write().await;
    if let Some(Some(game)) = games.get(game_idx) {
        game.write().await.end();
        games[game_idx] = None;
    }
    "Success".to_string()
}

#[derive(Serialize)]
struct GameOption {
    game_idx: usize,
    created: u128,
}

#[derive(Serialize)]
struct GameDetails {
    players: Vec<String>,
    categories: Vec<String>,
}

fn init_tracer() {
    // First, create a OTLP exporter builder. Configure it as you need.
    let otlp_exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint("http://otel-collector:4318");
    // Then pass it into pipeline builder
    let _ = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(otlp_exporter)
        .install_simple();
}

#[tokio::main]
async fn main() {
    init_tracer();

    let games: Arc<RwLock<Vec<Option<Arc<RwLock<Game>>>>>> =
        Arc::new(RwLock::new(Vec::with_capacity(10)));
    let games_filter = warp::any().map(move || games.clone());
    let start_route = warp::post()
        .and(warp::path!("api" / "start" / usize))
        .and(games_filter.clone())
        .and_then(start_game);

    let end_route = warp::post()
        .and(warp::path!("api" / "end" / usize))
        .and(games_filter.clone())
        .and_then(|game_idx, games| async move {
            Ok::<String, warp::Rejection>(end_game(games, game_idx).await)
        });

    let games_route = warp::path!("api" / "games")
        .and(games_filter.clone())
        .and_then(
            |games: Arc<RwLock<Vec<Option<Arc<RwLock<Game>>>>>>| async move {
                let games = games.read().await;
                let mut resp: Vec<GameOption> = Vec::with_capacity(games.len());
                for (i, game) in games.iter().enumerate() {
                    if let Some(game) = game {
                        resp.push(GameOption {
                            game_idx: i,
                            created: game.read().await.created,
                        })
                    }
                }

                match serde_json::to_string(&resp) {
                    Ok(s) => Ok(s),
                    Err(_) => Err(warp::reject()),
                }
            },
        );

    let game_route = warp::path!("api" / "game" / usize)
        .and(games_filter.clone())
        .and_then(
            |game_idx: usize, games: Arc<RwLock<Vec<Option<Arc<RwLock<Game>>>>>>| async move {
                let games = games.read().await;
                let game = match games.get(game_idx) {
                    Some(Some(g)) => g,
                    _ => return Err(warp::reject()),
                };

                let game = game.read().await;
                let round = &game.rounds[game.state.round_idx];
                let players = game.state.players.keys().map(|s| s.to_owned()).collect();
                let categories = round.get_categories();
                let resp = GameDetails {
                    players,
                    categories,
                };
                match serde_json::to_string(&resp) {
                    Ok(s) => Ok(s),
                    Err(_) => Err(warp::reject()),
                }
            },
        );

    let buzzer_route = warp::path!("api" / "ws" / usize / "buzzer")
        .and(warp::ws())
        .and(games_filter.clone())
        .map(
            |game_idx, ws: warp::ws::Ws, games: Arc<RwLock<Vec<Option<Arc<RwLock<Game>>>>>>| {
                ws.on_upgrade(move |ws| player_connected(games, game_idx, ws))
            },
        );

    let host_route = warp::path!("api" / "ws" / usize / "host")
        .and(warp::ws())
        .and(games_filter.clone())
        .map(
            |game_idx: usize,
             ws: warp::ws::Ws,
             games: Arc<RwLock<Vec<Option<Arc<RwLock<Game>>>>>>| {
                ws.on_upgrade(move |ws| host_connected(games, game_idx, ws))
            },
        );

    let board_route = warp::path!("api" / "ws" / usize / "board")
        .and(warp::ws())
        .and(games_filter.clone())
        .map(accept_board);

    let cors = warp::cors::cors().allow_any_origin();

    warp::serve(
        buzzer_route
            .or(host_route)
            .or(end_route)
            .or(board_route)
            .or(start_route)
            .or(games_route)
            .or(game_route)
            .with(cors),
    )
    .run(([0, 0, 0, 0], 10001))
    .await;
}

enum JeopardyError {
    DeserializationError,
    ConnectionError,
}
