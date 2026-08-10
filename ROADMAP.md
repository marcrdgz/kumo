# 🗺️ Kumo Roadmap

The long-term goal is that **every aspect of kumo is configurable by you**:
colors, keybindings, the leader key, the layout of the status bar and sidebar,
the AI panes, and more. The current build ships with a sensible fixed
configuration, and the pieces below are planned — **not yet implemented**.

## 🎨 Theme & colors

Colors are currently hard-coded constants (`src/app.rs`, `src/app/ui.rs`).
Planned: full color palette customization (Catppuccin-style schemes, per-scheme
backgrounds, status-dot colors, borders), a configurable theme, and light/dark
variants.

## ⌨️ Keybindings

The leader dispatch (`leader_command` in `src/app.rs`) and mouse actions are
hard-coded. Planned: remap any binding (splits, focus, sessions, zoom, sidebar,
…), custom leader keys, and per-mode keymaps.

✅ **Shipped** — `leader+?` opens a browsable keybind showcase, and the
leader-mode status-bar hint is generated from the same table
(`src/app/bindings.rs`), so the two never drift.

## ⚙️ Config file

Today the unique file `~/.config/kumo/config` covers the AI CLI command and the
default shell. Planned: theme, keymaps, leader key, status-bar layout, and pane
behavior.

## 🧭 Layout & chrome

Planned: toggle/order sidebar sections, customize the status bar (branch,
session, agent status, hostname, clock), pane titles, and border styling.

## 🧩 Plugins / extensions

Planned: a plugin system so the community can add custom commands, widgets, and
integrations without forking kumo.

---

Until then, kumo stays opinionated: it picks good defaults for you so it just
works, and you can follow the roadmap above as the knobs land. 🔧
