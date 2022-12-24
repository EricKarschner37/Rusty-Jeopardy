use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use warp::ws::Message;

use super::player::Player;

pub struct BoardData {
    pub categories: [String; 6],
    pub clues: [[String; 6]; 5],
    pub responses: [[String; 6]; 5],
}

pub struct FinalJeopardy {
    pub clue: String,
    pub response: String,
    pub category: String,
    pub wagers: HashMap<String, Option<i32>>,
    pub player_responses: HashMap<String, Option<String>>,
}

pub struct Game {
    pub state: State,
    pub single_jeopardy: BoardData,
    pub double_jeopardy: BoardData,
    pub final_jeopardy: FinalJeopardy,
    pub host_tx: Option<mpsc::UnboundedSender<Message>>,
    pub board_tx: Option<mpsc::UnboundedSender<Message>>,
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
            None => return,
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
}

#[derive(Serialize)]
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
    pub round: Round,
    pub clues_shown: u32,
}

#[derive(Serialize)]
pub enum Round {
    Single,
    Double,
    Final,
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
            buzzers_open: false,
            buzzed_player: None,
            active_player: None,
            cost: 0,
            category: "Welcome to Jeopardy!".to_string(),
            clue: "Please wait for the game to start.".to_string(),
            response: "I'm sure that'll be soon".to_string(),
            players: HashMap::new(),
            responded_players: HashSet::new(),
            round: Round::Single,
            clues_shown: 0,
        }
    }
}
