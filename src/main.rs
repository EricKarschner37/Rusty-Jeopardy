use lib::{board_connected, host_connected, player_connected, Game, Round, RoundType, State};
use serde::{Deserialize, Serialize};

use std::{
    env,
    error::Error,
    fs,
    path::Path,
    process::Command,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::RwLock;

use warp::{reply::WithStatus, Filter};

pub mod lib;

const DEFAULT_GAME_PREFIX: &str = "games/";
const GAME_PREFIX_NAME: &str = "JEOPARDY_GAME_ROOT";

#[derive(Deserialize)]
struct GameDefinition {
    rounds: Vec<RoundType>,
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

async fn start_game(
    games: Arc<RwLock<Vec<Option<Arc<RwLock<Game>>>>>>,
    num: usize,
) -> WithStatus<String> {
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

async fn end_game(games: Arc<RwLock<Vec<Option<Arc<RwLock<Game>>>>>>, game_idx: usize) -> String {
    let mut games = games.write().await;
    if let Some(Some(game)) = games.get(game_idx) {
        game.write().await.end();
        games[game_idx] = None;
    }
    "Success".to_string()
}

#[derive(Serialize)]
struct GameCreatedMessage<'a> {
    message: &'a str,
    gameIndex: usize,
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

#[tokio::main]
async fn main() {
    let games: Arc<RwLock<Vec<Option<Arc<RwLock<Game>>>>>> =
        Arc::new(RwLock::new(Vec::with_capacity(10)));
    let games_filter = warp::any().map(move || games.clone());
    let start_route = warp::post()
        .and(warp::path!("api" / "start" / usize))
        .and(games_filter.clone())
        .and_then(|num, games| async move {
            return Ok::<WithStatus<String>, warp::Rejection>(start_game(games, num).await);
        });

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
        .map(
            |game_idx: usize,
             ws: warp::ws::Ws,
             games: Arc<RwLock<Vec<Option<Arc<RwLock<Game>>>>>>| {
                ws.on_upgrade(move |ws| board_connected(games, game_idx, ws))
            },
        );

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
