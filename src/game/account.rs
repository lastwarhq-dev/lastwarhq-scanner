//! The game account being scanned: recognised only from messages that always describe the
//! logged-in player, never from login messages.

use std::time::Duration;

use crate::protocol::sfs::Value;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Account {
    pub uid: String,
    pub name: Option<String>,
    pub alliance_id: Option<String>,
    /// The alliance's name, and when it was last seen (on the game server's clock).
    pub alliance_name: Option<(String, Duration)>,
    /// The alliance's tag, and when it was last seen.
    pub alliance_abbr: Option<(String, Duration)>,
}

impl Account {
    pub fn alliance_name(&self) -> Option<&str> {
        self.alliance_name.as_ref().map(|(n, _)| n.as_str())
    }

    pub fn alliance_abbr(&self) -> Option<&str> {
        self.alliance_abbr.as_ref().map(|(a, _)| a.as_str())
    }
}

/// The logged-in player's uid, from messages that only ever describe that player.
pub(crate) fn own_uid(command: &str, data: &Value) -> Option<String> {
    let uid = match command {
        "gold.tree.act.view" => data
            .get("userGoldTreeDataInfo")?
            .get("userGoldTeeInfo")?
            .get("uid"),
        "hero.event.info.get" => data
            .get("eventList")?
            .as_array()?
            .iter()
            .find_map(|e| e.get("userScore")?.get("uid")),
        "push.mail" => data.get("toUser"),
        "push.chat.get.system.mails" => data.get("msg")?.as_array()?.first()?.get("toUser"),
        _ => None,
    };
    uid?.as_str().filter(|u| !u.is_empty()).map(str::to_string)
}

fn text(data: &Value, key: &str) -> Option<String> {
    data.get(key)?.as_str().map(str::to_string)
}

impl Account {
    /// Fills in name and alliance from messages about this account. `ours` is our alliance as
    /// known so far, from the account or the member list. Returns whether anything changed.
    pub(crate) fn absorb(
        &mut self,
        command: &str,
        data: &Value,
        time: Duration,
        ours: Option<&str>,
    ) -> bool {
        let before = self.clone();
        match command {
            // A player profile: ours only when the uid matches.
            "get.new.user.info" if text(data, "uid").as_deref() == Some(self.uid.as_str()) => {
                self.name = text(data, "name").or(self.name.take());
                self.note_alliance(
                    text(data, "allianceId"),
                    text(data, "allianceName"),
                    text(data, "abbr"),
                    time,
                );
            }
            // Desert Storm panel: lists both alliances, and `side` 0 isn't always ours. It only
            // gives the name and tag of the alliance already known, found by its id.
            "dragon.activity.info" => {
                let entry = ["group1", "group2"]
                    .iter()
                    .filter_map(|g| data.get(g)?.get("vsInfoArr")?.as_array())
                    .flatten()
                    .find(|e| ours.is_some() && text(e, "allianceId").as_deref() == ours);
                if let (Some(e), Some(id)) = (entry, ours) {
                    self.note_alliance(
                        Some(id.to_string()),
                        text(e, "name"),
                        text(e, "abbr"),
                        time,
                    );
                }
            }
            _ => {}
        }
        *self != before
    }

    /// A different alliance id replaces the alliance's name and abbreviation too, so a new
    /// alliance never shows the old one's name. Otherwise only the values sent are updated.
    /// Each value keeps the time it was itself last seen.
    fn note_alliance(
        &mut self,
        id: Option<String>,
        name: Option<String>,
        abbr: Option<String>,
        time: Duration,
    ) {
        let name = name.map(|n| (n, time));
        let abbr = abbr.map(|a| (a, time));
        if id.is_some() && id != self.alliance_id {
            self.alliance_id = id;
            self.alliance_name = name;
            self.alliance_abbr = abbr;
        } else {
            self.alliance_name = name.or(self.alliance_name.take());
            self.alliance_abbr = abbr.or(self.alliance_abbr.take());
        }
    }
}
