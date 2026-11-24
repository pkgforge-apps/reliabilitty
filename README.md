<p align="center">
  <img src="assets/icon.svg" width="128" alt="">
</p>

<h1 align="center">Reliabilitty</h1>

<p align="center">A small, fast, reliable X11 terminal emulator.</p>

<p align="center">
  <a href="https://github.com/pkgforge-apps/reliabilitty/releases/latest">Latest release</a>
</p>

It renders text entirely on the CPU, with no GPU or driver on the critical
path, yet launches quicker and uses less memory and CPU than the
hardware-accelerated terminals it is compared against.

## Why another terminal emulator?

See [why.md](why.md).

## Performance

See [PERFORMANCE.md](PERFORMANCE.md) for benchmarks.

## Status

Linux and X11 only. There is no Wayland, macOS or Windows backend.

This is an early release. The following are implemented and exercised:

- UTF-8, wide characters, combining marks, emoji fallback and 256/truecolour
  SGR.
- Primary and alternate screens, scrollback, scrolling regions, insert/delete
  line and character operations, tab stops and DEC graphics.
- Cursor styles, blinking, reverse video, underline/strike/overline, dim and
  bold.
- Mouse reporting (X10, 1000, 1002, 1003, SGR, urxvt and UTF-8 encodings).
- Selection with double-click words and triple-click lines, PRIMARY and
  CLIPBOARD copy, middle-click paste, bracketed paste and OSC 52.
- A right-click menu with copy, paste, select all and search. Applications
  that request mouse input keep the right button, so use Shift+right-click
  for the menu there.
- Scrollback search with case-insensitive highlighting, Ctrl+Shift+F and
  Enter to step through matches.
- OSC titles, palette changes, default colour changes and terminal responses.
- OSC 8 hyperlinks, underlined and opened with Ctrl+click via `xdg-open`.
- Shell-style configuration with live reload and environment overrides.
- Software rendering through SHM with a `PutImage` fallback.
- True per-pixel background transparency through a 32-bit ARGB visual, with
  whole-window compositor opacity as a fallback.
- A draggable scrollbar for scrollback.
- Static musl builds for x86_64 and aarch64, well below 3 MiB.

## Build

```sh
cargo build --release
./target/release/rtty
```

For a fully static musl binary:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked --target x86_64-unknown-linux-musl
strip target/x86_64-unknown-linux-musl/release/rtty
```

For aarch64, link with the host toolchain's `rust-lld`:

```sh
rustup target add aarch64-unknown-linux-musl
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin/rust-lld"
cargo build --release --locked --target aarch64-unknown-linux-musl
```

The result is `target/<target>/release/rtty`. It has no dynamic dependencies.
The static binary still uses the fonts installed on the host. No font files are
embedded and no FreeType/fontconfig runtime library is linked. The system font
list is scanned once at startup and kept for the process lifetime, so fonts
installed after launch are not picked up by a config reload or font-size change;
relaunch the application to see newly installed fonts.

Build needs only a Rust toolchain. The crate talks to X11 over the socket and
does not link libX11.

## Install

Or just install the released binary:

```sh
wget -O "reliabilitty-$(uname -m)-linux.tar.xz" "https://github.com/pkgforge-apps/reliabilitty/releases/latest/download/reliabilitty-$(uname -m)-linux.tar.xz" && \
tar -xJf "reliabilitty-$(uname -m)-linux.tar.xz" && \
mkdir -p "${XDG_BIN_HOME:-$HOME/.local/bin}" "${XDG_DATA_HOME:-$HOME/.local/share}/applications" && \
cp "./reliabilitty-$(uname -m)-linux/rtty" "${XDG_BIN_HOME:-$HOME/.local/bin}/" && \
cp "./reliabilitty-$(uname -m)-linux/share/applications/reliabilitty.desktop" "${XDG_DATA_HOME:-$HOME/.local/share}/applications/" && \
cp -r "./reliabilitty-$(uname -m)-linux/share/icons" "${XDG_DATA_HOME:-$HOME/.local/share}/"
```

## Usage

```sh
rtty [OPTIONS] [--] [COMMAND [ARGS...]]
rtty -e COMMAND [ARGS...]
```

Options:

- `-e, --command COMMAND...` run a command instead of the shell.
- `-t, --title TITLE` set the window title.
- `--class CLASS` set the X11 WM_CLASS class.
- `--instance NAME` set the X11 WM_CLASS instance.
- `-d, --working-directory DIR` set the child's starting directory.
- `--config-file PATH` use a specific configuration file.
- `--hold` keep the window open after the child exits.
- `--smoke` draw one frame and exit; useful for CI.
- `-h, --help`, `-V, --version`.

The child gets `TERM=xterm-256color`, `COLORTERM=truecolor`,
`TERM_PROGRAM=Reliabilitty` and `WINDOWID` unless overridden with `ENV_*`.

## Configuration

Reliabilitty looks for configuration in this order:

1. `--config-file PATH`
2. `$RTTY_CONFIG`
3. `$XDG_CONFIG_HOME/rtty/rtty.conf` or `~/.config/rtty/rtty.conf`
4. `/etc/rtty/rtty.conf`
5. built-in defaults

The file is a flat list of `KEY=value` lines. Unknown keys and invalid values
are reported. Environment variables override the file: `TRANSPARENCY=0.8` in
the file is overridden by `RTTY_TRANSPARENCY=0.8`. Priority is CLI, then
environment, then config file, then defaults. Saving a valid file reloads
colours, bindings, fonts, padding, scrolling, selection and bell settings
without restarting the window. `SIGHUP` forces a reload.

See [`rtty.example.conf`](rtty.example.conf) for every option.

### Default bindings

| Keys | Action |
| --- | --- |
| `Ctrl+Shift+C` | Copy to CLIPBOARD and PRIMARY |
| `Ctrl+Shift+V` | Paste CLIPBOARD |
| `Shift+Insert` | Paste CLIPBOARD |
| `Ctrl+Insert` | Copy |
| `Ctrl+Shift+F` | Search the scrollback |
| `Shift+PageUp` / `Shift+PageDown` | Scroll one page |
| `Ctrl+Shift+Home` / `Ctrl+Shift+End` | Oldest line / live screen |
| `Ctrl+Shift+Up` / `Ctrl+Shift+Down` | Scroll one line |
| `Ctrl+Shift+=` / `Ctrl+Shift+-` | Larger / smaller font |
| `Ctrl+Shift+0` | Reset font size |
| `F11` | Toggle fullscreen |
| `Ctrl+Shift+Alt+C` | Open another window |
| `Shift` + click | Select even when the application requested mouse input |
| `Ctrl` + click | Open an OSC 8 hyperlink with `xdg-open` |
| Middle click | Paste PRIMARY |
| Double click / triple click | Select word / line |

While the search bar is open, typing edits the query, Enter and Shift+Enter
move to the next and previous match, and Escape closes it. Search highlights
use `SEARCH_MATCH_BACKGROUND` and `SEARCH_CURRENT_BACKGROUND`, with text
contrast chosen automatically. Ctrl+F is left to the shell and full-screen
applications; bind it to `search` in the config to use it for the search bar
instead.

Any `BINDING` lines in the config replace the built-in list completely.

## Design

```text
X11 protocol (x11rb) -> window events, keys, mouse, selections
        |
        v
VT parser -> terminal grid + scrollback -> selection/clipboard
        |                                      |
        v                                      v
PTY child (libc openpty/fork/exec)       software renderer (swash)
                                               |
                                               v
                                         RGB canvas -> MIT-SHM -> X11
```

There is no GUI toolkit. X11 is used directly, glyphs are rasterized with
`swash`, fonts are discovered with `fontdb`, and the framebuffer, damage
tracking, cursor, selection and rendering loops are part of this program.
That keeps the dependency graph small and removes the GPU driver from the
critical path.

Rendering only touches damaged terminal rows. The canvas is converted to the
server's TrueColor visual and uploaded with MIT-SHM; if the extension is
unavailable, `PutImage` is used.

## Limitations

The current release intentionally keeps a narrow scope:

- X11 only. No Wayland, Windows or macOS.
- No XIM/IME support yet; dead keys are composed internally for Latin layouts.
- No ligatures, font feature configuration or image protocols.
- Selection requests using X11 `INCR` transfers are refused rather than pasted
  partially. Large clipboard transfers may therefore be unavailable.
- No tabs, splits, transparency blur or background images.
- Transparency needs a compositor. Without ARGB32 the fallback is
  whole-window opacity, so text is translucent too.
- Cursor blink, visual bell and OSC 52 are implemented; font fallback depends
  on the fonts installed on the host.

## Tests

```sh
cargo test
cargo clippy --all-targets
./target/release/rtty --smoke
```

`--smoke` needs a reachable X11 display (`DISPLAY`), and returns success after
drawing one frame. `Xvfb :99 -screen 0 1280x800x24` is enough for headless
checks.

## License

MIT. See [`LICENSE`](LICENSE).
