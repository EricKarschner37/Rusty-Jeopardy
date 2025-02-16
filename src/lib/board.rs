use futures_util::{SinkExt, StreamExt, TryFutureExt};
use rand::seq::{IteratorRandom, SliceRandom};
use serde::Deserialize;
use tokio::sync::mpsc::{self, UnboundedSender};
use warp::ws::{Message, WebSocket};

use super::{
    game::{BaseMessage, PlayerMessage, RevealMessage, RoundType, StateType},
    AsyncGameList, Game,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

#[derive(Deserialize)]
struct PlayerBalanceMessage {
    request: String,
    player: String,
    amount: i32,
}

#[derive(Deserialize)]
struct RandomizeActivePlayerMessage {
    request: String,
}

impl Game {
    fn board_connected(&mut self, tx: UnboundedSender<Message>) -> Result<(), ()> {
        if self.board_tx.is_some() {
            println!("attempted to connect board, but there's already ony connected");
            Err(())
        } else {
            println!("connecting board");
            self.board_tx = Some(tx);
            Ok(())
        }
    }
    fn board_disconnected(&mut self) {
        println!("removing board socket");
        if let Some(tx) = &self.board_tx {
            tx.send(Message::close());
        }
        self.board_tx = None;
        self.send_state();
    }

    fn next_round(&mut self) {
        self.state.round_idx += 1;
        self.state.clues_shown = 0;
        let new_round = &self.rounds[self.state.round_idx];
        self.state.bare_round = new_round.clone().to_bare_round();
        if let RoundType::FinalRound { category, .. } = new_round {
            self.state.category = category.to_string();
            self.state.state_type = StateType::FinalWager;
        } else {
            self.state.state_type = StateType::Board;
        }
        self.send_state();
    }

    fn remove_player(&mut self, player: String) {
        if let Some(Some(tx)) = self.state.players.remove(&player).map(|p| p.tx) {
            tx.send(Message::close());
        }
        self.state.wagers.remove(&player);
        self.state.player_responses.remove(&player);
        self.send_state();
    }

    fn set_player_balance(&mut self, player: String, amount: i32) {
        self.state
            .players
            .entry(player)
            .and_modify(|p| p.balance = amount);
        self.send_state();
    }
}

pub async fn board_connected(games: AsyncGameList, lobby_id: String, ws: WebSocket) {
    let game = match games.read().await.get(&lobby_id) {
        Some(Some(g)) => g.clone(),
        _ => {
            ws.close().await;
            return;
        }
    };
    let (mut ws_tx, mut ws_rx) = ws.split();
    let (tx, rx) = mpsc::unbounded_channel();
    let mut rx = UnboundedReceiverStream::new(rx);

    if game.write().await.board_connected(tx).is_err() {
        // There is already a board connected
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

    {
        let game = game.read().await;
        game.send_categories();
        game.send_state();
    }

    while let Some(message) = ws_rx.next().await {
        let msg = match message {
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
                    println!("board client disconnected");
                    game.write().await.board_disconnected();
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

        let mut game = game.write().await;
        match msg.request.as_str() {
            "next_round" => game.next_round(),
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
                        continue;
                    }
                };

                game.remove_player(msg.player);
            }
            "set_player_balance" => {
                let msg: PlayerBalanceMessage = match serde_json::from_str(txt) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("Deserialization Error: {}", e);
                        continue;
                    }
                };

                game.set_player_balance(msg.player, msg.amount);
            }
            "reveal" => {
                let msg: RevealMessage = match serde_json::from_str(txt) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("Deserialization Error: {}", e);
                        continue;
                    }
                };

                game.state.state_type = StateType::Clue;
                game.reveal(msg.row, msg.col);
            }
            "randomize_active_player" => {
                let active_player = game.state.players.keys().choose(&mut rand::thread_rng());
                game.state.active_player = active_player.cloned();
            }
            _ => {}
        };
        game.send_state();
    }

    game.write().await.board_disconnected();
}
