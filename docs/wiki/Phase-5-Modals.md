# Phase 5 — Modal upgrade

Phase 5 ships a unified modal system with five modal types: **Secret**,
**Choice**, **Wizard**, **Progress**, and **AuthFailure**. Modals stack
on top of the WM, so a modal can be opened from inside any pane.

## Why a modal system

Before Phase 5, the TUI had a single hard-coded "help modal" and a
handful of inline prompts. Every new flow (Wi-Fi password, package
install confirmation, batch operation progress) reinvented the
prompt-rendering code. Phase 5 unifies these into a small set of
modal kinds that share:

- A common frame (title, body, footer).
- A common `ModalAction::Submit` / `ModalAction::Cancel` event
  channel.
- A common keystroke for `Esc` (cancel) and `Enter` (submit).
- A common width / height calculation based on the body content.

## The five modal kinds

### Secret

For password / passphrase / token input. Renders a `TextArea` with
`secure = true` so the input is masked. The submitted value never
appears in the toast log.

```
┌─ Wi-Fi password ─────────────────────────────┐
│                                              │
│  Network: home-5g                            │
│  Password: ●●●●●●●●●●                       │
│                                              │
│  [Enter] connect   [Esc] cancel              │
└──────────────────────────────────────────────┘
```

### Choice

For picking one of N options. Renders a vertical list with `j`/`k`
(or `↑`/`↓`) navigation and `Enter` to confirm. The options are
filtered by the current screen's `ChoiceProvider`.

```
┌─ Pick a service to restart ──────────────────┐
│                                              │
│  ▶ cyberdeck-web        active (running)     │
│    ssh                  active (running)     │
│    NetworkManager       active (running)     │
│    cron                 inactive (dead)      │
│                                              │
│  [Enter] restart   [Esc] cancel              │
└──────────────────────────────────────────────┘
```

### Wizard

For multi-step flows. Renders a numbered step indicator at the top,
the current step's body in the middle, and `[Back] [Next] [Cancel]`
at the bottom. Each step is a separate `WizardStep` impl that holds
its own state.

```
┌─ New Wi-Fi connection · step 2 of 4 ─────────┐
│                                              │
│  ① SSID ✓                                    │
│  ▶ Password                                  │
│    Security                                  │
│    Confirm                                   │
│                                              │
│  Enter the Wi-Fi password:                   │
│  ●●●●●●●●●●                                 │
│                                              │
│  [Back] [Next] [Cancel]                      │
└──────────────────────────────────────────────┘
```

### Progress

For long-running operations. Renders a progress bar with a label, a
percentage, and an optional cancel button. The progress events come
through the action channel from the background task.

```
┌─ Upgrading packages ─────────────────────────┐
│                                              │
│  Running `apt upgrade -y`                    │
│                                              │
│  [████████████░░░░░░░] 62%  (124 / 200 pkgs) │
│                                              │
│  [Esc] cancel                                │
└──────────────────────────────────────────────┘
```

### AuthFailure

For surfacing permission failures cleanly. Renders the error message
in red, the suggested remediation in plain text, and a single
`[Acknowledge]` button. There is no submit — the modal is just a
banner that blocks input until acknowledged.

```
┌─ ⚠ Permission denied ────────────────────────┐
│                                              │
│  `sudo -n /usr/bin/systemctl restart ssh`    │
│  returned:                                   │
│                                              │
│  sorry, a password is required to run sudo. │
│                                              │
│  Re-run the installer with `--full` to write │
│  the NOPASSWD sudoers fragment, or set up    │
│  sudo manually (see README § Privilege      │
│  model).                                     │
│                                              │
│  [Enter] acknowledge                         │
└──────────────────────────────────────────────┘
```

## Module layout

```
crates/tui/src/modal/
├── mod.rs           # ModalStack, ModalAction
├── secret.rs        # SecretModal
├── choice.rs        # ChoiceModal
├── wizard.rs        # WizardModal, WizardStep
├── progress.rs      # ProgressModal
└── auth_failure.rs  # AuthFailureModal
```

## ModalStack

`ModalStack` is a `Vec<Modal>` with a single "current" pointer. Pushing
a new modal renders on top; popping the current modal returns to the
previous one. The stack is at most 3 deep — pushing past 3 toasts
`modal stack full (3)` and is otherwise a no-op.

```rust
pub enum Modal {
    Secret(SecretModal),
    Choice(ChoiceModal),
    Wizard(WizardModal),
    Progress(ProgressModal),
    AuthFailure(AuthFailureModal),
}

pub enum ModalAction {
    Submit { id: ModalId, value: ModalValue },
    Cancel { id: ModalId },
    StepBack { id: ModalId },
    StepNext { id: ModalId },
    Tick { id: ModalId, progress: f32 },
    Acknowledge { id: ModalId },
}
```

`ModalValue` is an enum that captures the submit payload for each
modal kind:

```rust
pub enum ModalValue {
    Secret(String),
    Choice(ScreenId),       // or whatever the choice was about
    Wizard(WizardState),
    Unit,                   // Progress, AuthFailure don't submit
}
```

## Rendering

The modal stack renders after the WM (so the WM panes are visible
underneath) but with a semi-transparent dim layer. The current modal
is rendered full-opacity; the rest of the stack is dim.

A modal that doesn't fit on the screen is clipped (with a "…"
indicator at the top / bottom). Most modals are small enough that
this never happens.

## Tests

Phase 5 modal tests are pure unit tests on the modal state machines
(no PTY, no ANSI). They live under `#[cfg(test)] mod tests` in each
modal file and use `cargo check -p cyberdeck-tui --all-targets` for
verification. The full suite (`make test ARGS='-p cyberdeck-tui --bin
cyberdeck-tui'`) finishes in ~1 s and does not exercise any modal
PTY-touching path.
