use std::{cmp, default, sync::Arc};

use futures_util::{SinkExt, StreamExt, TryFutureExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, RwLock};
use warp::ws::{Message, WebSocket};

use super::{
    game::{BaseMessage, Game, GameMode, RevealMessage, Round, RoundType, StateType},
    host::CorrectMessage,
    AsyncGameList,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

#[derive(Serialize, Debug)]
pub struct Player {
    pub name: String,
    #[serde(skip_serializing)]
    pub tx: Option<mpsc::UnboundedSender<Message>>,
    pub balance: i32,
    #[serde(skip_serializing)]
    pub did_auth: bool,
}

#[derive(Deserialize)]
struct ConnectMessage {
    request: String,
    name: String,
    password: Option<String>,
}

#[derive(Deserialize)]
struct WagerMessage {
    request: String,
    amount: i32,
}

#[derive(Deserialize)]
struct ResponseMessage {
    request: String,
    response: String,
}

#[derive(Serialize)]
struct PlayerInputResponseMessage {
    message: String,
    valid: bool,
    reason: String,
}

impl Game {
    fn register_player(
        &mut self,
        name: &str,
        tx: mpsc::UnboundedSender<Message>,
    ) -> Result<(), ()> {
        let name = name.to_owned();
        if self.state.players.contains_key(&name) {
            if let Some(Some(_tx)) = self.state.players.get(&name).map(|p| &p.tx) {
                return Err(());
            }

            self.state
                .players
                .entry(name)
                .and_modify(move |p| p.tx = Some(tx));
        } else {
            self.state.player_responses.insert(name.clone(), None);
            self.state.wagers.insert(name.clone(), None);
            self.state.players.insert(
                name.clone(),
                Player {
                    name,
                    tx: Some(tx),
                    balance: 0,
                    did_auth: false,
                },
            );
        }

        Ok(())
    }

    fn player_disconnected(&mut self, name: String) {
        self.state.players.entry(name).and_modify(move |p| {
            if let Some(tx) = &p.tx {
                tx.send(Message::close());
            }
            p.tx = None;
        });
    }

    pub fn buzz(&mut self, name: &str) {
        if !self.state.buzzers_open || self.state.responded_players.contains(name) {
            return;
        }

        self.state.buzzers_open = false;
        self.state.buzzed_player = Some(name.to_string());
        self.state.responded_players.insert(name.to_string());
        self.send_state();
    }

    fn response(&mut self, name: String, response: String) {
        let msg = if response.is_empty() {
            PlayerInputResponseMessage {
                message: "input-response".to_string(),
                valid: false,
                reason: "Input cannot be empty".to_string(),
            }
        } else {
            PlayerInputResponseMessage {
                message: "input-response".to_string(),
                valid: true,
                reason: "Input received".to_string(),
            }
        };

        if let Some(Some(tx)) = self.state.players.get(&name).map(|p| &p.tx) {
            if let Ok(txt) = serde_json::to_string(&msg) {
                tx.send(Message::text(txt));
            }
        }

        if !msg.valid {
            return;
        }

        self.state.player_responses.insert(name, Some(response));

        if self.state.player_responses.values().all(Option::is_some) {
            self.evaluate_final_responses();
        }
    }

    fn get_max_wager(&self, player: &str) -> i32 {
        let buzzed_player_balance = self.state.players[player].balance;
        let default_max_wager = match self.rounds[self.state.round_idx] {
            RoundType::DefaultRound {
                default_max_wager, ..
            } => default_max_wager,
            RoundType::FinalRound {
                default_max_wager, ..
            } => default_max_wager,
        };
        cmp::max(buzzed_player_balance, default_max_wager)
    }

    fn wager(&mut self, player: String, wager: i32) {
        let max = self.get_max_wager(&player);
        let msg: PlayerInputResponseMessage = if wager > max {
            PlayerInputResponseMessage {
                message: "input-response".to_string(),
                valid: false,
                reason: format!("Wager too high (max wager: {})", max),
            }
        } else if wager < 5 {
            PlayerInputResponseMessage {
                message: "input-response".to_string(),
                valid: false,
                reason: "Wager too low (min wager: 5)".to_string(),
            }
        } else {
            PlayerInputResponseMessage {
                message: "input-response".to_string(),
                valid: true,
                reason: "Wager accepted".to_string(),
            }
        };

        if let Some(Some(tx)) = self.state.players.get(&player).map(|p| &p.tx) {
            if let Ok(txt) = serde_json::to_string(&msg) {
                tx.send(Message::text(txt));
            }
        }

        if !msg.valid {
            return;
        }

        if self.state.state_type == StateType::DailyDouble {
            self.state.cost = wager;
            self.state.state_type = StateType::Clue;
            self.state.buzzers_open = true;
            self.buzz(&player);

            for p in self.state.players.keys() {
                self.state.responded_players.insert(p.to_string());
            }

            self.send_state();
            return;
        }
        self.state.wagers.insert(player, Some(wager));
        let (clue, response) = match &self.rounds[self.state.round_idx] {
            RoundType::DefaultRound { .. } => return,
            RoundType::FinalRound { clue, response, .. } => (clue, response),
        };
        if self.state.wagers.values().all(|w| w.is_some()) {
            self.state.state_type = StateType::FinalClue;
            self.state.clue = clue.clone();
            self.state.response = response.clone();
            self.send_state();
        }
    }

    fn declare_has_responded(&mut self, player: &str) {
        if self.mode != GameMode::Hostless {
            return;
        }
        self.state.responded_players.insert(player.to_string());
    }

    fn player_report_correct(&mut self, player: &str, correct: bool) {
        println!(
            "{}, {}\n",
            !self.state.responded_players.contains(player),
            self.state
                .buzzed_player
                .as_ref()
                .is_some_and(|p| p == player),
        );
        if self.mode != GameMode::Hostless
            || !self.state.responded_players.contains(player)
            || self
                .state
                .buzzed_player
                .as_ref()
                .is_some_and(|p| p == player)
        {
            return;
        }

        self.correct(correct)
    }
}

pub async fn player_connected(games: AsyncGameList, lobby_id: String, ws: WebSocket) {
    let game_lock = match games.read().await.get(&lobby_id) {
        Some(Some(game)) => game.clone(),
        _ => {
            ws.close().await;
            return;
        }
    };
    let (mut ws_tx, mut ws_rx) = ws.split();
    let (tx, rx) = mpsc::unbounded_channel();
    let mut rx = UnboundedReceiverStream::new(rx);

    if let Some(result) = ws_rx.next().await {
        let msg = match result {
            Ok(msg) => msg,
            Err(e) => {
                eprintln!("websocket error: {}", e);
                return;
            }
        };

        let msg = match msg.to_str() {
            Ok(s) => s,
            Err(_) => {
                eprintln!("websocket error: non-string message received");
                return;
            }
        };

        let m: ConnectMessage = match serde_json::from_str(msg) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("serde error: {}", e);
                return;
            }
        };

        tokio::task::spawn(async move {
            while let Some(message) = rx.next().await {
                ws_tx
                    .send(message)
                    .unwrap_or_else(|e| {
                        eprintln!("websocket send error: {}", e);
                    })
                    .await;
            }
        });

        let player_name = m.name;
        {
            let mut game = game_lock.write().await;
            if let Err(_) = game.register_player(&player_name, tx) {
                return;
            }
            game.send_state();
        }

        while let Some(result) = ws_rx.next().await {
            let msg = match result {
                Ok(msg) => msg,
                Err(e) => {
                    eprintln!("websocket error: {}", e);
                    break;
                }
            };

            let txt = match msg.to_str() {
                Ok(s) => s,
                Err(_) => {
                    if msg.is_close() {
                        game_lock
                            .write()
                            .await
                            .player_disconnected(player_name.clone());
                    }
                    eprintln!("websocket error: non-string message received");
                    continue;
                }
            };

            let msg: BaseMessage = match serde_json::from_str(txt) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("Deserialization Error: {}", e);
                    break;
                }
            };

            let mut game = game_lock.write().await;
            match msg.request.as_str() {
                "buzz" => game.buzz(&player_name),
                "response" => {
                    let msg: ResponseMessage = match serde_json::from_str(txt) {
                        Ok(m) => m,
                        Err(e) => {
                            eprintln!("Deserialization Error: {}", e);
                            break;
                        }
                    };
                    game.response(player_name.clone(), msg.response);
                }
                "wager" => {
                    let msg: WagerMessage = match serde_json::from_str(txt) {
                        Ok(m) => m,
                        Err(e) => {
                            eprintln!("Deserialization Error: {}", e);
                            break;
                        }
                    };
                    game.wager(player_name.clone(), msg.amount);
                }
                "reveal" => {
                    let msg: RevealMessage = match serde_json::from_str(txt) {
                        Ok(m) => m,
                        Err(e) => {
                            eprintln!("Deserialization Error: {}", e);
                            continue;
                        }
                    };
                    if game.state.active_player.as_deref() != Some(&player_name) {
                        continue;
                    };

                    game.reveal(msg.row, msg.col, game_lock.clone());
                }
                "correct" => {
                    let msg: CorrectMessage = match serde_json::from_str(txt) {
                        Ok(m) => m,
                        Err(e) => {
                            eprintln!("Deserialization Error: {}", e);
                            break;
                        }
                    };

                    game.player_report_correct(&player_name, msg.correct);
                }
                "responded" => {
                    if game.mode != GameMode::Hostless {
                        continue;
                    }
                    game.declare_has_responded(&player_name);
                }
                _ => {}
            }
            game.send_state()
        }

        game_lock.write().await.player_disconnected(player_name);
    }
}
