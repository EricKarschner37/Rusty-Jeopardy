use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use warp::ws::Message;

use super::player::Player;

pub trait Round {
    fn get_categories(&self) -> Vec<String>;
    fn get_name(&self) -> String;
}

#[derive(Deserialize)]
pub struct Clue {
    pub cost: i32,
    pub clue: String,
    pub response: String,
    pub is_daily_double: bool,
}

#[derive(Deserialize)]
pub struct Category {
    pub category: String,
    pub clues: Vec<Clue>,
}

#[derive(Deserialize)]
pub struct BaseRound {
    pub type: String,
}

#[derive(Deserialize)]
pub struct DefaultRound {
    pub type: String,
    pub categories: Vec<Category>,
    pub name: String,
}

impl Round for DefaultRound {
    fn get_categories(&self) -> Vec<String> {
        self.categories.iter().map(|c| c.category.clone()).collect()
    }

    fn get_name(&self) -> String {
        self.name.clone()
    }
}

#[derive(Deserialize)]
pub struct FinalRound {
    pub type: String,
    pub category: String,
    pub clue: String,
    pub response: String,
    pub name: String,

    #[serde(skip))]
    pub wagers: HashMap<String, Option<i32>>,
    #[serde(skip))]
    pub player_responses: HashMap<String, Option<String>>,
}

impl Round for FinalRound {
    fn get_categories(&self) -> Vec<String> {
        vec![self.category.clone()]
    }

    fn get_name(&self) -> String {
        self.name.clone()
    }
}

pub struct FinalJeopardy {
    pub clue: String,
    pub response: String,
    pub category: String,
    pub wagers: HashMap<String, Option<i32>>,
    pub player_responses: HashMap<String, Option<String>>,
}

pub struct Game {
    pub rounds: Vec<Box<dyn Round>>,
    pub state: State,
    pub round_idx: usize,
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
    categories: &'a [String; 6],
}

impl Game {
    pub fn send_categories(&self) {
        let categories = match self.state.round {
            Round::Single => &self.single_jeopardy.categories,
            Round::Double => &self.double_jeopardy.categories,
            Round::Final => {
                return;
            }
        };

        let msg = CategoriesMessage {
            message: "categories",
            categories,
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
        for p in self.state.players.keys() {
            if let Some(Some(r)) = self.final_jeopardy.player_responses.get(p) {
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
            player, response, self.final_jeopardy.response
        );

        self.state.cost = match self.final_jeopardy.wagers.get(&player) {
            Some(Some(a)) => *a,
            _ => 3000,
        };
        self.state.buzzers_open = true;
        self.buzz(&player);

        self.final_jeopardy.player_responses.remove(&player);
        self.final_jeopardy.wagers.remove(&player);

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

#[derive(Serialize)]
pub struct State {
    pub state_type: StateType,
    pub categories: Vec<String>,
    pub buzzers_open: bool,
    pub buzzed_player: Option<String>,
    pub active_player: Option<String>,
    pub responded_players: HashSet<String>,
    pub cost: i32,
    pub category: String,
    pub clue: String,
    pub response: String,
    pub players: HashMap<String, Player>,
    pub round_name: String,
    pub clues_shown: u32,
}

#[derive(Serialize, PartialEq)]
pub enum StateType {
    Response,
    Clue,
    Board,
    DailyDouble,
    FinalWager,
    FinalClue,
}

impl State {
    pub fn new() -> Self {
        Self {
            state_type: StateType::Board,
            categories: vec![
                "Category 1".to_string(),
                "Category 2".to_string(),
                "Category 3".to_string(),
                "Category 4".to_string(),
                "Category 5".to_string(),
                "Category 6".to_string(),
            ],
            buzzers_open: false,
            buzzed_player: None,
            active_player: None,
            cost: 0,
            category: "Welcome to Jeopardy!".to_string(),
            clue: "Please wait for the game to start.".to_string(),
            response: "I'm sure that'll be soon".to_string(),
            players: HashMap::new(),
            responded_players: HashSet::new(),
            round_name: "Jeopardy! Round".to_string(),
            clues_shown: 0,
        }
    }
}
