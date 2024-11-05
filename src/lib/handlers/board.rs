use crate::lib::{board_connected, AsyncGameList};
use warp::ws::WebSocket;

pub fn accept_board(game_idx: usize, ws: warp::ws::Ws, games: AsyncGameList) -> impl warp::Reply {
    ws.on_upgrade(move |ws: WebSocket| board_connected(games, game_idx, ws))
}
