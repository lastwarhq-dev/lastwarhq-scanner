//! What we know about the game: the account being scanned, what each panel lists, and the VS
//! week everything belongs to. `view` keeps game messages and mail results. `tasks` keeps the
//! secret tasks seen on the world map, and `trucks` the trucks driving across it.

pub mod account;
pub mod battle;
pub mod panel;
pub mod tasks;
pub mod trucks;
pub mod view;
pub mod week;
