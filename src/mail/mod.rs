//! The game's local mail, read at start-up and every 5 minutes: `database` reads Desert
//! Storm results from `config.db` through the read-only `sqlite` reader.

pub mod database;
pub mod sqlite;
