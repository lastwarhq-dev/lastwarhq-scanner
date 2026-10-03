//! Read-only companion for the Last War: Survival PC client.
//!
//! Data flows one way: `capture` turns network packets into the game connection's byte
//! stream, `protocol` turns that into SmartFox messages, `game` merges messages into what we
//! know about the alliance's players, and `ui` shows it. `mail` reads Desert Storm results from
//! the game's local mail database on request. `app` ties these together.

pub mod app;
pub mod capture;
pub mod game;
pub mod mail;
pub mod protocol;
pub mod ui;
pub mod util;
