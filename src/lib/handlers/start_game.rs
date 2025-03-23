use crate::lib::AsyncGameList;
use crate::Error;
use crate::Game;
use crate::GameDefinition;
use crate::State;
use opentelemetry::propagation::TextMapPropagator;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use reqwest::header::{HeaderName, HeaderValue};
use serde::Serialize;
use std::{collections::HashMap, path::Path};
use std::{
    env, fs,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::RwLock;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use warp::reply::WithStatus;

use super::AsyncIdStore;

#[derive(Serialize)]
struct GameCreatedMessage<'a> {
    message: &'a str,
    lobby_id: String,
}

const DEFAULT_GAME_PREFIX: &str = "games/";
const GAME_PREFIX_NAME: &str = "JEOPARDY_GAME_ROOT";

#[tracing::instrument]
pub async fn start_game(
    num: usize,
    games: AsyncGameList,
    id_store: AsyncIdStore,
) -> Result<WithStatus<String>, warp::Rejection> {
    let id = id_store.write().await.take();

    match id {
        Some(id) => Ok(create_game(games, num, id).await),
        None => Err(warp::reject()),
    }
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

async fn read_game_or_fetch(game_name: String) -> Result<GameDefinition, Box<dyn Error + Send>> {
    let prefix = env::var(GAME_PREFIX_NAME).unwrap_or(DEFAULT_GAME_PREFIX.to_string());
    let game_path = format!("{}{}.json", prefix, &game_name);
    println!("{}", game_path);
    let game_path = Path::new(&game_path);

    let game = read_game(game_path);
    if let Ok(game) = game {
        return Ok(game);
    }

    let span = tracing::Span::current();
    let context = span.context();
    let propagator = TraceContextPropagator::new();
    let mut fields = HashMap::new();
    propagator.inject_context(&context, &mut fields);
    let headers = fields
        .into_iter()
        .map(|(k, v)| {
            (
                HeaderName::try_from(k).unwrap(),
                HeaderValue::try_from(v).unwrap(),
            )
        })
        .collect();

    let url = format!("http://fetchardy/{}", &game_name);
    let client = reqwest::Client::builder().use_rustls_tls().build();
    let resp = client
        .expect("couldn't unwrap client")
        .get(url)
        .headers(headers)
        .send()
        .await;

    let game_id = match resp {
        Ok(resp) => resp.text().await,
        Err(e) => return Err(Box::new(e)),
    };

    println!("{game_id:#?}");

    match game_id {
        Ok(_) => read_game(game_path),
        Err(e) => Err(Box::new(e)),
    }
}

async fn create_game(games: AsyncGameList, num: usize, id: String) -> WithStatus<String> {
    let game_result = read_game_or_fetch(num.to_string());
    let game_def = match game_result.await {
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

    games.insert(id.clone(), Some(Arc::new(RwLock::new(game))));

    let msg = GameCreatedMessage {
        message: "Game created successfully",
        lobby_id: id,
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
