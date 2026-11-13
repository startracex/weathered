# Weathered Launcher

Weathered Launcher is an open-source launcher for Minecraft: Java Edition, named
for the weathered copper block. It downloads a version from Mojang, checks every
file with SHA-1, finds a Java runtime, and starts the game. The command is
`weathered`. The download, rule, and launch logic lives in the `weathered-core`
library so a graphical client can be added later without rewriting it.

Weathered is not an official Minecraft product. Minecraft is a trademark of Mojang
Synergies AB.

## Features

- Lists releases and snapshots from Mojang's version manifest (`release` and `snapshot` select the latest of each).
- Downloads the client jar, libraries, native libraries, asset index, and assets into the standard game-directory layout.
- Skips files that are already present and match their SHA-1. Downloads run concurrently and report progress.
- Applies library and argument rules for Windows, macOS, and Linux, including architecture and feature flags.
- Merges `inheritsFrom` versions: child libraries are prepended, and argument lists are concatenated.
- Builds the modern `arguments` block and the legacy `minecraftArguments` string, including classpath and natives.
- Finds a system JDK (`PATH`, `JAVA_HOME`, and common install locations) or downloads the Mojang Java runtime the version asks for.
- Launches offline with a username. The offline UUID matches Minecraft's `OfflinePlayer:<name>` scheme.
- Microsoft account login (device code, Xbox Live, XSTS, Minecraft Services) behind `weathered-core::auth::microsoft`. The Azure client id is not built in; you supply one.
- A full-screen terminal UI (`weathered` or `weathered tui`) with Launch, Download, Settings, and More pages. Tabs, lists, and buttons are clickable, and the mouse wheel scrolls. Install and launch progress updates live.

## Build

Install a current stable Rust toolchain (1.89 or newer), then:

```sh
cargo build --release
```

The binary is `target/release/weathered`. During development, `cargo run --` works too:

```sh
cargo run -- list
```

## Usage

```sh
weathered
weathered tui
weathered list
weathered list --kind snapshot --search 1.21
weathered install 1.21.1
weathered install release --game-dir ~/instances/vanilla
weathered launch 1.21.1 --username Steve
weathered launch 1.21.1 --username Steve --memory 4G --dry-run
weathered java
weathered login --client-id <azure-application-id>
weathered accounts list
```

### Terminal UI

`weathered` with no subcommand opens the full-screen UI. `weathered tui` does the same thing. The command-line subcommands are unchanged.

The layout follows Plain Craft Launcher: a top bar for Launch, Download, Settings, and More; a launch page with the account on the left and a Launch button; a download page split into release, snapshot, and old versions; and a settings page for the game directory, Java, memory, offline username, and Microsoft sign-in. The screen is dark, with a weathered-copper teal for the selection and a greener Launch control.

Click a tab, a version, or a button. The wheel scrolls the version lists. Keys work too: `1`–`4` switch pages, the arrows move, Enter activates, `q` quits.

```sh
weathered --game-dir ~/.local/share/weathered
```

A rendered Launch page:

```text
 weathered  launcher                                              /tmp/weathered-game
  Launch   Download   Settings   More
╭ Account ─────────────────────╮ ╭ Play ─────────────────────────────────────────╮
│ S  Steve                     │ │ ▸ 1.21.1             release                  │
│    offline                   │ │   b1.7.3             old_beta                 │
│ Profiles                     │ │                                               │
│ ▸ Steve          offline     │ │              1.21.1   ·   2G                  │
│                              │ │                   LAUNCH                      │
╰──────────────────────────────╯ ╰───────────────────────────────────────────────╯
 Ready  ·  1-4 pages   ↑↓ move   enter   click & wheel   q quit
```

Download progress stays on screen while files are fetched:

```text
  Release   Snapshot   Old
  search
╭ Versions ──────────────────────────────────────────────────────────────────────╮
│ ▸ 1.21.1               2024-08-08   installed                       reinstall  │
│   1.20.4               2023-12-07   not installed                   install    │
╰────────────────────────────────────────────────────────────────────────────────╯
  assets 30%  icons/icon.png
 █████████████████████████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░
```

The Launch button and the selected rows are filled with color in a real terminal. This capture is the text of a 100×36 frame. Install and launch run on background tasks, so the mouse and keyboard keep working while a progress bar is moving. The game's own log is appended under the Launch button. Stop kills that process.

`weathered list` prints the latest release and snapshot, then the newest matching
versions. `--limit 0` shows the whole list. `--kind` accepts `release`,
`snapshot`, `old-beta`, `old-alpha`, and `all`.

The first install of a current release downloads the client, libraries, and
several hundred megabytes of assets. Later installs reuse files whose SHA-1
already matches.

### Game directory

The default directory is `$WEATHERED_GAME_DIR` when that variable is set.
Otherwise it is the platform data directory plus `weathered`:

| Platform | Default |
| --- | --- |
| Linux | `~/.local/share/weathered` |
| macOS | `~/Library/Application Support/weathered` |
| Windows | `%APPDATA%\weathered` |

`--game-dir` overrides it for one command. The path can be an existing
`.minecraft` directory; Weathered uses the same folder names the game expects.

```
<game-dir>/
  versions/<id>/<id>.json
  versions/<id>/<id>.jar
  versions/<id>/natives/
  libraries/
  assets/indexes/
  assets/objects/
  assets/log_configs/
  assets/virtual/<index>/     # indexes marked "virtual"
  resources/                  # indexes marked "map_to_resources" (pre-1.6)
  runtime/<component>/<os>/   # Mojang Java runtime, when downloaded
  weathered/                    # launcher cache, not read by the game
```

### Java

On install, Weathered uses a JDK whose major version matches the version JSON
(Java 21 for current releases, Java 8 for `jre-legacy`). Search order:

1. A runtime already downloaded into the game directory.
2. `JAVA_HOME`, then `java` on `PATH`, then common install directories.
3. Mojang's runtime for the version's `javaVersion.component`, unless `--no-java` is set.

`--download-java` fetches Mojang's runtime even when a system JDK already
matches. `weathered java` prints what was detected. `weathered launch --java /path/to/java`
uses a specific binary.

### Offline play

```sh
weathered launch 1.21.1 --username Steve
```

No account check is made. The access token passed to the game is `0`, and the
UUID is stable for that username.

### Microsoft accounts

Device-code login needs an Azure **public** client id. Weathered does not ship
one.

1. Open [App registrations](https://portal.azure.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade) and create a registration.
2. Supported account types: **Personal Microsoft accounts only**.
3. Leave the redirect URI empty. The device-code flow does not use one.
4. Copy the Application (client) ID.
5. In the app, open Authentication → Advanced settings → **Allow public client flows: Yes**.

Pass the id in any of these ways:

```sh
weathered login --client-id 00000000-0000-0000-0000-000000000000
export WEATHERED_MS_CLIENT_ID=00000000-0000-0000-0000-000000000000
```

or in the config file (`~/.config/weathered/config.toml` on Linux,
`~/Library/Application Support/weathered/config.toml` on macOS,
`%APPDATA%\weathered\config.toml` on Windows):

```toml
ms_client_id = "00000000-0000-0000-0000-000000000000"
```

`weathered login` prints a URL and a short code. Open the URL in a browser, enter
the code, and approve the request. The Minecraft profile name and tokens are
stored in `accounts.json` next to the config file (mode `0600` on Unix).
Launch without `--username` uses the active account and refreshes an expired
token.

Xbox Live rejects some Azure applications with **XErr 2148916233** ("this app
is not allowed to use Xbox Live"), even when the account owns the game. That
decision is Microsoft's. Offline `--username` launches do not need this login.

The config also stores a random `client_id`. That value is sent to the game as
`${clientid}`. It is not the Azure application id.

## Project layout

```
crates/weathered-core   library: manifest, downloads, rules, Java, auth, launch
crates/weathered    `weathered` binary, including the terminal UI
```

A GUI should depend on `weathered-core` and call `install_version`,
`prepare_launch`, and `auth::MicrosoftAuth`. Progress is a callback
(`ProgressSink`); the library does not print.

## Tests

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Unit tests cover manifest parsing, `inheritsFrom` merging, library and argument
rules (including OS, architecture, Windows version regexes, and feature
flags), Maven paths, native classifier selection, legacy and modern launch
arguments, offline UUIDs, the Microsoft identity-token format, and the terminal
UI (page changes, mouse hits, wheel scrolling, and rendered frames). They do
not download Minecraft.

GitHub Actions runs the same three commands on Linux, macOS, and Windows.

## Known gaps

- There is a terminal UI, not a desktop window. The game itself still opens in its own window.
- Forge, Fabric, and Quilt are not installed. `inheritsFrom` is implemented, so a version JSON that is already on disk can be launched, but Weathered does not run those installers.
- Microsoft login requires your own Azure client id, and Xbox Live may reject it.
- Mojang does not publish a Java runtime for Linux on ARM64. Install a JDK and pass `--java`.
- Older native classifiers that use `${arch}` expand to `32` or `64`. That does not select an ARM build.
- Very long classpaths on Java 8 are passed as normal arguments and can hit the Windows command-line limit. Java 9 and newer use an argument file when the command is longer than about 7 KB. That file can contain an access token and is written as `weathered/launch-args.txt` with user-only permissions.
- Servers, modpacks, and the demo-only library set are outside this version. `--demo`, custom resolution, and Quick Play flags are supported.

## License

MIT. See [LICENSE](LICENSE).
