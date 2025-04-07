use futures_util::{SinkExt, StreamExt, TryFutureExt};
use serde::Deserialize;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;
use warp::ws::{Message, WebSocket};

use super::{
    game::{BaseMessage, PlayerMessage, RevealMessage, RoundType, StateType},
    AsyncGameList, Game,
};

#[derive(Deserialize)]
pub struct CorrectMessage {
    request: String,
    pub correct: bool,
}

pub async fn host_connected(games: AsyncGameList, lobby_id: String, ws: WebSocket) {
    let game_lock = match games.read().await.get(&lobby_id) {
        Some(Some(g)) => g.clone(),
        _ => {
            ws.close().await;
            return;
        }
    };
    let (mut ws_tx, mut ws_rx) = ws.split();
    let (tx, rx) = mpsc::unbounded_channel();
    let mut rx = UnboundedReceiverStream::new(rx);

    if game_lock.write().await.host_connected(tx).is_err() {
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

    game_lock.write().await.send_state();

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

        let mut game = game_lock.write().await;
        match msg.request.as_str() {
            "open" => game.set_buzzers_open(true, game_lock.clone()),
            "close" => game.set_buzzers_open(false, game_lock.clone()),
            "correct" => {
                let msg: CorrectMessage = match serde_json::from_str(txt) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("Deserialization Error: {}", e);
                        break;
                    }
                };

                game.correct(msg.correct);
            }
            "player" => {
                let msg: PlayerMessage = match serde_json::from_str(txt) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("Deserialization Error: {}", e);
                        break;
                    }
                };

                game.player(msg.player);
            }
            "reveal" => {
                let msg: RevealMessage = match serde_json::from_str(txt) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("Deserialization Error: {}", e);
                        continue;
                    }
                };

                game.reveal(msg.row, msg.col, game_lock.clone());
            }
            _ => {}
        };
        game.send_state();
    }

    game_lock.write().await.host_disconnected();
}

impl Game {
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

    fn player(&mut self, player: String) {
        self.state.active_player = Some(player);
        self.send_state();
    }
}
