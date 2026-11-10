# handwriting-input

A handwriting panel for **Wayland**: draw a letter with a finger, stylus or mouse,
tap a candidate, and the text appears in whatever application has focus.
Single Rust binary, [egui] + [wgpu], no winit, no Electron, no browser.

Recognition is done by Google's handwriting API (the same endpoint the Chrome OS
virtual keyboard uses), so Latin scripts recognise very well and CJK works as a
bonus. **See [Privacy](#privacy) before using it.**

## Requirements

| | |
|---|---|
| Compositor | wlroots-based: `zwlr_layer_shell_v1` is required (sway, Hyprland, river, Wayfire, labwc, noctalia, …). GNOME and wlroots-free X11 setups will not work. |
| GPU | anything wgpu 30 speaks: Vulkan or OpenGL ES |
| Network | `www.google.com` |
| Fonts | extended Latin is built in; CJK candidates need a system CJK font (see [Fonts](#fonts)) |

Three optional protocols change how text gets into the target app — the app uses
whichever exist and degrades gracefully: `zwp_input_method_v2`,
`zwp_virtual_keyboard_v1`, `zwlr_data_control_v1`.
`wp_fractional_scale_v1` + `wp_viewporter` give crisp HiDPI output,
`zxdg_output_v1` is used to size the panel.

## Run

```sh
# from GitHub
nix run "github:C10udburst/handwriting-input#default"

# with an explicit config file (the only CLI argument)
nix run "github:C10udburst/handwriting-input#default" -- ~/my-config.toml
```

Nix caches the `master` tarball of a GitHub flake for an hour, so right after a
push add `--refresh`.

Building by hand:

```sh
cargo build --release                # or: nix build .#handwriting-input
./target/release/handwriting-input
```

The build uses wayland-sys' `dlopen` backend, so no `pkg-config` is needed at
compile time — but on NixOS a bare `cargo` binary must be told where the
libraries live at runtime (the Nix package does this wrapping automatically):

```sh
export LD_LIBRARY_PATH="$(nix eval --raw --impure \
  --expr 'with import <nixpkgs> {}; lib.makeLibraryPath [ wayland vulkan-loader libglvnd ]'):/run/opengl-driver/lib"
./target/release/handwriting-input
```

`nix develop` sets that for you, so inside a dev shell plain `cargo run` works.

## Using it

The panel is a layer-surface strip; it **never grabs the keyboard**, so hardware
typing keeps working normally.

| Control | Effect |
|---|---|
| draw | adds a stroke; recognition runs when the pen is lifted |
| double tap the canvas | clear everything |
| candidate | types it into the focused app (and clears the canvas) |
| `×` | clear the canvas |
| `undo` | drop the last stroke and re-recognise what is left |
| `space` | type a space |
| `del` | Backspace in the focused app |
| `enter` | Enter in the focused app |
| `,` `.` `?` `!` | type punctuation without clearing the canvas ([`punctuation`]) |
| `quit` | close the panel (and the app) |

## How text reaches the app

`send = "auto"` (default) picks the first layer that can do the job:

1. **`zwp_input_method_v2`** — `commit_string` for whole words, and
   `delete_surrounding_text` for Backspace. Cleanest: no keys are simulated, and
   the receiving app sees plain text. Needs the focused app to be an IM client.
2. **`zwp_virtual_keyboard_v1`** — types through a self-generated XKB keymap: one
   keycode per character (printable ASCII, extended Latin for pl/de/fr/es/nordic/
   cs/tr/ro, common punctuation), with dynamically allocated, LRU-recycled slots
   for anything else (CJK included). Works with any app, but the app must accept
   synthetic keys.
3. **`zwlr_data_control_v1`** — puts the text on the clipboard as a last resort.

Special keys (Enter, Backspace) always go through the virtual keyboard, since the
input-method protocol has no keysym requests.

## Configuration

Everything lives in `~/.config/handwriting-input/config.toml` (or the path given
as the single CLI argument). The file is created with these defaults on first
run; unknown keys are rejected rather than silently ignored.

| key | default | meaning |
|---|---|---|
| `langs` | `["en"]` | language codes for the recogniser, e.g. `["pl", "en"]` |
| `server-url` | Google inputtools URL | recognition endpoint |
| `send` | `"auto"` | `auto`, `input-method`, `virtual`, `clipboard`, `none` |
| `num-candidates` | `8` | how many candidates to ask for / show |
| `theme` | `"auto"` | `"light"`, `"dark"`, `"auto"` (currently dark) |
| `position` | `"bottom"` | `"bottom"` = anchored strip, `"center"` = floating box |
| `layer` | `"top"` | `"top"` or `"overlay"` layer-shell layer |
| `height` | `300` | panel height in logical pixels |
| `stroke-width` | `6.0` | pen thickness |
| `key-delay-ms` | `0` | pause between simulated key presses (helps stubborn apps) |
| `punctuation` | `[",", ".", "?", "!"]` | quick-type buttons |
| `font-file` | unset | font to use for candidate glyphs (needed for CJK) |
| `auto-cjk-font` | `true` | look for a CJK font in the fontconfig directories |
| `multi-lang` | `"auto"` | `auto` = one combined request then fall back per language; `parallel` = one request per language |
| `timeout-secs` | `6` | per-request HTTP timeout |
| `use-context` | `true` | send the text before the caret to the API |
| `context-chars` | `40` | how much of it |
| `preedit-preview` | `false` | push the best candidate into the app as live preedit |
| `clear-after-send` | `true` | clear the canvas after a candidate is sent |

Example:

```toml
langs = ["pl", "en"]
num-candidates = 10
position = "center"          # a floating box instead of a bottom strip
height = 420
preedit-preview = true
```

## Fonts

egui's bundled fonts cover Latin, so the UI and Latin candidates always render.
CJK glyphs need a real CJK font: with `auto-cjk-font = true` the app walks the
`<dir>` entries of the system fontconfig configuration (this is how it finds fonts
on NixOS, where they live in the store). If nothing is found you will see boxes
for Chinese/Japanese/Korean candidates only, and the run prints a note — either
install a CJK font (`fonts.fonts = [pkgs.noto-fonts-cjk-sans]` on NixOS) or point
`font-file` straight at a `.ttf`/`.otf`/`.ttc`.

## Privacy

Recognition is **not local**. Every request sends the raw strokes, the language
list, and — with `use-context = true` — up to `context-chars` characters of text
sitting before the caret (the API's `pre_context`, which is what makes connected
handwriting accurate). Two guard rails:

* password and PIN fields are detected through the input-method `content_type`
  event and never contribute context;
* nothing is logged or written anywhere; strokes are discarded when you clear,
  undo or send.

Set `use-context = false` (or `context-chars = 0`) to send only the strokes.

## Development

```
src/main.rs            poll loop: Wayland socket + eventfd wake-ups
src/app/mod.rs         App state, egui context, Action enum
src/app/setup.rs       layer surface, scaling, binding the injection objects
src/app/render.rs      wgpu surface + egui-wgpu frame, commit
src/app/canvas.rs      strokes, double-tap, undo
src/app/recognize.rs   generation-counter guarded API round-trips
src/app/send.rs        which injection layer gets the text
src/app/fonts.rs       CJK fallback discovery
src/api.rs             Google request/response, language probing, merging
src/ink.rs             stroke model + the `ink` JSON wire format
src/inject/keymap.rs   generated XKB keymap for zwp_virtual_keyboard_v1
src/inject/mod.rs      the three injection layers
src/wayland/*          Dispatch impls: layer/outputs, seat/pointer/touch, IM/clipboard
```

Two things worth knowing before touching the injection code:

* `zwp_virtual_keyboard_v1.key()` carries **kernel** keycodes while the keymap
  text is written in **XKB** keycodes, offset by 8. Mixing them up is how
  Backspace ends up typing a comma; `keymap::evdev_of` is the single place where
  the conversion happens, and a unit test pins it down.
* a layer surface only receives its first `configure` after the `wl_surface` is
  committed, and `configure` is what tells us the width — so `after_globals`
  commits before waiting.

```sh
cargo test            # offline unit tests (keymap, wire format, API, config)
WAYLAND_DEBUG=1 ./target/release/handwriting-input   # protocol trace
```

## License

MIT

[egui]: https://github.com/emilk/egui
[wgpu]: https://github.com/gfx-rs/wgpu
[`punctuation`]: #configuration
