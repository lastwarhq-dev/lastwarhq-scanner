# LastWarHQ Scanner

A small Windows tool that collects your alliance's data from the **Last War: Survival** PC client
while you play: the member roster, Desert Storm sign-ups and results, and VS duel scores. It
gathers everything in one window and copies it out as JSON for use in other tools.

It works by **reading the game's own network traffic**, passively. It never sends anything to
the game, never touches the game process, and saves nothing to disk.

> **Unofficial.** Not affiliated with or endorsed by the makers of Last War: Survival. Game
> terms of service may prohibit third-party tools, even passive ones. Use it at your own risk.

---

## Using it

### What you need

| Requirement | Notes |
|---|---|
| Windows 10 or 11, 64-bit | |
| **[Npcap](https://npcap.com/#download)** | The Windows packet capture driver. Install it with the default options. It has to be installed separately: its licence doesn't allow other programs to bundle it. If you already have Wireshark with Npcap, you're set. |
| Last War: Survival PC client | The standard PC version. |

That's all. The exe is self-contained: no Wireshark, no Visual C++ runtime, nothing else to
install.

### Download and run

1. Download `lastwarhq-scanner.exe` from the [latest release](../../releases/latest). To check
   the download, compare `Get-FileHash lastwarhq-scanner.exe` (PowerShell) with the SHA-256
   in the release notes.
2. Start the game and log in.
3. Run `lastwarhq-scanner.exe`.
   - The exe isn't code-signed, so Windows SmartScreen may say "Windows protected your PC".
     Choose **More info → Run anyway**.
   - If Npcap was installed with "Restrict Npcap driver's access to Administrators only",
     Windows asks for admin approval **once each time the tool starts**. Approve it, or capture
     can't start.
4. The status line turns green with **Connected · server · heartbeat**.
5. In the game, open the panels whose data you want. The tool only sees what the game loads:

| To get… | Open in the game |
|---|---|
| Ranks, power, kills | the **alliance member list** |
| Hero power, Desert Storm time slots and teams | the **Desert Storm participants** panel |
| VS scores | the **VS duel** panel, and each **day tab** you want |
| Desert Storm results | press **Load mail** in the tool (see below) |

6. Press **Copy JSON** and paste the result wherever you need it.

Only one copy of the tool runs at a time.

### The window

| Line | Shows |
|---|---|
| Status | Capture state. Green: connected to the game server, with the time since its last heartbeat (every 4 s). Amber: searching for the game, or no heartbeat. Red: capture failed, with the reason. |
| Account | The logged-in player and alliance. Identified at login, when the VS panel opens, or when mail arrives. |
| Week | The current VS week (Monday–Sunday, UTC) and today. |
| Roster | Players in the alliance member list, and when it was last loaded. |
| DS sign-ups | How many players picked Desert Storm time slots, and how many are assigned to a team. |
| DS results | This week's Desert Storm battles (Team A/B, won/lost) after **Load mail**. |
| VS scores | Which VS days (Mon–Sat) have been loaded. `…` marks today, which is still in progress. |
| Activity | Messages decoded from the game, and when the last arrived. |

All times are UTC. Until their data arrives, the data lines say which panel to open (or, for
DS results, to press Load mail).

### Load mail (Desert Storm results)

Desert Storm results aren't sent with the panels; the game keeps them in its local mail
database (`%USERPROFILE%\AppData\LocalLow\FunFly\Last War-Survival Game\config.db`). **Load mail**
reads that file, only when pressed:

- It's **read-only**: the file is opened without locking it, copied into memory, and closed. The
  tool never writes to it, and doesn't use a SQLite library, which could change the file while
  opening it.
- If the game is in the middle of saving, the tool waits and retries rather than read a
  half-written file.
- It keeps only **this week's** Desert Storm result mails. The file holds the mail of every
  account played on the PC, so a battle counts only if it was fought by your alliance. The
  alliance comes from your profile or the alliance member list; until one of them has arrived,
  no battle is counted and the line says to open the member list.
- The team for each battle comes from the participants panel, so open that first.
- If the account, alliance or week changes while the file is being read, the result is
  dropped; press Load mail again.

### Copy JSON

```json
{
  "schemaVersion": 1,
  "week": "2026-09-28",
  "generated": "2026-10-04T12:00:00Z",
  "account": { "uid": "…", "name": "…", "allianceId": "…", "allianceName": "…", "allianceAbbr": "…" },
  "updated": { "roster": "…Z", "dsSignups": "…Z", "dsResults": "…Z", "vs": "…Z" },
  "players": [
    {
      "uid": "…", "name": "…", "rank": 4,
      "stats": { "power": 250000000, "heroPower": 140000000, "armyKill": 2000000 },
      "desertStorm": {
        "chooseTimeList": [2, 1],
        "result": { "team": 1, "attended": true, "score": 5000000, "won": false }
      },
      "vsScores": [90000000, 40000000, null, null, null, null]
    }
  ]
}
```

| Field | Meaning |
|---|---|
| `schemaVersion` | Version of this layout; raised whenever fields change meaning or shape. |
| `week` | Monday (UTC date) of the VS week the data belongs to. |
| `account.allianceId` | The alliance the data belongs to: from the account's profile, or else from the member list. |
| `updated` | When each part was last updated (UTC), or `null` if not loaded. `dsSignups`, `dsResults` and `vs` reset to `null` at the weekly reset; `roster` keeps its time, as the roster stays. `dsResults` stays `null` until the alliance is known. |
| `rank` | Alliance rank 1–5 (R5 highest). |
| `stats.heroPower` | "Total Hero Power" from the Desert Storm participants panel: hero strength without troops. |
| `desertStorm.chooseTimeList` | Time slots the player picked, in the order clicked: `1` = 11:00 UTC, `2` = 20:00 UTC, `3` = 01:00 UTC. |
| `desertStorm.result` | `team` 1 = Team A, 2 = Team B. `attended` means scored above 0. A player assigned to a team but missing from its battle has `attended: false` and `score: null`. `null` means no part this week, or that a player entry in a result mail couldn't be read, so absence can't be told. |
| `vsScores` | Personal VS score per day, Monday–Saturday. Only completed days; `null` until loaded. |

Unknown values are `null`.

### Weekly reset

VS days end at **02:00 UTC** (midnight server time), and the week rolls over on Monday at
02:00 UTC. At the rollover, the tool clears that week's data: VS scores, Desert Storm results,
sign-ups and team assignments. The roster stays. This happens even if the game is closed. Copy
JSON checks for the rollover before copying, game messages from the week that has ended are
ignored, and **Load mail** in a new week ignores last week's battles, so a copy never mixes two
weeks.

Switching to another game account also clears everything and starts fresh. If the account
moves to another alliance, the old alliance's players, scores, sign-ups and results are
cleared. Once the account's alliance is known, another alliance's member list is ignored.

### Troubleshooting

| Message | What to do |
|---|---|
| "Npcap is not installed" | Install Npcap from https://npcap.com and start the tool again. |
| Stays on "Starting capture" | Approve the Windows admin prompt; it may be behind other windows. |
| "Searching for the game" | Start the game and log in. |
| "No heartbeat" | The game was closed or lost connection. The tool picks up the new connection by itself. |
| Account says "waiting" | Open the VS panel, or log in again. |
| "LastWarHQ Scanner is already running" | Another copy is open; use that one. |
| Mail: "the game is saving its mail" | Press Load mail again in a moment. |
| DS results: "battles loaded · open the alliance member list" | Open the member list so the tool knows which battles are your alliance's. |
| Mail: "WAL mode, which is not supported" | The game stored its mail in a format the tool doesn't read. Please report it. |

### What it does and doesn't do

- **Receives only.** It captures a copy of the traffic on the game's ports. Only the server's
  messages are decoded; the game's own requests are encrypted and only counted. It sends
  nothing to the game, opens no network connections of its own, and listens on no ports.
- **Hands off the game.** It never hooks, reads the memory of, or modifies the game. The only
  game file it reads is the mail database, and only when you press Load mail.
- **Memory only.** Data lives in memory while the tool runs and is gone when it closes. Nothing
  is written to disk.
- **No login data kept.** The game's login messages (`login.ext`, `login.other`, `init`) are
  decoded like every other message as they pass through, but nothing in them is used, kept,
  shown or copied.
- **Recovers by itself.** If the capture misses a packet, the tool gives up on the missing
  bytes after 3 seconds and carries on with what follows.

---

## Building from source

```sh
git clone https://github.com/lastwarhq-dev/lastwarhq-scanner
cd lastwarhq-scanner
cargo build --release
```

The exe is `target\release\lastwarhq-scanner.exe`.

- **Toolchain:** `rust-toolchain.toml` pins a dated nightly (minimal profile, plus `rustfmt` and
  `clippy`), so every build uses the same compiler. The project stays on nightly, for local
  builds and the release workflow alike, until the dependency age filter below is available in
  stable Rust.
- **Dependency age filter:** `.cargo/config.toml` sets `global-min-publish-age = "7 days"`, so
  `cargo add` and `cargo update` only pick crate versions that have been public for a week.
- **Static C runtime:** `.cargo/config.toml` links the C runtime into the exe
  (`+crt-static`), so it doesn't need the Visual C++ Redistributable.
- **No Npcap SDK needed:** Npcap's `wpcap.dll` is loaded at run time.

The exe depends on two crates directly: `etherparse` (packet headers) and `ruzstd` (zstd
decompression), which bring in `arrayvec` and `twox-hash`. The window (Win32), the SQLite reader, JSON and SmartFox decoding are written in
the project, with bounds checks so damaged input gives an error rather than a crash. Damaged
input also can't make them use much memory:

- The SQLite reader uses each page and cell once, decodes no more payload than the file
  holds, and reads rows one at a time.
- A SmartFox message may take at most 16 times its size once decoded (64 MiB at most).

### Tests and checks

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

- Unit tests cover each stage with synthetic data, including malformed input: damaged database
  pages, oversized compressed frames, packets missed by the capture, messages from an ended
  week, and partly readable lists. They also cover account, alliance and week changes,
  including a mail load that crosses the weekly reset, and the connection and memory limits.
- No test uses real game data: no captures, no game files. None of them need Npcap or the game
  installed.

### Releases

`.github/workflows/release.yml` runs on every push to `main`, including merged pull requests:

1. On Windows: formatting, Clippy and the tests, then `cargo build --release --locked`.
2. The exe and its SHA-256 are kept as a workflow artifact.
3. A new release, `v<version>-build.<run number>`, is published with both files and marked
   as the latest.

The build job only has read access to the repository; the separate publish job is the only one
that can create releases. The workflow uses only GitHub's own actions (`checkout`,
`upload-artifact`, `download-artifact`), pinned to exact commits.

### Code layout

| Folder | Job |
|---|---|
| `src/app` | Start-up, capture loop, Load mail; shared state; the JSON export |
| `src/capture` | Adapter list, Npcap capture, TCP reassembly, locking on to the game connection |
| `src/protocol` | SmartFox framing, decompression and SFSObject decoding |
| `src/game` | The player view, account, player records, Desert Storm battles and VS week rules |
| `src/mail` | The read-only SQLite reader and Desert Storm result mails |
| `src/ui` | The Win32 window and its status text |
| `src/util` | JSON and UTC time helpers |
| `.github/workflows` | The release build |

---

## How it works

Everything below was worked out from the PC client's own traffic and files.

### Capture

- The tool picks network adapters from Windows' adapter list: connected, not loopback, with a
  real IPv4 address. Npcap names each adapter `\Device\NPF_{<adapter GUID>}`. Listing adapters
  this way doesn't open the capture driver, so it needs no admin prompt.
- Each adapter is opened through Npcap with the filter `tcp portrange 10000-19999`, and read on
  its own thread. Every 10 s the tool opens newly connected adapters (for example after
  switching from Ethernet to WiFi).
- The game keeps **one TCP connection** to its game server for the whole session. Its port is
  **10000 + the server number** the base is on, so it changes when the base moves server. Every
  connection on the port range is decoded, but only one is used: the first to produce a
  SmartFox message is locked on as the game connection, and messages from the others are
  discarded.
- The server answers the client's ping every 4 s (see below). That's the heartbeat: the lock is
  released when the connection closes or the heartbeat stops for 12 s, and the next game
  connection takes over.
- TCP segments are put back in order. If one is missing, later data waits up to 3 s for it (a
  lost segment is resent well within that); after that the missing bytes are given up and the
  framer finds the next message boundary.
- Memory is bounded. Connections quiet for 2 minutes are forgotten. At most 64 connections are
  tracked, and together they may hold 64 MiB of data waiting for a missing segment or the
  rest of a frame; past either limit, other connections are dropped, never the game
  connection. The packet queue between the adapters and the decoder holds 1,024 packets.

### Framing (server → client)

The protocol is **SmartFox 2X**. Messages are packed back to back in the TCP stream: one
segment can hold several messages, and one message can span several segments.

| Header byte | Layout | Use |
|---|---|---|
| `0x80` | `80` · u16 BE length · SFSObject | Most messages |
| `0xb0` | `b0` · u16 BE length · u32 BE uncompressed size · zstd frame | Larger messages |

Flag bits: `0x80` binary, `0x40` encrypted, `0x20` compressed, `0x08` 4-byte length, and
`0x10` appears with the uncompressed-size field. Messages from the client start `0xc4`
(binary + encrypted) and are not parsed. Server messages are never encrypted.

When capture starts partway through a connection, the framer looks for two consecutive
plausible frames before trusting a message boundary.

### Messages

An SFSObject is: type byte `0x12`, u16 entry count, then entries of u16 key length · UTF-8 key ·
type byte · value. Types seen: `0x01` bool, `0x02` byte, `0x03` short, `0x04` int, `0x05` long,
`0x08` string, `0x11` array, `0x12` object.

Every message has the envelope:

```
{ c: 1, a: 13, p: { c: "<command>", p: { ...data... } } }
```

- **Replies:** the client sends an encrypted request; the server answers with a message named
  after the command. Replies carry no request id.
- **Pushes:** commands starting `push.` arrive unprompted.
- **Ping:** every 4 s the client pings, and the server replies with `c: 0`, `a: 29`,
  `p: { serverTime, clientTime }` and no command.

Timestamps are Unix epoch (milliseconds or seconds, per field). Server time is UTC−2.

### Alliance member list: `al.rank`

Sent when the game fetches the member list. Not every open fetches it: in testing, reopening
the list shortly after sent nothing.

| `p.p` key | Contents |
|---|---|
| `allianceId` | 32 hex characters |
| `list` | every member |
| `allianceOfficialArr` | `{ uid, type }` for the officers |
| `groupDescription` | the alliance's group names |
| `r4MaxNum` | maximum number of R4 members |

Member fields include `uid`, `name`, `rank` (1–5), `power`, `armyKill`, `online`,
`mainCityLv`, `todayProgress`, `weeklyProgress`, `joinTime`, `donateTime`, `careerType`,
`careerLv`, `serverId` and `pointId`. There is one `power` value per member, with no breakdown.

### Desert Storm participants: `dragon.assign.player.info`

Sent every time the participants panel opens. `p.p.users` lists the whole alliance:

| Key | Contents |
|---|---|
| `uid`, `name` | |
| `heroPower` | "Total Hero Power" as shown in the panel |
| `armyPower`, `power` | `power` is more than `heroPower + armyPower` |
| `chooseTimeList` | time slots picked, in the order clicked (1 = 11:00, 2 = 20:00, 3 = 01:00 UTC) |
| `apply` | 1 = signed up |
| `group` | assigned team: 1 = Team A, 2 = Team B, 0 = none |
| `state` | within a team: 20 players have 1, 10 have 2 |
| `lv`, `commander`, `serverId`, `monthCardEndTime`, `pic`, `picVer` | |

There is no rank field, and the list isn't in rank order.

**`dragon.activity.info`** arrives with it. `group1` and `group2` each hold the team's
`timeInfo` `{prepTime, battleOpenTime, endTime}`, `battleServerId`, `signUp`, `assigned` and
`vsInfoArr`. `vsInfoArr` lists both alliances (`allianceId`, `name`, `abbr`, `strength`,
`power`, `armyPower`); our alliance is the entry with `side: 0`.

**`dragon.battle.history`** is the history panel: past battles with `battleTime` (the battle's
end), `groupM` (team), `score`, `enemyScore`, `state` (2 = won, 3 = lost), `userNum`, and the
opponent's name and server. It has player counts but no per-player data.

### Desert Storm results: local mail

Per-player Desert Storm results are only in the result mails. Opening a result mail sends
nothing over the game connection: the game already holds the mail locally.

- **`config.db`**, in the game's data folder, is a SQLite 3 database. Its table `MailData` holds
  the mail of every account that has played on the PC (column `toUser`). Columns: `uid`,
  `toUser`, `fromUser`, `fromName`, `title`, `subTitle`, `contents`, `rewardId`, `itemIdFlag`,
  `status`, `type`, `rewardStatus`, `saveFlag`, `createTime`, `reply`, `replyText`,
  `translationId`, `mbLevel`, `rewardTime`, `extParam1`, `extParam2`, `translateMsg`,
  `translatedLang`, `custom`, `expireTime`, `mailId`.
- A Desert Storm result mail has `type` 109 and `mailId` 23003. Its `contents` is JSON:

| `obj` key | Contents |
|---|---|
| `result` | 1 won, 0 lost |
| `alliances` | both alliances `{alId, abbr, name, icon, serverId, score}`, ours first |
| `ranks` | every player who fought: `{uid, name, score, rank, abbr, pic, picVer}` |
| `killScoreMvp`, `collectScoreMvp`, `occupyScoreMvp`, `brokeScoreMvp` | the MVPs |

- A mail's team is the panel `group` that most of its `ranks` players belong to. The mail's
  `createTime` is also a few seconds after that team's battle `endTime`.
- The database can hold other accounts' alliances' battles, so only battles of our alliance
  (`alliances[0].alId`) are used. Until our alliance is known (see [Account](#account)), no
  battle is used.
- If an entry in `ranks` can't be read, nobody is judged absent that week: the unreadable
  entry could be any player.
- Each account on the PC gets its own copy of a battle's mail, seconds apart. Copies are
  recognised by the same alliance, opponent and scores within 10 minutes.
- While the game runs it keeps `config.db` open. A reader that allows read/write/delete sharing
  can still read it; one asking for exclusive read access is refused.
- Battle reports (`type` 103 mails) are cached separately as
  `FileContents\MailContents\<uid>\<mail uid>.bin`: a varint length, then JSON
  `{"b":{},"obj":{"battleContent":"<base64 protobuf>"}}`.

### VS duel

- **`al.battle.rank.info`**: one message per day tab opened. `p.p.day` is 1 (Monday) to 6
  (Saturday). `rankInfo` lists every player of both alliances with `uid`, `name`, `score`,
  `aid` (alliance id), `abbr` and `alName`, sorted by score, including players with 0. Only each
  alliance's **current** members are listed: a player who has left disappears from every
  day, even days they scored on.
- **`hero.event.info.get`**: the VS event, sent when the VS panel opens. `eventList[0]` holds
  the day's start and end (`st`, `et`), `weekEndTime`, the capturing player's own `userScore`,
  and `vsAllianceInfo` for both alliances: `alName`, `abbr`, `power`, `win` (days won),
  `mvpPlayer`, `alScore` (today so far) and `scoreHistory` (`{day, score}` for each completed
  day). `scoreHistory` holds the official daily totals, which can be higher than the sum of
  that day's `rankInfo` scores.
- **`get.alliance.duel.season.info`** and **`get.alliance.duel.group.info`**: the alliance's
  league group and position, and the alliances in that group.

### Account

The logged-in player's uid is taken only from messages that always describe that player:

| Command | Field (`p.p`) | Arrives |
|---|---|---|
| `gold.tree.act.view` | `userGoldTreeDataInfo.userGoldTeeInfo.uid` | at login |
| `hero.event.info.get` | `eventList[].userScore.uid` | at login, VS panel |
| `push.chat.get.system.mails` | `msg[0].toUser` | at login, when there is mail |
| `push.mail` | `toUser` | when new mail arrives |

Name and alliance come from `get.new.user.info`, a player profile, only when its `uid` matches
(the game also sends it for other players' profiles), and from `dragon.activity.info`
(`side: 0`). A different uid from these messages means the user switched accounts; a different
alliance id for the same uid means the account moved alliance.

Until one of those names the account's alliance, the member list's `allianceId` stands in for
it, and a member list from another alliance replaces the roster. Once the account's alliance is
known, member lists from other alliances are ignored.

### Merging into the view

- One record per player, keyed by `uid`. Each message updates only the fields it carries, and
  the newest value wins.
- The member list and the participants panel each list the whole alliance, so a player missing
  from the newest one has left and is removed. An empty list changes nothing, and a list with
  any unreadable entry updates players but removes nobody.
- VS scores are kept by uid for the week, whatever order panels are opened in. They show only
  on players in the roster, so the opponent's players never appear. Today's scores are skipped
  until the day ends.

### Other findings

- Rally messages (`push.alliance.march.create`, `push.alliance.march.refresh`) carry per-march
  `power` and an `armyInfo` field that is base64-encoded protobuf.
- The game's `Player.log` is empty during play.

---

## Licence

[MIT](LICENSE)
