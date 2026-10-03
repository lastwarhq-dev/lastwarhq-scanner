//! A Desert Storm battle result, as read from the game's result mail.

use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct DsBattle {
    /// Mail `createTime`, a few seconds after the battle ends.
    pub time: Duration,
    pub won: bool,
    /// The alliance the result belongs to (ours, if this battle is relevant).
    pub alliance_id: String,
    pub score: i64,
    pub enemy_abbr: String,
    pub enemy_name: String,
    pub enemy_score: i64,
    /// Every player who fought for the alliance: uid and personal score.
    pub players: Vec<(String, i64)>,
    /// Every player entry in the mail could be read. When false, a player missing from
    /// `players` may have fought, so nobody can be judged absent from this battle.
    pub players_complete: bool,
}
