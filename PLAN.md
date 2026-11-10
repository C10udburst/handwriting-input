# PLAN.md — Handwriting input for Linux/Wayland

Goal: rewrite of `Handwriting-keyboard-for-Linux` (cloned to `/tmp/handwriting`) as a
**single, pure-Rust application** using **egui**, keeping the **same Google handwriting API**,
adding **multi-language support**, targeting **Wayland only**, and replacing the
xdotool/ydotool + clipboard + `ctrl+v` injection with the **same Wayland protocols a modern
OSK uses** (virtual keyboard / text-input), so text injection is reliable and never depends
on window focus or modifier key shortcuts.

---

## 1. Why rewrite (problems in the current app)

| Current (v2.1, Tauri)                                                          | Problem                                                                                                                                                                              |
| ------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Tauri 2 + TypeScript/Vite frontend, `handwriting.js` in a `<canvas>`           | Two build systems (npm + cargo), webview overhead, JS glue for everything                                                                                                            |
| Injection via `xdotool key ctrl+v` / `ydotool` after copying to clipboard      | Clipboard roundtrip is flaky; `ctrl+v` requires the target app to accept the shortcut; **breaks when our own window is focused** (current app tries `alt+Tab` first as a workaround) |
| `ydotool` needs `ydotoold` running as root; pastes via clipboard, doesn't type | Not a real input path, extra daemon, X11/legacy                                                                                                                                      |
| Focus dance (`alt+Tab`, `alt+Tab BackSpace`, window `isFocused()` checks)      | Fragile race conditions, visible focus flicker                                                                                                                                       |
| Single `--language` arg                                                        | Multi-language handwriting not supported                                                                                                                                             |
| X11-oriented (xdotool, `fly-to-bottom` positioning quirks on Wayland)          | User only cares about Wayland                                                                                                                                                        |

**Key insight for the fix:** our window will be a **layer-shell surface** with
`keyboard_interactivity = None`. On wlroots-style compositors that means we *never* take
keyboard focus, so the last focused application keeps it. Injecting keys through a
**virtual keyboard** then lands in the app the user was typing in — no `alt+Tab`, no
clipboard, no shortcut, no focus handling at all. Focus-stoppage bugs disappear by
construction.

---

## 2. Architecture overview

```
┌─────────────────────────────────────────────────────────────┐
│ single binary: handwriting-input (pure Rust, no npm/webview)│
│                                                             │
│  cli.rs ──► Config { langs: Vec<Bcp47>, theme, send mode… } │
│                                                             │
│  app.rs  ── egui::App (frame loop)                          │
│    ├─ ink.rs      stroke model + painting                   │
│    ├─ api.rs      Google IME client (multi-language)        │
│    ├─ theme.rs    light/dark egui visuals                   │
│    └─ inject.rs   text injection (see §4)                   │
│                                                             │
│  wayland/mod.rs   raw wayland-client connection, globals,  │
│                   outputs (zxdg_output_manager_v1),         │
│                   layer surface (zwlr_layer_shell_v1),      │
│                   pointer/touch → egui events               │
└─────────────────────────────────────────────────────────────┘
        │ Wayland (wayland-client, own event queue)
        ▼
   compositor: Sway / Hyprland / niri / river / …
```

One process, one Wayland connection, one event loop. Rendering via `egui-wgpu` on a
layer-shell surface.

---

## 3. Windowing & rendering (egui instead of Tauri)

### Verified constraints (Nov 2026)
- `egui` / `eframe` **0.37.0** is current; `wgpu` **30.x**; `winit` **0.30.13**.
- **wgpu 30 dropped `RawWindowHandle`/`WaylandSurface` targets** — `wgpu::Instance::create_surface`
  now only accepts a `wl_surface` pointer + display handle. So the "hand
  `WindowHandle` to wgpu" path still works, but **any `winit`-based `eframe` window cannot be
  a layer-shell surface** (winit's window role is xdg-shell only).
- `smithay-client-toolkit` **0.21.1** has `shell::wlr_layer` with
  `LayerShell::create_layer_surface`, `set_anchor/set_size/set_layer`,
  **`KeyboardInteractivity::{None, Exclusive, OnDemand}`** and
  `LayerSurfaceData::layer_surface()` — this is the layer-shell implementation we need.
- Existing reference implementation of exactly this pattern (`egui` + raw Wayland layer
  surface + `egui-wgpu`): the `lockrs` crate (lockscreen, same problem class) and
  `egui-pop-wayland` research notes.

### Decision: **Option A — own Wayland event loop, no winit**
Run `wayland-client` ourselves; create a `zwlr_layer_shell_v1` surface anchored to the
bottom of the output; render with `egui-wgpu::Renderer` into the `wl_surface` via wgpu's
raw-pointer surface target; translate Wayland `pointer`/`touch` events into
`egui` input events ourselves.

*Why not winit/eframe + layer-shell hacks:* winit owns its connection and event queue and
always creates an xdg-shell window; layer-shell needs a different role on the *same*
surface. Mixing SCTK's queue with winit's queue on one connection is possible but is
exactly the fragile plumbing we're trying to eliminate.

*(Fallback **Option B** if the spike fails: `layershellev` 0.19.1 — "extra shell binding
for layershell with winit-like eventloop" — + stock `eframe`. Keep as plan B, but it adds
a dependency whose API we haven't validated; Option A matches `lockrs` proven design.)*

### Layer surface setup
- `anchor = Bottom | Left | Right`, `exclusive_zone = 0`, layer `Top` (or `Overlay`),
  `keyboard_interactivity = None`.
- Size: full screen width × ~300 logical px, positioned at the bottom (this is the
  `--fly-to-bottom` behavior made *default and correct* — layer-shell anchors do it natively,
  fixing the Wayland note in the current README).
- Output geometry via **`zxdg_output_manager_v1`** (SCTK's `OutputManager` binds it
  automatically) → `logical_position` / `logical_size` for bottom anchoring and for
  detecting output changes (scale/position) to re-anchor.
- Fractional scale: bind `wp_fractional_scale_v1` + `wp_viewporter` for crisp rendering
  (SCTK has `wp_viewporter` support; fractional-scale is in `wayland-protocols` staging).

---

## 4. Text injection (the core improvement)

### Primary: `zwp_input_method_v2` — whole-string commits (user decision 2026-11-10, like Stevia OSK)
- Present in **`wayland-protocols-misc` 0.3.12** as `zwp_input_method_v2` (XML verified).
- We become the seat's input-method client: `get_input_method(seat)` → receive
  `activate` / `deactivate` / `surrounding_text{text, cursor, anchor}` / `content_type` /
  `done` (done-count == `commit(serial)` serial).
- Insert a recognized candidate with **one `commit_string(text)` + `commit(serial)`** —
  no keymap, no per-key simulation, arbitrary Unicode including CJK works natively.
- `set_preedit_string(text, cursor_begin, cursor_end)` → optional *live suggestion* in the
  target field, replaced when the user picks a candidate (`preedit-preview` config, off
  by default). `delete_surrounding_text(before, after)` → context-aware backspace.
- **We do NOT `grab_keyboard`** — hardware keyboard keeps working normally; keys only
  reach us if we grab (we don't need them).
- Caveats handled: only one IM per seat (compositor sends `unavailable` — then fall back
  to virtual keyboard); IM only routes to apps that enable text-input (foot, XWayland
  apps are covered by the virtual-keyboard fallback when `!active`).

### Fallback 1: `zwp_virtual_keyboard_v1` (wlr-virtual-keyboard-unstable-v1)
- Available in **`wayland-protocols-misc` 0.3.12** as `zwp_virtual_keyboard_v1`.
- Flow: bind manager → `create_virtual_keyboard(seat)` → **`keymap(memfd, XKB format)`**
  → `key(time, evdev_keycode, Press/Release)` / `modifiers(...)`.
- Used when: input-method unavailable/inactive, and always for **Enter/Tab keysyms**
  (input-method has no keysym request; only the keyboard grab does, and we don't grab).
- **No clipboard, no `ctrl+v`, no `alt+Tab`.** The compositor routes injected keys to the
  focused app — thanks to `KeyboardInteractivity::None` our layer surface never steals it.

### Typing arbitrary Unicode (Latin-first; CJK as bonus)
**User priority: Latin + extended Latin (pl, de, fr, es, nordic…), CJK only as a bonus.**
- Ship a **self-contained XKB keymap text** (no `include`s — we cannot rely on XKB data
  paths on NixOS): US-ish codes for ASCII (2 levels via Shift) plus **direct keysyms for
  ~90 extended-Latin/punctuation chars assigned to free keycodes (150–247)**. Any Latin
  script text types with **zero keymap regeneration**.
- Characters outside the base table (CJK ideographs, rare symbols) → dynamic regeneration:
  assign the string's uncovered chars to remaining free keycodes, re-issue `keymap()`,
  press/release; batches of ≤ free-slot count.
The virtual keyboard carries **its own keymap**, and the compositor computes keysyms from
*our* keymap when routing to the focused app. So we can type any character:
- `keymap.rs`: build an XKB keymap **text** (memfd) in which we assign keycodes to the
  keysyms we need. Strategy:
  1. ASCII/Latin: standard `us` layout keycodes for printable ASCII.
  2. Non-Latin (e.g. Chinese, punctuation like `，。`): dynamically assign free keycodes to
     the required keysyms (`ks = codepoint` for BMP, `0x10000000 | codepoint` for the
     `0x1xxxx` range), regenerate the keymap memfd, `keymap()` again, then press/release.
- Cache generated keymaps per session; regenerate only when a new unseen char is typed.
- Verify in milestone M2 against GTK4, Qt6, Firefox (all compute keysyms from the
  keymap they receive).

### Fallback 2: clipboard (`send = "clipboard"`)
Copy the candidate via **ext-data-control** (`wayland-protocols-wlr` `data_control::v1`);
the user pastes themselves. For compositors exposing neither IM nor virtual keyboard
(GNOME/KDE). `send = "none"`: clipboard copy on click, no typing ever.

### Selection order (`send = "auto"`, the default)
`input-method (active) → virtual keyboard → clipboard`. Config pins any layer explicitly.

### Buttons
`Enter` → virtual-keyboard keysym (or no-op + hint if no vk). `Backspace` →
IM `delete_surrounding_text(len_of_prev_char, 0)` when active with surrounding support,
else vk keysym. Punctuation chips → same text path as candidates.

### Pre-context for recognition (user decision 2026-11-10)
With `zwp_input_method_v2` we receive `surrounding_text` + `cursor`. The last
`context-chars` (default 40) characters before the caret are sent to the Google API as
context. The endpoint is undocumented; we send Chrome-OS-style keys —
`pre_edit_string` (+ `use_pre_space`, `use_bigram_model`, `sequence_id`,
`starting_language`) — which this endpoint really receives from ChromeOS; unknown fields
are ignored by Google's parser. `use-context` config toggle (default on) + curl
verification in M1/M3.

---

## 5. Handwriting recognition (Google API, multi-language)

Keep the **exact same endpoint** as the current app:
```
POST https://www.google.com/inputtools/request?ime=handwriting&app=mobilesearch&cs=1&oe=UTF-8
Content-Type: application/json
```
Body shape (from `handwriting.canvas.js`):
```json
{
  "options": "enable_pre_space",
  "requests": [{
    "writing_guide": { "writing_area_width": W, "writing_area_height": H },
    "ink": [[[x,y,t?], ...], ...],     // strokes: [x[], y[], z[]] triples in JS: trace = [[xs],[ys],[]] per stroke
    "language": "zh-TW"                 // or array (see below)
  }]
}
```
Response: `[err, [ [ [score, text, ...], ... ] ]]` — candidates in `response[1][0][1]`;
length-1 response is an error payload.

### Multi-language strategy (per user requirement)
`--lang` is repeatable (`-l zh-CN -l en -l de`), default `zh-CN`.

1. **Probe**: first try a **single combined request** with `"language": ["zh-CN", "en", ...]`
   (some IME endpoints accept arrays). Cache the verdict for the process lifetime.
2. **Fallback (required path)**: if the API rejects/ignores the array, issue **one POST per
   language in parallel** (`tokio`/`futures::join_all` or a simple thread pool —
   prefer `std` threads, no async runtime needed for 2–5 requests).
3. **Merge**: round-robin interleave candidates from each language, dedupe identical
   strings, tag each chip with its language (small colored label) so the user sees which
   script the candidate came from. `num_of_return` (default 8) applies per language
   before merging.
4. Errors: per-language errors are shown inline (red, as today) without killing the
   other languages' results; total failure shows the current error panel style.

Ink is captured in Rust (`Vec<Stroke>`), serialized to the JSON `ink` format directly —
no `handwriting.js` dependency; drop the JS file entirely.

---

## 6. Configuration (user decision 2026-11-10)

**All options live in a TOML config file; the only CLI argument is the config path.**
Also: **Latin is the priority — extended Latin first, CJK is a nice-to-have.**

```
handwriting-input [CONFIG]        # positional; or --config <path>
                                  # default: ~/.config/handwriting-input/config.toml
```

`config.toml` keys (serde, defaults in `config.rs`):

```toml
langs = ["en", "zh-CN"]      # recognition languages (first = primary)
server_url = "https://www.google.com/inputtools/request?ime=handwriting&app=mobilesearch&cs=1&oe=UTF-8"
send = "virtual"             # virtual | clipboard | none (auto-fallback to clipboard if virtual unsupported)
num_candidates = 8
theme = "auto"               # light | dark | auto
position = "bottom"          # bottom | center
layer = "top"                # top | overlay
height = 300                 # panel height, logical px
stroke_width = 6.0
key_delay_ms = 0             # delay between synthesized keys
punctuation = [",", ".", "?", "!", "'"]  # chips next to candidates
```

**Removed** (vs old app): `--use-ydotool`, `--use-shift`, `--return-keyboard`,
`--dark-theme`, `--fly-to-bottom`, `--language` — all obsolete or replaced by config keys.
`clap` 4 derive is kept only for the one path argument + `--version`/`--help`.

---

## 7. UI (egui)

Single `egui::Window`/panel on the layer surface, bottom-anchored:

```
┌────────────────────────────────────────────────────────────┐
│ [候选1 zh] [候选2 en] [候选3 zh] …      (candidate strip) ⌫  │
│ ┌────────────────────────────────────────────────────────┐ │
│ │            canvas: draw strokes with pointer/touch     │ │
│ └────────────────────────────────────────────────────────┘ │
└────────────────────────────────────────────────────────────┘
```

- ⌫ clears the canvas
- Canvas: `Stroke` list painted with `Shape::Line`/`Circle` per segment; clear on
  double-click; auto-recognize on pointer release (same UX as now).
- Candidate chips: click → inject text + clear canvas (same as `choseWord`).
- Dark/light theme via `egui::Visuals`, honoring `--theme`.
- Error states rendered inline in red (as current `errorMessage`).

---

## 8. Project layout

```
handwriting-input/
├── PLAN.md
├── Cargo.toml
├── Cargo.lock            # committed; flake uses cargoLock
├── flake.nix             # §9
├── README.md
├── src/
│   ├── main.rs           # CLI parse (config path only), Wayland connect, run loop
│   ├── config.rs         # TOML config (all options), clap path arg
│   ├── app.rs            # egui App: update() frame, state machine (Idle/Recognizing/Results)
│   ├── ink.rs            # Stroke model, serialization to Google `ink` JSON
│   ├── api.rs            # Google client: single-lang + multi-lang probe/merge
│   ├── theme.rs          # light/dark egui visuals
│   ├── wayland/
│   │   ├── mod.rs        # Connection, GlobalState (registry, outputs, seats, layers)
│   │   ├── shell.rs      # layer-surface create/configure/anchor, scale, viewporter
│   │   └── input.rs      # pointer/touch → egui events
│   └── inject/
│       ├── mod.rs        # Injector trait, mode selection + capability probe
│       ├── virtual_kb.rs # zwp_virtual_keyboard_v1
│       ├── keymap.rs     # dynamic XKB keymap generation (memfd)
│       └── clipboard.rs  # fallback: wl_data_device / clipboard + ctrl+v
└── tests/
    └── api.rs            # offline request/merge tests (mock HTTP)
```

Dependencies (verified against fetched sources, Nov 2026): `egui 0.37`, `egui-wgpu 0.37`
(feature `wayland`, **no winit**), `wgpu 30` (vulkan+gles; **`wgpu::rwh` re-export +
`SurfaceTargetUnsafe::RawHandle` still exist** — the earlier "dropped RawWindowHandle" note
in this PLAN was wrong; raw display+window handles are still accepted), `wayland-client
0.31` + `wayland-backend` **`dlopen`** feature (no pkg-config / wayland headers at build
time), `wayland-protocols 0.32.13` (`client`,`unstable`,`staging`: `xdg::xdg_output::zv1`,
`wp::fractional_scale::v1`, `wp::viewporter::v1`), `wayland-protocols-wlr 0.3.12`
(`layer_shell::v1`, **`data_control::v1` for clipboard fallback**),
`wayland-protocols-misc 0.3.12` (`zwp_virtual_keyboard_v1`), `clap 4` derive,
`serde`/`serde_json`/`toml`, `ureq 3` (blocking HTTPS, bundled webpki roots — hermetic on
NixOS), `libc` (memfd_create, eventfd). **Dropped:** smithay-client-toolkit, the
`xkbcommon` crate (keymap text is generated by hand), `memfd` crate.

---

## 9. flake.nix plan

Pure Rust project → **no node/npm in the build at all**. Use
`rustPlatform.buildRustPackage` with the committed `Cargo.lock`.

```nix
{
  description = "Handwriting input keyboard for Linux (Wayland, egui)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        # Rust pinned per rustPlatform of this nixpkgs rev
        mkHandwriting = pkgs.rustPlatform.buildRustPackage {
          pname = "handwriting-input";
          version = "0.1.0";
          src = ./.;                       # clean source filter via lib.cleanSource

          cargoLock.lockFile = ./Cargo.lock;
          # only needed if we add git deps:
          # cargoLock.outputHashes = { ... };

          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [
            pkgs.wayland                   # libwayland (wayland-sys "system" or dlopen)
            pkgs.libxkbcommon              # xkbcommon for keymap generation
          ];

          # wgpu loads Vulkan/EGL at runtime via dlopen — no build-time need,
          # but document runtime deps in meta + README.
          doCheck = true;                  # cargo test (offline api tests)

          meta = {
            description = "Handwriting input keyboard using Google handwriting API on Wayland";
            license = pkgs.lib.licenses.mit;
            mainProgram = "handwriting-input";
          };
        };
      in {
        packages = {
          default = mkHandwriting;
          handwriting-input = mkHandwriting;
        };

        apps.default = {
          type = "app";
          program = "${mkHandwriting}/bin/handwriting-input";
        };

        devShells.default = pkgs.mkShell {
          packages = [
            pkgs.rustc pkgs.cargo pkgs.rustfmt pkgs.clippy
            pkgs.pkg-config pkgs.wayland pkgs.libxkbcommon
            pkgs.rust-analyzer
          ];
          env.RUST_LOG = "info";
        };

        checks = {
          build = self.packages.${system}.default;
          clippy = pkgs.runCommand "clippy" { buildInputs = [ pkgs.cargo pkgs.rustc ]; } ''
            cd ${./.} && cargo clippy --all-targets -- -D warnings
          '';
        };
      });
}
```

**flake.nix notes / open items**
- Pin `nixpkgs` rev once the Rust toolchain version is settled (wgpu 30 needs a recent
  rustc ≥1.82-ish; `nixos-unstable` is fine).
- Wayland linkage: default `wayland-client` feature is `dlopen` — either add
  `features = ["system"]` for a hard link (then `buildInputs = [wayland]` is required)
  or leave dlopen and rely on `LD_LIBRARY_PATH` from the Nix wrapper; prefer
  `system` feature for hermeticity.
- Runtime deps (documented, not in build inputs): GPU drivers for wgpu
  (`libGL`/Vulkan ICD), `wayland` socket obviously.
- Optional outputs to add later: `nixosModules.handwriting-input` (a
  `programs.handwriting-input` module), `homeManager` example, and a
  `pkgs.writeShellApplication` wrapper that launches with default `-l zh-CN`.
- CI: `nix flake check` + `nix build` for the package; `nix run` smoke test is manual
  (needs a Wayland session).

---

## 10. Milestones

| #      | Deliverable                                                                                        | Exit criteria                                                                    |
| ------ | -------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| **M0** | *Spike (go/no-go for Option A)*: layer-shell surface + `egui-wgpu` render + pointer events on Sway | Window shows an egui frame anchored bottom; click hits egui                      |
| **M1** | Ink capture + Google API client (single lang) headless                                             | `cargo test`: strokes → valid request JSON; mocked response parsed               |
| **M2** | Virtual keyboard injection + dynamic keymap                                                        | Type `"你好, world!"` into `foot`/`gtk4-demo` via CLI harness; no focus juggling |
| **M3** | Multi-language: probe array, parallel per-lang fallback, merge/tag UI                              | `-l zh-CN -l en` returns tagged candidates from both                             |
| **M4** | Full UI (canvas, chips, punctuation, theme), CLI, `--send` fallback probe                          | End-to-end: draw → recognize → click → text appears in focused app               |
| **M5** | `flake.nix` + packaging + README                                                                   | `nix build` produces binary; `nix run` launches app                              |
| **M6** | Polish: output changes/scale, error UX, clipboard fallback, release 0.1                            | Works on Sway/Hyprland/niri; documented compositor matrix                        |

---

## 11. Risks & unknowns

1. **Compositor support matrix** — `zwp_virtual_keyboard_v1` is wlroots-family
   (Sway, Hyprland, niri, river, Waybox…). GNOME/Mutter and KDE do **not** expose it.
   → mitigation: registry probe + `--send=clipboard` fallback (M4); document matrix in README.
2. **egui + wgpu on a layer-shell surface** — unproven *in this repo*; wgpu 30's raw-pointer
   surface target makes it possible, `lockrs` proves the pattern, but M0 is a real
   gate. If it fails → Option B (`layershellev` + eframe).
3. **Keymap regeneration quirks** — some compositors/apps cache keysym tables; re-issuing
   `keymap()` mid-session must be tested (M2); worst case, pre-generate a large static
   keymap covering Latin + CJK ranges at startup instead of regenerating.
4. **Google API stability** — endpoint is undocumented; keep `--server-url` escape hatch
   (Digital ink recognition Server compatible). Multi-language array probe is empirical.
5. **Pointer-only input** — no keyboard focus means no egui keyboard shortcuts/text
   fields in our window; acceptable (we don't need typing input in our own UI), but note
   if we ever add a settings panel (use on-canvas buttons or take `Exclusive` interactivity
   temporarily).
