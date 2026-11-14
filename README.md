# Weathered Launcher

Weathered Launcher is an open-source launcher for Minecraft: Java Edition, named
for the weathered copper block. Downloaded files live once in a shared store.
Each instance has its own mods, saves, config, and resource packs, so two
instances can play the same version with different mods. The command is
`weathered`. The download, rule, and launch logic lives in the `weathered-core`
library so a graphical client can be added later without rewriting it.

Weathered is not an official Minecraft product. Minecraft is a trademark of Mojang
Synergies AB.

## Features

- Lists releases and snapshots from Mojang's version manifest (`release` and `snapshot` select the latest of each).
- Stores the client jar, libraries, assets, loader files, and modpack files once, keyed by SHA-512.
- Downloads vanilla, Fabric, Quilt, Forge, and NeoForge profiles into that store, and launches them.
- Creates a vanilla instance from a stored Minecraft version, a loader instance from a stored loader build, or an instance from a Modrinth `.mrpack`.
- Downloads run concurrently, up to a configurable limit shared by every install, and report each job plus one overall progress bar.
- Applies library and argument rules for Windows, macOS, and Linux, including architecture and feature flags.
- Merges `inheritsFrom` versions: child libraries are prepended, and argument lists are concatenated.
- Builds the modern `arguments` block and the legacy `minecraftArguments` string, including classpath and natives.
- Finds a system JDK (`PATH`, `JAVA_HOME`, and common install locations) or downloads the Mojang Java runtime the version asks for.
- Launches offline with a username. The offline UUID matches Minecraft's `OfflinePlayer:<name>` scheme.
- Microsoft account login (device code, Xbox Live, XSTS, Minecraft Services) behind `weathered-core::auth::microsoft`. The Azure client id is not built in; you supply one.
- A full-screen terminal UI (`weathered` or `weathered tui`) with Launch, Download, Settings, and More pages. The Launch page lists instances and a New instance button. Tabs, lists, and buttons are clickable, and the mouse wheel scrolls. Several installs can run at once and show their own progress plus one overall bar.

## Build

Install a current stable Rust toolchain (1.89 or newer; the workspace uses Rust 2024), then:

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
weathered download 1.21.1
weathered download 1.21.1 --loader fabric
weathered download 1.21.1 --loader quilt --loader-version 0.27.0
weathered instance list
weathered instance create --name Skyblock 1.21.1 --loader fabric
weathered instance import ./pack.mrpack
weathered instance delete Skyblock
weathered launch Skyblock --username Steve
weathered launch Skyblock --username Steve --memory 4G --dry-run
weathered java
weathered login --client-id <azure-application-id>
weathered accounts list
```

`weathered install` is the same command as `weathered download`. `--jobs` sets how many files download at once (default 16, or `download_jobs` in the config file).

### Instances

An instance is a name plus a Minecraft version, an optional loader, and its own game directory. `instance create` uses a profile that is already in the store, or downloads it first. `instance import` reads a Modrinth `.mrpack`: it downloads the Minecraft version and Fabric or Quilt loader, stores each mod once, then copies the mods and `overrides/` into the new instance.

The Launch page lists those instances. **New instance** offers Download, which opens the Download page, or import a `.mrpack`. Press `n` on the Launch page to open that flow. The Download page installs a vanilla version or creates a vanilla instance from one that is already stored. Its Loaders page does the same for Fabric, Quilt, Forge, and NeoForge. Launch uses the selected instance.

Libraries are not copied into the instance. The classpath points at the shared store. The client jar, version JSON, mods, configs, and pack overrides are copied, so the instance keeps its own copies of those files. Asset bytes stay in the store and are hard-linked (or copied, when a link is not possible) into a shared `assets/objects/<sha1>` tree, because the game looks assets up by SHA-1.

### Shared store

```
<data>/
  store/objects/<sha512[0..2]>/<sha512>
  store/index/sha1/<sha1>          # sha512 of that file
  store/index/url/<sha256>/<sha256>
  catalog/<id>.json
  assets/                          # shared SHA-1 view of asset objects
  instances/<id>/instance.toml
  instances/<id>/game/             # mods, saves, config, resourcepacks, version json, client jar
  runtime/                         # unpacked Mojang Java runtimes
  versions/                        # legacy installs, left in place
```

`<data>` is `$WEATHERED_GAME_DIR` when that variable is set, otherwise the platform data directory plus `weathered`. Mojang publishes SHA-1, so the index records `sha1 -> sha512` and `url -> sha512`. A later download checks those indexes before opening a connection, verifies SHA-1 when the source provided one, and stores the SHA-512.

### Older installs

If the data directory already contains `versions/<id>/<id>.jar` from an earlier Weathered install, and no instances exist yet, Weathered adds one legacy instance per version. Those instances keep using the old directory. Their files are not moved. New instances use `instances/<id>/game`.

### Terminal UI

`weathered` with no subcommand opens the full-screen UI. `weathered tui` does the same thing.

The layout follows Plain Craft Launcher: a top bar for Launch, Download, Settings, and More; a launch page with the account on the left and the instance list on the right; a download page split into release, snapshot, and old versions; and a settings page for the data directory, Java, memory, JVM arguments, offline username, and Microsoft sign-in. The screen is dark, with a weathered-copper teal for the selection and a black-and-gold focus on the row buttons.

The footer lists `[Q][E] pages` with `[W][A][S][D] move`, then that page's actions, then the Ctrl combinations. Letter shortcuts ignore Caps Lock. On an instance row, left and right switch only between Launch and Edit. The focused button is the gold label, such as `>Launch<`, and a newly selected row starts on Launch. Clicking the name selects that row. Clicking `[Launch]` starts the game, and clicking `[Edit]` opens the editor. At startup the default instance's Launch button is focused, so Enter starts the game. Several versions can download at once, sharing one file-concurrency limit. Esc cancels the latest job. A download does not block launching a different instance. The path in the top bar opens the data directory. Tab reaches it and Enter opens it. Text fields still accept `q`, `e`, `w`, `s`, `a`, and `d` as letters, including their case. While a field is open, the characters you type stay on screen with a cursor. Enter saves the field and Esc discards it. Ctrl+C quits.

The search box on Download stays hidden until Ctrl+F, and it is the only extra row between the category tabs and the version list. The first Esc clears that search. A later Esc cancels a download or leaves the edit page. `O` cycles the instance list between name, creation time, and last played, and cycles the download list between newest, oldest, version name, and stored-first. The version panel title shows the current download order. Both choices are saved. The default instance stays pinned at the top. `R` and Ctrl+R refresh the version list. The list is cached in memory for the session and under `<data>/cache/` for an hour, shown immediately, and refreshed in the background. A failed refresh keeps the cached list so the page still works offline.

Each instance row ends with Launch and Edit. The focused one is drawn in gold, and the row name is not a separate stop. Clicking the name selects the row. Clicking Launch starts the game. Edit is its own page, with the account panel left off. It renames the instance, edits the description, sets or clears the default, deletes the instance, and sets Java, minimum memory, maximum memory, and JVM arguments. An empty override shows `inherit global` and uses the settings page value. Clearing the field returns it to that inherited value. Open folder opens the instance game directory in the system file manager. Actions run immediately. Delete removes only that instance directory. The shared store stays. Game output is written to `<data>/logs/launch-<timestamp>.log`. If the game crashes, the status line gives that path and any new file in the instance `crash-reports` directory.

Each download row ends with the same kind of button. `[Install]` stores a vanilla version that is not downloaded yet. When it is already in the store the button becomes `[Create instance]` and asks for a name, defaulting to the version id. `[Loaders]` opens a page for that Minecraft version. That page lists Fabric, Quilt, Forge, and NeoForge builds. `A` and `D` switch loader, `B` shows unstable builds, and the recommended or newest stable build is highlighted. `[Download]` stores a build. `[Create instance]` on a stored build makes an instance that includes the loader. Esc returns to the version list at the same row. The Microsoft sign-in dialog is cleared before it is drawn and sized to its text.

```sh
weathered --game-dir ~/.local/share/weathered
```

A rendered Launch page:

```text
 Weathered Launcher                                             /tmp/weathered-game
  Launch   Download   Settings   More
╭ Account ─────────────────────╮ ╭ Instances ────────────────────────────────────╮
│ S  Steve                     │ │ ▸ Vanilla  1.21.1  vanilla  [Launch] [Edit] │
│    offline                   │ │   Beta     b1.7.3   vanilla  [Launch] [Edit] │
│ Profiles                     │ │ + New instance                               │
│ ▸ Steve          offline     │ │                                              │
╰──────────────────────────────╯ ╰───────────────────────────────────────────────╯
 Ready  ·  [Q][E] pages [W][A][S][D] move  Enter launch  O sort  N new  Ctrl+C quit
```

Download progress stays on screen while files are fetched:

```text
  Release   Snapshot   Old
╭ Versions · newest ─────────────────────────────────────────────────────────────╮
│ ▸ 1.21.1               2024-08-08   in store   [Create instance] [Loaders] │
│   1.20.4               2023-12-07   not stored [Install]          [Loaders] │
╰────────────────────────────────────────────────────────────────────────────────╯
  assets 30%  icons/icon.png
 █████████████████████████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░
```

The Launch button and the selected rows are filled with color in a real terminal. This capture is the text of a 100×36 frame. Install and launch run on background tasks, so the mouse and keyboard keep working while a progress bar is moving. Stop ends the game process. The captured output stays in `logs/`, and the status line reports a crash.

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

`--game-dir` overrides the data directory for one command. Each new instance
then keeps its own game directory under `instances/<id>/game`, using the folder
names the game expects. A legacy instance can still point `--game-dir` at an
existing `.minecraft` directory.

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
UI (page changes, mouse hits, wheel scrolling, and rendered frames), the
content store (one object per SHA-512, with SHA-1 and URL indexes), instance
creation, legacy migration, and Modrinth `.mrpack` parsing. They do
not download Minecraft.

GitHub Actions runs the same three commands on Linux, macOS, and Windows.

## Known gaps

- There is a terminal UI, not a desktop window. The game itself still opens in its own window.
- Fabric, Quilt, Forge 1.13+, and NeoForge can be downloaded and launched from the Loaders page. `B` shows unstable builds. Forge builds from before 1.13 use a different installer and are refused with an explanation. LiteLoader and OptiFine are not installed.
- CurseForge modpacks are not imported. A CurseForge zip names project and file ids, and downloading them needs an API key this launcher does not ship. Modrinth `.mrpack` files are supported for vanilla, Fabric, and Quilt. Forge and NeoForge packs are still refused because those installers need the processor step, which the pack import does not run on its own.
- LiteLoader and OptiFine are not installed. `inheritsFrom` is implemented, so a version JSON that is already on disk can still be launched.
- Microsoft login requires your own Azure client id, and Xbox Live may reject it.
- Mojang does not publish a Java runtime for Linux on ARM64. Install a JDK and pass `--java`.
- Older native classifiers that use `${arch}` expand to `32` or `64`. That does not select an ARM build.
- Very long classpaths on Java 8 are passed as normal arguments and can hit the Windows command-line limit. Java 9 and newer use an argument file when the command is longer than about 7 KB. That file can contain an access token and is written as `weathered/launch-args.txt` with user-only permissions.
- Servers and the demo-only library set are outside this version. `--demo`, custom resolution, and Quick Play flags are supported. Modrinth packs are imported; CurseForge packs are not.

## License

MIT. See [LICENSE](LICENSE).
