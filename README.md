# LexVault

LexVault is an offline English-Chinese dictionary written in Rust.

Current implementation:

- Youdao desktop dictionary data, converted to one local SQLite database
- Read-only SQLite lookup
- `lexvault <word>` command-line lookup
- `lexvault --tui` interactive terminal interface
- `lexvault-gui` desktop interface using Ely GPUI Components
- Paged search results with lazy loading
- Nearest-word fallback using Damerau-Levenshtein distance

## Dictionary data is not bundled

LexVault does not include the Youdao installer or any converted dictionary
database in its repository, release archive, or installer.

The tested source is:

```text
YoudaoDictFull.exe
Version 6.3.69.8341
```

Use of the proprietary dictionary content is the user's responsibility.

## First-time setup

The installer and nested 7z can be unpacked without running them:

```bash
7z x YoudaoDictFull.exe -o/tmp/youdao-outer '$PLUGINSDIR/dict_local.7z'
7z x /tmp/youdao-outer/'$PLUGINSDIR'/dict_local.7z -o/tmp/youdao-dict
```

The extracted dictionary files are in:

```text
/tmp/youdao-dict/localdicts/
```

Build and run the converter:

```bash
cargo build --release -p lexvault-youdao-convert

target/release/lexvault-youdao-convert \
  --input /tmp/youdao-dict/localdicts \
  --output /path/to/youdao.db
```

The verified conversion produces:

```text
528,811 rows
En2Cn: 352,430
Cn2En: 176,381
SQLite size: about 205 MB
```

## Fixed data location

Place the converted database at:

Linux:

```text
~/.local/share/lexvault/dictionaries/youdao-6.3.69.8341/youdao.db
```

macOS:

```text
~/Library/Application Support/LexVault/dictionaries/youdao-6.3.69.8341/youdao.db
```

Windows:

```text
%LOCALAPPDATA%\LexVault\dictionaries\youdao-6.3.69.8341\youdao.db
```

For development, `LEXVAULT_DATA_DIR` may override the platform data root.

On first startup, if the database is missing, LexVault prints the converter
command and the exact destination path. It does not download ECDICT or any
other dictionary automatically.

## Build

The desktop GUI uses Ely's core components with GPUI `0.3.11`, pinned to Ely
commit `e0f355a`.

On Debian/Ubuntu, install the GPUI Linux build dependencies:

```bash
sudo apt install libxkbcommon-x11-dev
```

Wayland and font discovery libraries are also required by the distribution's
Rust/GPUI packaging guidance.

```bash
cargo build --release
```

Binaries:

```text
target/release/lexvault
target/release/lexvault-gui
target/release/lexvault-youdao-convert
```

## Desktop GUI

```bash
lexvault-gui
```

The GUI uses the same fixed dictionary location and read-only database as the
CLI and TUI. Typing immediately loads the first result. Select another result
with `Up`/`Down`, `Ctrl+N`/`Ctrl+P`, or a mouse click. Result pages load as the
list reaches its end.

If the database is missing, the window shows the converter command and exact
destination path instead of exiting silently. After adding the database, press
“重新检查” to open it without restarting.

## Windows releases

Pushing a version tag creates a GitHub Release with native Windows x64 and
ARM64 packages:

```bash
git tag v0.1.0
git push origin v0.1.0
```

Each ZIP contains `lexvault.exe`, `lexvault-gui.exe`,
`lexvault-youdao-convert.exe`, README, LICENSE and the format documentation.
It does not contain any dictionary database. The release also includes
`SHA256SUMS.txt`.

The workflow can be started manually from the Actions tab for build
verification; manual runs upload packages as Actions artifacts without
creating a release. Pull requests and pushes to `main` run the same build as
CI and retain packages as Actions artifacts.

## Look up a word

```bash
lexvault apple
```

If there is no exact result:

```bash
lexvault aple
```

`lexvault tui` looks up the word `tui`; it does not start the TUI.

## Interactive TUI

```bash
lexvault --tui
```

Keys:

- `Up` / `Down`: select a suggestion and load its definition
- `Esc`: clear the query
- `PageUp` / `PageDown`: scroll definitions
- `Ctrl+C`: exit

The TUI fills the viewport, preloads a buffer, and loads more results as the
selection moves downward. There is no fixed result-count limit.

## Read-only behavior

The converted database is opened with `SQLITE_OPEN_READ_ONLY`. LexVault does
not create tables, indexes, WAL files, or journal files in the dictionary
directory.

## Documentation

- [Implementation plan](docs/implementation-plan.md)
- [Youdao YDIC format analysis](docs/youdao-ydic-format.md)
