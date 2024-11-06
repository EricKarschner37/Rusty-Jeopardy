use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use warp::ws::Message;

use crate::GameDefinition;

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

        println!("{}", cat_str);
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
    pub response: String,
    pub players: HashMap<String, Player>,
    pub clues_shown: u32,
    pub wagers: HashMap<String, Option<i32>>,
    pub player_responses: HashMap<String, Option<String>>,
    pub bare_round: BareRoundType,
    pub round_idx: usize,
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
            response: "I'm sure that'll be soon".to_string(),
            players: HashMap::new(),
            responded_players: HashSet::new(),
            clues_shown: 0,
            wagers: HashMap::new(),
            player_responses: HashMap::new(),
            bare_round: first_round.clone().to_bare_round(),
            round_idx: 0,
        }
    }
}
