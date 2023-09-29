use futures_util::{future::Ready, sink::With, TryFutureExt};
use lib::{
    board_connected, host_connected, player_connected, BoardData, FinalJeopardy, Game, State,
};
use serde::Serialize;

use std::{
    collections::HashMap,
    env,
    error::Error,
    future::ready,
    path::Path,
    process::Command,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::RwLock;

use warp::{reply::WithStatus, Filter, Reply};

pub mod lib;

const DEFAULT_GAME_PREFIX: &str = "games/";
const GAME_PREFIX_NAME: &str = "JEOPARDY_GAME_ROOT";

fn game_exists(num: usize) -> bool {
    let prefix = env::var(GAME_PREFIX_NAME).unwrap_or(DEFAULT_GAME_PREFIX.to_string());
    let game_dir = format!("{}{}", prefix, num);
    let game_dir = Path::new(&game_dir);
    if !game_dir.is_dir() {
        return false;
    }

    for filename in [
        "single_clues",
        "single_responses",
        "double_clues",
        "double_responses",
        "final",
    ] {
        if !game_dir.join(format!("{}.csv", filename)).is_file() {
            println!("{}", filename);
            return false;
        }
    }

    true
}

fn ensure_game_exists(num: usize) -> Result<(), Box<dyn Error>> {
    if game_exists(num) {
        return Ok(());
    }

    let mut c = Command::new("python");
    c.arg("get_game.py");
    c.arg(num.to_string());

    match c.status() {
        Ok(_) => Ok(()),
        Err(e) => Err(Box::new(e)),
    }
}

fn read_round(
    mut clue_rdr: csv::Reader<std::fs::File>,
    mut response_rdr: csv::Reader<std::fs::File>,
) -> Result<BoardData, Box<dyn Error>> {
    let mut clues: [[String; 6]; 5] = Default::default();
    let mut responses: [[String; 6]; 5] = Default::default();
    let mut categories: [String; 6] = Default::default();

    for i in 0..=4 {
        let clue_record = match clue_rdr.records().next() {
            Some(r) => r?,
            None => return Err(From::from("Not enough clues to unpack")),
        };
        let response_record = match response_rdr.records().next() {
            Some(r) => r?,
            None => return Err(From::from("Not enough responses to unpack")),
        };

        for j in 0..=5 {
            if i == 0 {
                categories[j] = clue_rdr.headers()?[j].to_string();
            }
            clues[i][j] = clue_record[j].to_string();
            responses[i][j] = response_record[j].to_string();
        }
    }

    Ok(BoardData {
        categories,
        clues,
        responses,
    })
}

fn read_final(mut final_rdr: csv::Reader<std::fs::File>) -> Result<FinalJeopardy, Box<dyn Error>> {
    let clue = match final_rdr.records().next() {
        Some(r) => r?[0].to_string(),
        None => return Err(From::from("No final jeopardy clue")),
    };

    let response = match final_rdr.records().next() {
        Some(r) => r?[0].to_string(),
        None => return Err(From::from("No final jeopardy response")),
    };

    let category = final_rdr.headers()?[0].to_string();

    Ok(FinalJeopardy {
        clue,
        response,
        category,
        wagers: HashMap::new(),
        player_responses: HashMap::new(),
    })
}

async fn start_game(
    games: Arc<RwLock<Vec<Option<Arc<RwLock<Game>>>>>>,
    num: usize,
) -> WithStatus<String> {
    if let Err(e) = ensure_game_exists(num) {
        eprintln!("Error fetching game {}: {}", num, e);
        eprintln!("(Couldn't ensure it exists)");
        return warp::reply::with_status(
            format!("Error: no game #{} found", num),
            warp::http::StatusCode::NOT_FOUND,
        );
    }

    let prefix = env::var(GAME_PREFIX_NAME).unwrap_or(DEFAULT_GAME_PREFIX.to_string());
    let game_dir = format!("{}{}", prefix, num);
    let game_dir = Path::new(&game_dir);

    let clue_rdr = match csv::Reader::from_path(game_dir.join("single_clues.csv")) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error fetching game {}: {}", num, e);
            return warp::reply::with_status(
                format!("Error: game #{} not found", num),
                warp::http::StatusCode::NOT_FOUND,
            );
        }
    };

    let resp_rdr = match csv::Reader::from_path(game_dir.join("single_responses.csv")) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error fetching game {}: {}", num, e);
            return warp::reply::with_status(
                format!("Error: no game #{} not found", num),
                warp::http::StatusCode::NOT_FOUND,
            );
        }
    };

    let single_jeopardy = match read_round(clue_rdr, resp_rdr) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error reading game {}: {}", num, e);
            return warp::reply::with_status(
                format!("Error loading game #{}: {}", num, e),
                warp::http::StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };

    let clue_rdr = match csv::Reader::from_path(game_dir.join("double_clues.csv")) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error fetching game {}: {}", num, e);
            return warp::reply::with_status(
                format!("Error: no game #{} not found", num),
                warp::http::StatusCode::NOT_FOUND,
            );
        }
    };

    let resp_rdr = match csv::Reader::from_path(game_dir.join("double_responses.csv")) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error fetching game {}: {}", num, e);
            return warp::reply::with_status(
                format!("Error: no game #{} not found", num),
                warp::http::StatusCode::NOT_FOUND,
            );
        }
    };

    let double_jeopardy = match read_round(clue_rdr, resp_rdr) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error reading game {}: {}", num, e);
            return warp::reply::with_status(
                format!("Error loading game #{}: {}", num, e),
                warp::http::StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };

    let final_rdr = match csv::Reader::from_path(game_dir.join("final.csv")) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error fetching game {}: {}", num, e);
            return warp::reply::with_status(
                format!("Error: game #{} not found", num),
                warp::http::StatusCode::NOT_FOUND,
            );
        }
    };

    let final_jeopardy = match read_final(final_rdr) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error fetching game {}: {}", num, e);
            return warp::reply::with_status(
                format!("Error: game #{} not found", num),
                warp::http::StatusCode::NOT_FOUND,
            );
        }
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
    games.push(Some(Arc::new(RwLock::new(Game {
        state: State::new(),
        host_tx: None,
        board_tx: None,
        single_jeopardy,
        double_jeopardy,
        final_jeopardy,
        created: timestamp,
    }))));

    let msg = GameCreatedMessage {
        message: "Game created successfully",
        game_idx: games.len() - 1,
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
    game_idx: usize,
}

#[derive(Serialize)]
struct GameOption {
    game_idx: usize,
    created: u128,
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
            print!("{}", num);
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
            .with(cors),
    )
    .run(([0, 0, 0, 0], 10001))
    .await;
}

enum JeopardyError {
    DeserializationError,
    ConnectionError,
}
