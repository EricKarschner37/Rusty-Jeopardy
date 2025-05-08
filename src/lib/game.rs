use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    thread,
    time::{Duration, SystemTime},
};

use futures_util::{future::BoxFuture, FutureExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, RwLock};
use warp::ws::Message;
use futures::executor::block_on;

use super::player::Player;

pub trait Round {
    fn get_categories(&self) -> Vec<String>;
    fn get_name(&self) -> String;
}

#[derive(Deserialize, Clone, Debug)]
pub struct Clue {
    pub cost: i32,
    pub clue: String,
    pub response: String,
    pub is_daily_double: bool,
    pub media_url: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Category {
    pub category: String,
    pub clues: Vec<Clue>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "round_type")]
pub enum RoundType {
    DefaultRound {
        categories: Vec<Category>,
        name: String,
        default_max_wager: i32,
    },
    FinalRound {
        category: String,
        name: String,
        clue: String,
        response: String,
        default_max_wager: i32,
    },
}

#[derive(Serialize, Debug)]
pub struct BareCategory {
    pub category: String,
    pub clue_costs: Vec<i32>,
}

#[derive(Serialize, Debug)]
#[serde(tag = "round_type")]
pub enum BareRoundType {
    DefaultRound {
        categories: Vec<BareCategory>,
        name: String,
        default_max_wager: i32,
    },
    FinalRound {
        category: String,
        name: String,
        default_max_wager: i32,
    },
}

impl RoundType {
    pub fn to_bare_round(self) -> BareRoundType {
        match self {
            RoundType::DefaultRound {
                categories,
                name,
                default_max_wager,
            } => {
                let categories = categories
                    .into_iter()
                    .map(|category| {
                        let clue_costs = category.clues.into_iter().map(|clue| clue.cost).collect();
                        BareCategory {
                            clue_costs,
                            category: category.category,
                        }
                    })
                    .collect();
                BareRoundType::DefaultRound {
                    name,
                    categories,
                    default_max_wager,
                }
            }
            RoundType::FinalRound {
                category,
                name,
                default_max_wager,
                ..
            } => BareRoundType::FinalRound {
                category,
                name,
                default_max_wager,
            },
        }
    }
}

impl Round for RoundType {
    fn get_categories(&self) -> Vec<String> {
        match self {
            RoundType::DefaultRound { categories, .. } => {
                categories.iter().map(|c| c.category.clone()).collect()
            }
            RoundType::FinalRound { category, .. } => vec![category.clone()],
        }
    }

    fn get_name(&self) -> String {
        match self {
            RoundType::DefaultRound { name, .. } => name.clone(),
            RoundType::FinalRound { name, .. } => name.clone(),
        }
    }
}

#[derive(Debug)]
pub struct Game {
    pub rounds: Vec<RoundType>,
    pub state: State,
    pub host_tx: Option<mpsc::UnboundedSender<Message>>,
    pub board_tx: Option<mpsc::UnboundedSender<Message>>,
    pub created: u128,
    pub mode: GameMode,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GameMode {
    Host,
    Hostless,
}

#[derive(Deserialize)]
pub struct BaseMessage {
    pub request: String,
}

#[derive(Deserialize)]
pub struct PlayerMessage {
    pub request: String,
    pub player: String,
}

#[derive(Serialize)]
struct StateMessage<'a> {
    message: &'a str,
    #[serde(flatten)]
    state: &'a State,
}

#[derive(Serialize)]
struct CategoriesMessage<'a> {
    message: &'a str,
    categories: &'a Vec<String>,
}

#[derive(Deserialize)]
pub struct RevealMessage {
    pub request: String,
    pub row: usize,
    pub col: usize,
}

impl Game {
    pub fn send_categories(&self) {
        let categories = self.rounds[self.state.round_idx].get_categories();

        let msg = CategoriesMessage {
            message: "categories",
            categories: &categories,
        };

        let cat_str = match serde_json::to_string(&msg) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error serializing categories: {}", e);
                return;
            }
        };

        let msg = Message::text(cat_str);
        self.send_to_all(msg);
    }

    fn send_to_all(&self, msg: Message) {
        if let Some(tx) = self.board_tx.as_ref() {
            tx.send(msg.clone());
        }
        for player in self.state.players.values() {
            if let Some(tx) = player.tx.as_ref() {
                tx.send(msg.clone());
            }
        }
        if let Some(tx) = self.host_tx.as_ref() {
            tx.send(msg);
        }
    }

    pub fn send_state(&self) {
        let state = StateMessage {
            message: "state",
            state: &self.state,
        };

        let state_str = match serde_json::to_string(&state) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error serializing state: {}", e);
                return;
            }
        };

        let state_msg = Message::text(state_str);
        self.send_to_all(state_msg);
    }

    pub fn evaluate_final_responses(&mut self) {
        let mut player = None;
        let mut response = "";

        let correct_response = match &self.rounds[self.state.round_idx] {
            RoundType::FinalRound { response, .. } => response,
            _ => return,
        };

        for p in self.state.players.keys() {
            if let Some(Some(r)) = self.state.player_responses.get(p) {
                player = Some(p.clone());
                response = r;
                break;
            }
        }
        let player = match player {
            Some(p) => p,
            None => {
                self.state.state_type = StateType::Response;
                self.state.buzzed_player = None;
                self.send_state();
                return;
            }
        };

        self.state.state_type = StateType::Clue;
        self.state.response = format!(
            "{}'s response: {}\nCorrect response: {}",
            player, response, correct_response
        );

        self.state.cost = match self.state.wagers.get(&player) {
            Some(Some(a)) => *a,
            _ => 3000,
        };
        self.state.buzzers_open = true;
        self.buzz(&player);

        self.state.player_responses.remove(&player);
        self.state.wagers.remove(&player);

        self.send_state();
    }

    pub fn show_response(&mut self) {
        if self.state.buzzers_open || self.state.buzzed_player.is_some() {
            return;
        }
        self.state.state_type = StateType::Response;
        self.state.responded_players.clear();
        self.send_state();
    }

    pub fn end(&mut self) {
        self.send_to_all(Message::close());
    }

    pub fn set_buzzers_open(&mut self, open: bool, game_lock: Arc<RwLock<Game>>) {
        self.state.buzzers_open = open;
        if self.mode == GameMode::Hostless && open {
            let timer = Duration::from_secs(10);
            self.state.timer_end_secs = Some(get_utc_now(Some(timer)));
            set_timeout(timer, move || {
                {
                    let game_lock = game_lock.clone();
                    async move {
                        let mut game = game_lock.write().await;
                        game.set_buzzers_open(false, game_lock.clone());
                        game.state.timer_end_secs = None;
                    }
                }
                .boxed()
            })
        }
        self.send_state();
    }

    pub fn reveal(&mut self, row: usize, col: usize, game_lock: Arc<RwLock<Game>>) {
        if row > 5 || col > 6 {
            return;
        }

        let board = &self.rounds[self.state.round_idx];
        let categories = match board {
            RoundType::FinalRound { .. } => return,
            RoundType::DefaultRound { categories, .. } => categories,
        };

        let bitset_key = 1 << (row * 6 + col);

        let clue_obj = &categories[col].clues[row];
        self.state.clue = clue_obj.clue.clone();
        self.state.response = clue_obj.response.clone();
        self.state.category = categories[col].category.clone();
        self.state.cost = clue_obj.cost;
        self.state.state_type = if clue_obj.is_daily_double {
            StateType::DailyDouble
        } else {
            StateType::Clue
        };
        self.state.media_url = clue_obj.media_url.clone();

        self.state.clues_shown |= bitset_key;

        if self.mode == GameMode::Hostless {
            let timer = Duration::from_secs(10);
            self.state.timer_end_secs = Some(get_utc_now(Some(timer)));
            set_timeout(timer, move || {
                    let game_lock = game_lock.clone();
                    async move {
                        let mut game = game_lock.write().await;
                        game.set_buzzers_open(true, game_lock.clone());
                        game.state.timer_end_secs = None;
                        game.send_state();
                    }
                .boxed()
            }
            )
        }
    }

    pub fn correct(&mut self, correct: bool) {
        println!("in correct handler");
        if let Some(player) = &self.state.buzzed_player {
            self.state.players.entry(player.clone()).and_modify(|p| {
                p.balance += if correct {
                    self.state.cost
                } else {
                    -self.state.cost
                };
            });

            if let RoundType::FinalRound { .. } = self.rounds[self.state.round_idx] {
                self.evaluate_final_responses();
                self.send_state();
                return;
            }

            if correct {
                self.state.active_player = Some(player.clone());
            }

            if correct || self.state.responded_players.len() == self.state.players.keys().len() {
                self.state.buzzed_player = None;
                self.state.buzzers_open = false;
                self.show_response();
            } else {
                self.state.buzzed_player = None;
                self.state.buzzers_open = true;
                self.send_state();
            }
        } else {
            self.state.buzzers_open = true;
        }
    }
}

fn get_utc_now(offset: Option<Duration>) -> u64 {
    let offset = match offset {
        Some(offset) => offset,
        None => Duration::new(0, 0),
    };
    (SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        + offset)
        .as_secs()
}

fn set_timeout<F>(timeout: Duration, mut callback: F)
where
    F: (FnMut() -> BoxFuture<'static, ()>) + std::marker::Send + 'static,
{
    thread::spawn(move || {
        thread::sleep(timeout);
        block_on(callback())
    });
}

#[derive(Serialize, Debug)]
pub struct State {
    pub state_type: StateType,
    pub buzzers_open: bool,
    pub buzzed_player: Option<String>,
    pub active_player: Option<String>,
    pub responded_players: HashSet<String>,
    pub cost: i32,
    pub category: String,
    pub clue: String,
    pub media_url: Option<String>,
    pub response: String,
    pub players: HashMap<String, Player>,
    pub clues_shown: u32,
    pub wagers: HashMap<String, Option<i32>>,
    pub player_responses: HashMap<String, Option<String>>,
    pub bare_round: BareRoundType,
    pub round_idx: usize,
    pub timer_end_secs: Option<u64>,
}

#[derive(Serialize, PartialEq, Debug)]
pub enum StateType {
    Response,
    Clue,
    Board,
    DailyDouble,
    FinalWager,
    FinalClue,
}

impl State {
    pub fn new(first_round: &RoundType) -> Self {
        Self {
            state_type: StateType::Board,
            buzzers_open: false,
            buzzed_player: None,
            active_player: None,
            cost: 0,
            category: "Welcome to Jeopardy!".to_string(),
            clue: "Please wait for the game to start.".to_string(),
            media_url: None,
            response: "I'm sure that'll be soon".to_string(),
            players: HashMap::new(),
            responded_players: HashSet::new(),
            clues_shown: 0,
            wagers: HashMap::new(),
            player_responses: HashMap::new(),
            bare_round: first_round.clone().to_bare_round(),
            round_idx: 0,
            timer_end_secs: None,
        }
    }
}
