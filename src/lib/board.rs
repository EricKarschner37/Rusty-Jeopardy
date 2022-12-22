use std::sync::Arc;

use futures_util::{SinkExt, StreamExt, TryFutureExt};
use serde::Deserialize;
use tokio::sync::{
    mpsc::{self, UnboundedSender},
    RwLock,
};
use warp::ws::{Message, WebSocket};

use super::{
    game::{BaseMessage, PlayerMessage, Round, StateType},
    Game,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

#[derive(Deserialize)]
struct PlayerBalanceMessage {
    request: String,
    player: String,
    amount: i32,
}

#[derive(Deserialize)]
struct RevealMessage {
    request: String,
    row: usize,
    col: usize,
}

impl Game {
    fn board_connected(&mut self, tx: UnboundedSender<Message>) -> Result<(), ()> {
        if self.board_tx.is_some() {
            Err(())
        } else {
            self.board_tx = Some(tx);
            Ok(())
        }
    }
    fn board_disconnected(&mut self) {
        self.board_tx = None;
        self.send_state();
    }

    fn start_double(&mut self) {
        self.state.state_type = StateType::Board;
        self.state.round = Round::Double;
        self.state.clues_shown = 0;
        self.send_categories();
        self.send_state();
    }

    fn start_final(&mut self) {
        self.state.state_type = StateType::FinalWager;
        self.state.round = Round::Final;
        self.state.clues_shown = 0;
        self.send_state();
    }

    fn remove_player(&mut self, player: String) {
        if let Some(Some(tx)) = self.state.players.remove(&player).map(|p| p.tx) {
            tx.send(Message::close());
        }
        self.final_jeopardy.wagers.remove(&player);
        self.final_jeopardy.player_responses.remove(&player);
    }

    fn set_player_balance(&mut self, player: String, amount: i32) {
        self.state
            .players
            .entry(player)
            .and_modify(|p| p.balance = amount);
    }

    fn reveal(&mut self, row: usize, col: usize) {
        let board = match self.state.round {
            Round::Single => &self.single_jeopardy,
            Round::Double => &self.double_jeopardy,
            Round::Final => {
                return;
            }
        };
        if row > 5 || col > 6 {
            return;
        }

        let bitset_key = 1 << (row * 6 + col);
        let cost_multiplier = match self.state.round {
            Round::Single => 200,
            _ => 400,
        };

        self.state.clue = board.clues[row][col].clone();
        self.state.response = board.responses[row][col].clone();
        self.state.category = board.categories[col].clone();
        self.state.cost = (row as i32 + 1) * cost_multiplier;
        self.state.state_type = if self.state.clue.starts_with("Daily Double:") {
            StateType::DailyDouble
        } else {
            StateType::Clue
        };

        self.state.clues_shown |= bitset_key;
    }
}

pub async fn board_connected(
    games: Arc<RwLock<Vec<Arc<RwLock<Game>>>>>,
    game_idx: usize,
    ws: WebSocket,
) {
    let game = match games.read().await.get(game_idx) {
        Some(g) => g.clone(),
        None => {
            ws.close().await;
            return;
        }
    };
    let (mut ws_tx, mut ws_rx) = ws.split();
    let (tx, rx) = mpsc::unbounded_channel();
    let mut rx = UnboundedReceiverStream::new(rx);

    if game.write().await.board_connected(tx).is_err() {
        // There is already a host connected
        ws_tx.send(Message::close());
        return;
    }

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

    game.read().await.send_categories();

    while let Some(message) = ws_rx.next().await {
        let msg = match message {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Websocket error: {}", e);
                return;
            }
        };

        let txt = match msg.to_str() {
            Ok(s) => s,
            Err(_) => {
                eprintln!("Received non-text Websocket message");
                return;
            }
        };

        let msg: BaseMessage = match serde_json::from_str(txt) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("Deserialization Error: {}", e);
                return;
            }
        };

        let mut game = game.write().await;
        match msg.request.as_str() {
            "start_double" => game.start_double(),
            "start_final" => game.start_final(),
            "response" => game.show_response(),
            "board" => {
                game.state.state_type = StateType::Board;
                game.send_state();
            }
            "remove" => {
                let msg: PlayerMessage = match serde_json::from_str(txt) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("Deserialization Error: {}", e);
                        return;
                    }
                };

                game.remove_player(msg.player);
            }
            "set_player_balance" => {
                let msg: PlayerBalanceMessage = match serde_json::from_str(txt) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("Deserialization Error: {}", e);
                        return;
                    }
                };

                game.set_player_balance(msg.player, msg.amount);
            }
            "reveal" => {
                let msg: RevealMessage = match serde_json::from_str(txt) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("Deserialization Error: {}", e);
                        return;
                    }
                };

                game.state.state_type = StateType::Clue;
                game.reveal(msg.row, msg.col);
            }
            _ => {}
        };
        game.send_state();
    }

    game.write().await.board_disconnected();
}
