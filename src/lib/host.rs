use std::sync::Arc;

use futures_util::{SinkExt, StreamExt, TryFutureExt};
use serde::Deserialize;
use tokio::sync::{
    mpsc::{self, UnboundedSender},
    RwLock,
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use warp::ws::{Message, WebSocket};

use super::{
    game::{BaseMessage, PlayerMessage, Round, StateType},
    Game,
};

#[derive(Deserialize)]
struct CorrectMessage {
    request: String,
    correct: bool,
}

pub async fn host_connected(
    games: Arc<RwLock<Vec<Option<Arc<RwLock<Game>>>>>>,
    game_idx: usize,
    ws: WebSocket,
) {
    let game = match games.read().await.get(game_idx) {
        Some(Some(g)) => g.clone(),
        _ => {
            ws.close().await;
            return;
        }
    };
    let (mut ws_tx, mut ws_rx) = ws.split();
    let (tx, rx) = mpsc::unbounded_channel();
    let mut rx = UnboundedReceiverStream::new(rx);

    if game.write().await.host_connected(tx).is_err() {
        // There is already a host connected
        ws_tx.send(Message::close()).await;
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

    game.write().await.send_state();

    while let Some(msg) = ws_rx.next().await {
        let msg = match msg {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Websocket error: {}", e);
                break;
            }
        };

        let txt = match msg.to_str() {
            Ok(s) => s,
            Err(_) => {
                if msg.is_close() {
                    break;
                }
                eprintln!("Received non-text Websocket message");
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
            "open" => game.write().await.set_buzzers_open(true),
            "close" => game.write().await.set_buzzers_open(false),
            "correct" => {
                let msg: CorrectMessage = match serde_json::from_str(txt) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("Deserialization Error: {}", e);
                        break;
                    }
                };

                game.write().await.correct(msg.correct);
            }
            "player" => {
                let msg: PlayerMessage = match serde_json::from_str(txt) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("Deserialization Error: {}", e);
                        break;
                    }
                };

                game.write().await.player(msg.player);
            }
            _ => {}
        }
    }

    game.write().await.host_disconnected();
}

impl Game {
    fn set_buzzers_open(&mut self, open: bool) {
        self.state.buzzers_open = open;
        self.send_state();
    }

    fn host_connected(&mut self, tx: UnboundedSender<Message>) -> Result<(), ()> {
        if self.host_tx.is_some() {
            Err(())
        } else {
            self.host_tx = Some(tx);
            self.send_state();
            Ok(())
        }
    }
    fn host_disconnected(&mut self) {
        if let Some(tx) = &self.host_tx {
            tx.send(Message::close());
        }
        self.host_tx = None;
    }

    fn correct(&mut self, correct: bool) {
        if let Some(player) = &self.state.buzzed_player {
            self.state.players.entry(player.clone()).and_modify(|p| {
                p.balance += if correct {
                    self.state.cost
                } else {
                    -self.state.cost
                };
            });

            if self.state.round == Round::Final {
                self.evaluate_final_responses();
                self.send_state();
                return;
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

    fn player(&mut self, player: String) {
        self.state.active_player = Some(player);
        self.send_state();
    }
}
