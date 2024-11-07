use crate::{lib::IdStore, Game};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

pub type AsyncGameList = Arc<RwLock<HashMap<String, Option<Arc<RwLock<Game>>>>>>;
pub type AsyncIdStore = Arc<RwLock<IdStore>>;
