use std::{cmp, sync::Arc};

use futures_util::{SinkExt, StreamExt, TryFutureExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, RwLock};
use warp::ws::{Message, WebSocket};

use super::game::{BaseMessage, Game, Round, StateType};
use tokio_stream::wrappers::UnboundedReceiverStream;

#[derive(Serialize)]
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

impl Game {
    fn register_player(&mut self, name: &str, tx: mpsc::UnboundedSender<Message>) {
        let name = name.to_owned();
        if self.state.players.contains_key(&name) {
            self.state
                .players
                .entry(name)
                .and_modify(move |p| p.tx = Some(tx));
        } else {
            self.final_jeopardy
                .player_responses
                .insert(name.clone(), None);
            self.final_jeopardy.wagers.insert(name.clone(), None);
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
        self.final_jeopardy
            .player_responses
            .insert(name, Some(response));

        if self
            .final_jeopardy
            .player_responses
            .values()
            .all(Option::is_some)
        {
            self.evaluate_final_responses();
        }
    }

    fn is_wager_valid(&self, player: &str, amount: i32) -> bool {
        let buzzed_player_balance = self.state.players[player].balance;
        let max_wager = match self.state.round {
            Round::Single => cmp::max(buzzed_player_balance, 1000),
            Round::Double => cmp::max(buzzed_player_balance, 2000),
            Round::Final => cmp::max(buzzed_player_balance, 3000),
        };
        amount >= 5 && amount <= max_wager as i32
    }

    fn wager(&mut self, player: String, wager: i32) {
        if !self.is_wager_valid(&player, wager) {
            return;
        }
        if self.state.state_type == StateType::DailyDouble {
            self.state.cost = wager;
            self.state.state_type = StateType::Clue;
            self.state.active_player = None;
            self.state.buzzers_open = true;
            self.buzz(&player);
            self.send_state();
            return;
        }
        self.final_jeopardy.wagers.insert(player, Some(wager));
        if self.final_jeopardy.wagers.values().all(|w| w.is_some()) {
            self.state.state_type = StateType::FinalClue;
            self.state.clue = self.final_jeopardy.clue.clone();
            self.state.response = self.final_jeopardy.response.clone();
            self.send_state();
        }
    }
}

pub async fn player_connected(
    games: Arc<RwLock<Vec<Option<Arc<RwLock<Game>>>>>>,
    game_idx: usize,
    ws: WebSocket,
) {
    let game = match games.read().await.get(game_idx) {
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
            Err(e) => {
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

        {
            let mut game = game.write().await;
            game.register_player(&m.name, tx);
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
                        game.write().await.player_disconnected(m.name.clone());
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

            match msg.request.as_str() {
                "buzz" => game.write().await.buzz(&m.name),
                "response" => {
                    let msg: ResponseMessage = match serde_json::from_str(txt) {
                        Ok(m) => m,
                        Err(e) => {
                            eprintln!("Deserialization Error: {}", e);
                            break;
                        }
                    };
                    game.write().await.response(m.name.clone(), msg.response);
                }
                "wager" => {
                    let msg: WagerMessage = match serde_json::from_str(txt) {
                        Ok(m) => m,
                        Err(e) => {
                            eprintln!("Deserialization Error: {}", e);
                            break;
                        }
                    };
                    game.write().await.wager(m.name.clone(), msg.amount);
                }
                _ => {}
            }
        }

        game.write().await.player_disconnected(m.name);
    }
}
