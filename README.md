# LastWarHQ Scanner

A small Windows tool that collects your alliance's data from the **Last War: Survival** PC client
while you play: the member roster, Desert Storm sign-ups and results, and VS duel scores. One
window shows what has been collected and what is still missing.

It works by **reading the game's own network traffic**, passively. It never sends anything to
the game and never touches the game process.

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
   in the release notes. Put it in a folder you can write to (not Program Files), so it can
   update itself.
2. Start the game and log in.
3. Run `lastwarhq-scanner.exe`.
   - The exe isn't code-signed, so Windows SmartScreen may say "Windows protected your PC".
     Choose **More info → Run anyway**.
   - If Npcap was installed with "Restrict Npcap driver's access to Administrators only",
     Windows asks for admin approval **once each time the tool starts**. Approve it, or capture
     can't start.
4. Under the headline, the dot turns green with **Game connected**.
5. In the game, open the panels whose data you want. The tool only sees what the game loads:

| To get… | Open in the game |
|---|---|
| Ranks, power, kills | the **alliance member list** |
| Hero power, Desert Storm time slots and teams | the **Desert Storm participants** panel |
| VS scores | the **VS duel** panel, and each **day tab** you want |
| Desert Storm results | nothing: the tool reads them from the game's mail (see below) |

When everything is loaded, the headline says **All in sync**.

Only one copy of the tool runs at a time.

### The window

| Part | Shows |
|---|---|
| Headline | **All in sync** (green tick): the game is connected, every row has a green tick (or is closed), and every finished VS day is loaded. **Open the game panels** (amber): connected, but something is missing; the rows say what. **Waiting for the game** or **Starting capture** (grey): no game connection yet. **Not capturing** (red): capture failed. |
| Line under it | The game connection. Green: connected, with a heartbeat (every 4 s) in the last 12 s. Grey: searching for the game, or no heartbeat. Red: why capture failed. |
| LastWarHQ | Your LastWarHQ sign-in and sync: your username, and once synced, **synced HH:MM UTC** (or what the sync is waiting for, or why it failed); until signed in, **Click to sign in**. See [Signing in to LastWarHQ](#signing-in-to-lastwarhq) and [Syncing](#syncing). It doesn't count towards the headline. |
| Alliance | The account's alliance, as `[ABBR] Name`. Click the row to copy the alliance ID. The member list names the alliance only by id; the name comes with the identified account's profile or the Desert Storm participants panel, and until then the row shows the id. Until either has arrived, **Open the member list**. |
| Roster | **In sync** once the alliance member list has been loaded; until then, **Open the member list**. |
| DS sign-ups | **In sync** once the Desert Storm participants panel has been loaded; until then, **Open the DS participants**. From Saturday 02:00 UTC to the Monday reset, **Closed until Monday** (grey): see [Desert Storm sign-ups](#desert-storm-sign-ups). |
| DS results | **In sync** once the mail has been read. **Open the member list** while battles were found but the alliance isn't known yet. Red: why the mail couldn't be read; after an earlier good read, **Stale · mail: …**: the earlier results are kept, but may be out of date. |
| VS scores | One tile per day, Monday to Saturday. Green tick: loaded. Dots: today, still in progress. Amber: over, but not loaded; open that day's tab. Empty ring: later this week. |
| Footer | The version, or an update (see [Updates](#updates)). |

### Signing in to LastWarHQ

Click the **LastWarHQ** row to sign in. The tool opens LastWarHQ's connect page in your browser,
where you approve this PC (sign in to the site first if it asks). The browser then comes back
to the tool. Once the token has been swapped and saved, the tab says **Connected** and the row
shows your username; if anything fails, the tab says why the sign-in didn't finish.

- It's PKCE sign-in for desktop apps (RFC 8252, RFC 7636): the tool listens on `127.0.0.1`, on a
  port Windows picks, only until the browser comes back or 10 minutes pass. The browser brings a
  one-time code, which the tool swaps for a token over HTTPS with a secret only it holds, so the
  code is no use to anything else that sees it.
- The token is kept in **Windows Credential Manager**, as "LastWarHQ Scanner" under Windows
  Credentials, encrypted for your Windows account. Nothing else of the sign-in is saved.
- At start-up the tool checks the token with LastWarHQ. If the site no longer accepts it (the
  PC was disconnected on the site, or it went 90 days unused), the token is deleted and the
  row asks you to sign in again.
- If you manage alliances on LastWarHQ but not the one the game shows, the row says so: that
  alliance can't be synced.
- Click the row again to sign out: the token is deleted, and LastWarHQ is told to disconnect
  this PC. If Credential Manager won't delete the token, you count as signed out only once
  LastWarHQ confirms the disconnect; otherwise the row says sign-out failed.

### Syncing

Once you're signed in, the tool uploads the [data payload](#data-payload) to LastWarHQ
(`POST /scanner-api/v1/sync`) by itself:

- Only once the alliance is known, its **full member list** has been loaded (every entry
  readable) and the game server's clock is known (from its first ping reply). Until then the
  LastWarHQ row says what it's waiting for.
- Only when something has changed since the last upload, and at most once a minute, or less
  often if LastWarHQ asks. Waits count from when LastWarHQ answers, on a clock the PC's time
  setting can't move.
- LastWarHQ decides whether you manage the alliance. If not, the row says so, and the tool
  tries again every 5 minutes, in case the alliance is added on the site.
- If LastWarHQ refuses the data as invalid, the row says why, and the same data isn't sent
  again; the next change is. If LastWarHQ is busy with other uploads for the alliance (`503
  busy`), the same upload goes again after the wait it asks for. Other failures (no connection,
  a server error) are retried after a minute. If LastWarHQ no longer accepts the sign-in, the
  token is deleted and the row asks you to sign in again.
- After an upload, the row shows **synced HH:MM UTC**.

### Desert Storm sign-ups

Sign-ups and team assignments are for the week's battles, which end on Friday. From Saturday,
the participants panel still lists the alliance, but with empty time slots and no other sign-up
fields. So from Saturday 02:00 UTC (the end of Friday, server time) until the Monday 02:00 UTC
reset, the tool clears the week's sign-ups, ignores the panel, and leaves `dsSignups` out of the
payload. Sign-ups sent earlier in the week are the record for that week.

### Desert Storm results (mail)

Desert Storm results aren't sent with the panels; the game keeps them in its local mail
database (`%USERPROFILE%\AppData\LocalLow\FunFly\Last War-Survival Game\config.db`). The tool
reads that file when it starts and every 5 minutes after:

- It's **read-only**: the file is opened without locking it, copied into memory, and closed. The
  tool never writes to it, and doesn't use a SQLite library, which could change the file while
  opening it.
- If the game is in the middle of saving, the tool waits and retries rather than read a
  half-written file. A read that fails keeps what the last good read found.
- It keeps only **this week's** Desert Storm result mails. The file holds the mail of every
  account played on the PC, so a battle counts only if it was fought by your alliance. The
  alliance comes from your profile or the alliance member list; until one of them has arrived,
  no battle is counted and the row says to open the member list.
- Each battle is kept as its mail gives it: when it ended, won or lost, and each player's uid
  and score. The tool doesn't work out teams or who was absent; the receiver can, from the
  sign-ups.
- If the account, alliance or week changes while the file is being read, the result is
  dropped; the next read picks up the change.

### Updates

The tool checks GitHub for a new release when it starts and every hour after. When there is
one, the footer says **Update available** with an **Update now** button.

Pressing it asks first, because the tool restarts to finish: restarting clears the data
loaded so far, so the game panels need opening again, and Windows may ask for admin approval
again. Then the tool:

1. Downloads the new `lastwarhq-scanner.exe` and its SHA-256 file from the release, and
   checks that they match.
2. Renames the running exe to `lastwarhq-scanner.exe.old` (Windows won't overwrite a running
   exe, but lets it be renamed) and puts the new one in its place.
3. Starts the new version and closes. The new version waits for the old one to close,
   removes the `.old` file, and says **Updated to version …** in the footer.

If a step fails, the footer says why, the running exe stays as it was, and the button
becomes **Try again**.

### Data payload

The tool syncs this payload to LastWarHQ (see [Syncing](#syncing)). It's built in
`src/app/export.rs`.

- **Times** are on the game server's clock, since LastWarHQ orders uploads from several PCs by
  them: the tool reads the server's time from its ping replies (`serverTime`, every 4 s), and
  each time it records is moved by the difference known when it is recorded, so a later
  correction never changes it. The week, the weekly reset, today's VS day and the Desert
  Storm weekend follow the server's clock too. Battle times come from the result mails, which
  already carry the server's time. A ping reply more than a day off this PC's clock is taken
  as a misreading and ignored.
- **Before the first ping reply** only this PC's clock is known, and what is recorded meanwhile
  is on it. When the first reply arrives, those times are moved onto the server's clock, once,
  and if the PC's clock had already crossed a weekly reset the server hasn't, the week is put
  back (mail read under the wrong week is read again at the next read). A VS day's ranking
  that turns out to have been captured before its day ended on the server is dropped, to be
  captured again once the day is over. The sync waits for that
  first reply.
- **Values LastWarHQ would refuse are sent as unknown** (`null`), so one odd value can't get a
  whole upload refused: control characters are removed from names, and a name that is then
  empty or over 64 characters (16 for the tag), a negative power, kill count or score, or a
  warzone outside 1–99,999 is `null`. In each list, a player whose uid isn't 1–20 digits, and
  any past the 200th, is left out, and the list is then marked `complete: false`; a player named
  twice keeps their first entry.

Each panel's newest list is sent as the game sent it, and every entry carries the player's
`uid`. The tool doesn't merge panels or work anything out from them; the receiver joins them
by `uid`. Nothing is sent twice: names and power come only from the roster, and the alliance
only from `alliance`, so the other sections carry just what their panel adds.

```json
{
  "schemaVersion": 2,
  "week": "2026-09-28",
  "generated": "2026-10-04T12:00:00Z",
  "alliance": { "id": "…", "name": "…", "abbr": "…", "warzone": 901, "updated": "…Z" },
  "roster": {
    "updated": "…Z", "complete": true,
    "players": [{ "uid": "…", "name": "…", "rank": 4, "power": 250000000, "armyKill": 2000000 }]
  },
  "dsSignups": {
    "updated": "…Z", "complete": true,
    "players": [{ "uid": "…", "heroPower": 140000000, "chooseTimeList": [2, 1], "group": 1 }]
  },
  "dsResults": {
    "updated": "…Z",
    "battles": [{ "time": "…Z", "won": false, "complete": true, "players": [{ "uid": "…", "score": 5000000 }] }]
  },
  "vs": [
    { "updated": "…Z", "complete": true, "players": [{ "uid": "…", "score": 90000000 }] },
    null, null, null, null, null
  ]
}
```

| Field | Meaning |
|---|---|
| `schemaVersion` | Version of this layout; raised whenever fields change meaning or shape. |
| `week` | Monday (UTC date) of the VS week the data belongs to. |
| `alliance.id` | The alliance the data belongs to: from the logged-in account's own messages, or, until those name it, from the member list. `alliance` is `null` until either has arrived. The logged-in player isn't named separately: they are one of the `roster` players. |
| `alliance.name`, `alliance.abbr` | From the account's profile or the Desert Storm participants panel; `null` while the alliance is known only from the member list. |
| `alliance.warzone` | The `serverId` the member list gives its members, when every member that has one has the same; `null` until the member list has arrived, or if they differ. Sent once here rather than per member. |
| `alliance.updated` | When the fields sent were seen in the game: name and tag with the profile or the Desert Storm panel, the warzone with the member list, each with its own time. LastWarHQ takes one `updated` for the three, so an upload carries only the fields seen at that one time and `null` (no observation) for the rest; the others follow in the next uploads, oldest first. With nothing seen, the member list's time. |
| `updated` | When that panel's list arrived (UTC). |
| `complete` | Every entry in the list could be read. When `false`, a player missing from `players` may still be on the panel. |
| `roster` | The alliance member list. `rank` is 1–5 (R5 highest). Kept across the weekly reset. |
| `dsSignups` | The Desert Storm participants panel, without the name and power the roster gives. `heroPower` is "Total Hero Power". `chooseTimeList`: time slots picked, in the order clicked (`1` = 11:00 UTC, `2` = 20:00 UTC, `3` = 01:00 UTC). `group`: team assigned, `1` = Team A, `2` = Team B, `0` = none. `null` from Saturday 02:00 UTC to the Monday reset. |
| `dsResults` | This week's battles from the result mails: when each ended, whether it was won, and each player's score. `complete: false` means a player entry in the mail couldn't be read. `null` until the mail has been read and the alliance is known. |
| `vs` | Monday to Saturday: each completed day's VS ranking, `null` until that day's tab has been opened. The ranking lists both alliances; only our alliance's players are sent, so `players` is empty until the alliance is known. `complete` is `false` if any row of the ranking couldn't be read, or had no alliance (it might be one of ours). |

Unknown values are `null`. Sections cleared at the weekly reset (`dsSignups`, `dsResults`,
`vs`) go back to `null`.

### Weekly reset

VS days end at **02:00 UTC** (midnight server time), and the week rolls over on Monday at
02:00 UTC. At the rollover, the tool clears that week's data: VS rankings, Desert Storm
results and sign-ups. The roster stays. This happens even if the game is closed. The
payload is built after checking for the rollover, game messages from the week that has ended
are ignored, and mail read in a new week ignores last week's battles, so the data never mixes
two weeks.

Switching to another game account also clears everything and starts fresh. If the account
moves to another alliance, the old alliance's member list, rankings, sign-ups and results are
cleared. Once the account's alliance is known, another alliance's member list is ignored.

### Troubleshooting

| Message | What to do |
|---|---|
| "Npcap is not installed" | Install Npcap from https://npcap.com and start the tool again. |
| Stays on "Starting capture" | Approve the Windows admin prompt; it may be behind other windows. |
| "Waiting for the game" · "Start the game and log in" | Start the game and log in. |
| "No heartbeat from the game" | The game was closed or lost connection. The tool picks up the new connection by itself. |
| "LastWarHQ Scanner is already running" | Another copy is open; use that one. |
| DS results: "Mail: the game is saving its mail" | Nothing; the next read, within 5 minutes, tries again. |
| DS results: "Open the member list" | Open the member list so the tool knows which battles are your alliance's. |
| DS results: "Mail: … WAL mode, which is not supported" | The game stored its mail in a format the tool doesn't read. Please report it. |
| "Update failed: cannot save the update beside the exe" | The exe's folder isn't writable (for example Program Files). Move the exe to a folder you own, or download the release by hand. |

### What it does and doesn't do

- **Receives only.** It captures a copy of the traffic on the game's ports. Only the server's
  messages are decoded; the game's own requests are encrypted and only counted. It sends
  nothing to the game. Its only connections of its own, all over HTTPS, are to GitHub
  (`api.github.com` and `github.com`: one an hour to check for a new release, and, when you
  press Update now, to download it) and to LastWarHQ (`lastwarhq.dev`: signing in, checking
  the sign-in at start-up, and syncing at most once a minute). It listens only while you sign
  in, on `127.0.0.1`, for the browser to come back.
- **Hands off the game.** It never hooks, reads the memory of, or modifies the game. The only
  game file it reads is the mail database.
- **Memory only.** Data lives in memory while the tool runs and is gone when it closes. It
  keeps only its LastWarHQ sign-in token, in Windows Credential Manager, and the only file it
  writes is its own exe, when you update it.
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
decompression), which bring in `arrayvec` and `twox-hash`. The window (Win32, drawn with GDI+
and GDI), HTTPS (WinHTTP), SHA-256 and random bytes (Windows' `bcrypt`), the sign-in token store
(Windows Credential Manager), the sign-in listener, the SQLite reader, JSON and SmartFox
decoding are written in the project, with bounds checks so damaged
input gives an error rather than a crash. Damaged input also can't make them use much memory:

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
  For updates, they cover versions, release replies, checksum files and swapping the exe
  (on files in a temporary folder). For signing in, they cover PKCE (against RFC 7636's
  example), the connect address, API answers and errors, and the callback listener, which
  they drive through a socket on `127.0.0.1`. For syncing, they cover when an upload is due,
  the waits LastWarHQ asks for, its answers and errors, the game-clock correction and the
  values sent as unknown.
- No test uses real game data: no captures, no game files. None of them need Npcap or the game
  installed, none connect to the internet, and none touch Credential Manager.

### Releases

`.github/workflows/release.yml` runs on every push to `main`, including merged pull requests:

1. On Windows: formatting, Clippy and the tests, then `cargo build --release --locked`.
2. The exe and its SHA-256 are kept as a workflow artifact.
3. If the version in `Cargo.toml` has no release yet, a release `v<version>` is published with
   both files and marked as the latest. Otherwise nothing is published. Raising the version is
   what releases a new version, and every running copy offers it as an update within an hour.

The build job only has read access to the repository; the separate publish job is the only one
that can create releases. The workflow uses only GitHub's own actions (`checkout`,
`upload-artifact`, `download-artifact`), pinned to exact commits.

### Code layout

| Folder | Job |
|---|---|
| `src/app` | Start-up, capture loop, mail reads; shared state; the data payload |
| `src/capture` | Adapter list, Npcap capture, TCP reassembly, locking on to the game connection |
| `src/protocol` | SmartFox framing, decompression and SFSObject decoding |
| `src/game` | The player view, account, player records, Desert Storm battles and VS week rules |
| `src/mail` | The read-only SQLite reader and Desert Storm result mails |
| `src/ui` | The Win32 window, what it shows, and drawing it |
| `src/update` | Checking GitHub for releases; downloading, checking and swapping in the new exe |
| `src/auth` | Signing in to LastWarHQ: browser sign-in, the scanner API, the stored token |
| `src/net` | HTTPS through WinHTTP |
| `src/sync.rs` | Uploading the payload to LastWarHQ when it changes |
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
- If an entry in `ranks` can't be read, the battle is marked incomplete: the unreadable entry
  could be any player.
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

### Keeping the panels

- Each panel keeps only its newest list, as sent: the member list, the participants panel,
  and one VS ranking per day. Panels aren't merged; every entry carries its player's `uid`.
- A newer list replaces the last one whole. An empty list (nothing readable) changes nothing.
  A list with an unreadable entry is kept and marked incomplete.
- Today's VS ranking is skipped until the day ends.
- From Saturday, the participants panel lists the alliance with empty `chooseTimeList` and no
  other sign-up fields; see [Desert Storm sign-ups](#desert-storm-sign-ups).

### Other findings

- Rally messages (`push.alliance.march.create`, `push.alliance.march.refresh`) carry per-march
  `power` and an `armyInfo` field that is base64-encoded protobuf.
- The game's `Player.log` is empty during play.

---

## Licence

[MIT](LICENSE)
