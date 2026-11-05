//! A Desert Storm or Canyon Storm battle result, as read from the game's result mail.

use std::time::Duration;

/// Which event a battle was fought in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    DesertStorm,
    CanyonStorm,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Battle {
    pub event: Event,
    /// Mail `createTime`, a few seconds after the battle ends.
    pub time: Duration,
    pub won: bool,
    /// The alliance the result belongs to (ours, if this battle is relevant).
    pub alliance_id: String,
    /// The alliance's score; in Canyon Storm, its side's.
    pub score: i64,
    /// The opponent; in Canyon Storm, every alliance on the other side, joined with `+`.
    pub enemy_abbr: String,
    pub enemy_name: String,
    pub enemy_score: i64,
    /// Every player who fought for the alliance: uid and personal score.
    pub players: Vec<(String, i64)>,
    /// Every player entry in the mail could be read. When false, a player missing from
    /// `players` may have fought, so nobody can be judged absent from this battle.
    pub players_complete: bool,
}
