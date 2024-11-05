use crate::Game;
use std::sync::Arc;
use tokio::sync::RwLock;

pub type AsyncGameList = Arc<RwLock<Vec<Option<Arc<RwLock<Game>>>>>>;
