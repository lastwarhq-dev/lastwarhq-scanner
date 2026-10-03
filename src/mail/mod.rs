//! The game's local mail, read only when the user presses Load mail: `database` reads Desert
//! Storm results from `config.db` through the read-only `sqlite` reader.

pub mod database;
pub mod sqlite;
