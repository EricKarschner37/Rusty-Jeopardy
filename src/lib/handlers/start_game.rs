use crate::lib::AsyncGameList;
use crate::Error;
use crate::Game;
use crate::GameDefinition;
use crate::State;
use opentelemetry::trace::{Span, SpanKind, Status};
use opentelemetry::{global, trace::Tracer};
use serde::Serialize;
use std::path::Path;
use std::{
    env, fs,
    process::Command,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::RwLock;
use warp::reply::WithStatus;

#[derive(Serialize)]
struct GameCreatedMessage<'a> {
    message: &'a str,
    gameIndex: usize,
}

const DEFAULT_GAME_PREFIX: &str = "games/";
const GAME_PREFIX_NAME: &str = "JEOPARDY_GAME_ROOT";

#[tracing::instrument]
pub async fn start_game(
    num: usize,
    games: AsyncGameList,
) -> Result<WithStatus<String>, warp::Rejection> {
    let tracer = global::tracer("rusty_jeopardy");
    let mut span = tracer
        .span_builder("POST /api/start/:num")
        .with_kind(SpanKind::Server)
        .start(&tracer);

    let result = Ok::<WithStatus<String>, warp::Rejection>(create_game(games, num).await);

    span.set_status(Status::Ok);
    span.end();

    result
}

fn read_game(game_path: &Path) -> Result<GameDefinition, Box<dyn Error + Send>> {
    let data = fs::read_to_string(game_path);
    let data = match data {
        Ok(string) => string,
        Err(e) => return Err(Box::new(e)),
    };
    let res = serde_json::from_str(&data);
    if let Err(e) = &res {
        println!("{}", e);
    }
    match res {
        Ok(def) => Ok(def),
        Err(e) => Err(Box::new(e)),
    }
}

fn read_game_or_fetch(game_name: String) -> Result<GameDefinition, Box<dyn Error + Send>> {
    let prefix = env::var(GAME_PREFIX_NAME).unwrap_or(DEFAULT_GAME_PREFIX.to_string());
    let game_path = format!("{}{}.json", prefix, &game_name);
    println!("{}", game_path);
    let game_path = Path::new(&game_path);

    let game = read_game(game_path);
    if let Ok(game) = game {
        return Ok(game);
    }

    let mut c = Command::new("get_game.py");
    c.arg(game_name);

    match c.status() {
        Ok(_) => read_game(game_path),
        Err(e) => Err(Box::new(e)),
    }
}

async fn create_game(games: AsyncGameList, num: usize) -> WithStatus<String> {
    let game_result = read_game_or_fetch(num.to_string());
    let game_def = match game_result {
        Err(e) => {
            eprintln!("Error fetching game {}: {}", num, e);
            eprintln!("(Couldn't ensure it exists)");
            return warp::reply::with_status(
                format!("Error: no game #{} found", num),
                warp::http::StatusCode::NOT_FOUND,
            );
        }
        Ok(g) => g,
    };

    let timestamp = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis(),
        Err(e) => {
            return warp::reply::with_status(
                format!(
                    "something went wrong getting the timestamp for the new game: {}",
                    e
                ),
                warp::http::StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };

    let mut games = games.write().await;

    let game = Game {
        state: State::new(&game_def.rounds[0]),
        host_tx: None,
        board_tx: None,
        rounds: game_def.rounds,
        created: timestamp,
    };

    games.push(Some(Arc::new(RwLock::new(game))));

    let msg = GameCreatedMessage {
        message: "Game created successfully",
        gameIndex: games.len() - 1,
    };

    let resp = serde_json::to_string(&msg);
    println!("started game");
    match resp {
        Ok(s) => warp::reply::with_status(s, warp::http::StatusCode::OK),
        Err(e) => warp::reply::with_status(
            format!("Sorry, something went wrong: {}", e),
            warp::http::StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}
