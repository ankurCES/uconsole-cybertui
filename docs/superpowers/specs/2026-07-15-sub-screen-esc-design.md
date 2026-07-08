# Sub-Screen Esc Ownership — Design

> One-line summary: In a layered TUI where modal / menu / launcher / sub-screen can all want Esc, give Esc to the **innermost** consumer. Sub-screens that have a meaningful "back" action (e.g. Files = "go up a folder") claim it; if the sub-screen doesn't want it, the launcher takes it.

**Date:** 2026-07-15
**Status:** Approved
**Supersedes:** the implicit convention in `handle_key` (main.rs:1088) that the launcher's `Esc → Region::ContentLeft` always wins, which makes Esc useless to any sub-screen.

---

## Problem

Today the dispatcher in `main::handle_key` gives `Esc` to the launcher before it reaches the screen's `on_key`:

```text
4. modal is open?         → consumes Esc
5. menu dropdown is open? → consumes Esc
6. region is Sidebar?     → Esc → move to content
7. region nav (B/Esc back) → consumes Esc  ← eats it
8. screen.on_key()        → never sees it
```

That means sub-screens like `Files` (which wants `Esc` to mean "go up a folder") can't implement it without first disabling the launcher. That's the wrong direction — sub-screens should win for their own context.

The user reported two concrete symptoms:

1. **Files screen** — they want `Esc` to go up a folder (same as `h`/`Left`), but Esc currently returns them to the launcher.
2. **Settings → Keys** (the recently-merged keymap feature) — the capture loop's `Esc` worked because it runs *above* the screen, but if any future sub-screen wants to use Esc for its own action, it can't.

## Rule

**Innermost consumer wins.** The dispatch order in `handle_key` is re-ordered so that the focused screen's `on_key` runs **before** the launcher's "Esc → leave" handler. If the screen returns `false` on Esc, control falls through to the launcher.

The full order after this change:

```text
1. Capture loop (keymap rebind)  → consumes its own Esc
2. Hardware shim (wm::keymap)    → may rewrite the keycode
3. User keymap resolve           → may rewrite the keycode
4. Modal is open?                → consumes its own Esc
5. Menu bar dropdown is open?    → consumes its own Esc
6. Region is Sidebar?            → Esc moves to content (unchanged)
7. **NEW: screen.on_key() gets Esc first**
8. If screen returned false on Esc → launcher focus
9. Global shortcut keys (Ctrl+M, etc.)
10. Region nav between content-left/right (Tab/BackTab)
```

**The only structural change** is swapping steps 7 and 8.

## Per-screen decisions

| Screen | Esc behavior | Implementation |
|---|---|---|
| **Files** | "Go up a folder" if there's a parent; fall through if at `/` | New arm in `Files::on_key` (mirrors `Char('h')` / `Left`). Returns `false` when at root so launcher takes over. |
| **Editor** | Close the editor (return to the previous screen / launcher) | New arm in `Editor::on_key`. |
| **Logs** | Dismiss the active filter; fall through if no filter is set | New arm in `Logs::on_key`. |
| **Settings → Keys** (sub-mode) | "Clear current binding" | Already implemented in Task 7 of the user-keymap plan. Unchanged. |
| **All other screens** (Bluetooth, Display, Audio, Network, Processes, Power, Services, Storage, System, Packages, City, LoRa, Settings main list) | Don't add an Esc arm — launcher takes Esc as today | No change. |

The choice to add an Esc arm is per-screen: only screens where Esc has an obvious "back" action get one. The rest keep the current behavior (Esc → launcher).

## `B` is a dedicated "back to launcher" shortcut

Today the codebase has an undertested fallback for `Char('b')` (a stub at main.rs:4111). The new behavior:

- `B` from `Region::ContentLeft | ContentRight` → `app.set_region(Region::Sidebar)`, return `false`.
- `B` from `Region::Sidebar` → no-op (you're already there).
- `B` is matched at step 9 (after the screen's on_key) so the screen can't accidentally claim it.

This gives the user **three distinct "back" affordances**, each with a clear role:

| Key | Role | Fires from |
|---|---|---|
| `Esc` | "Go back one level" — handled by the innermost context (sub-screen or launcher) | Anywhere |
| `B` (or `b`) | "Back to launcher" — explicit, always available from a content region | Content region only |
| `F10` / `Alt+F` | "Open the menu bar" — same as today | Anywhere |

## Help hint update

The footer help block in `main.rs:560-580` changes from:

```text
("esc", "leave to sidebar"),
```

to two lines:

```text
("esc", "back (sub-screen · or leave to launcher)"),
("b",   "back to launcher"),
```

The sub-mode hint at the bottom of the keymap editor (settings.rs:297) is unchanged — its hint is already specific to that mode.

## What does NOT change

- **Modal Esc-dismiss** (Help, Confirm, Input, Secret, Choice, Wizard, Progress, AuthFailure, ToastLog, CommandPalette). These run at step 4, above the screen.
- **Menu bar Esc-closes-dropdown** (main.rs:1433-1436). Runs at step 5, above the screen.
- **Hardware shim** (UConsole B-button → Esc). The screen will now see the B-button-Esc first; inside Files it goes up a folder, which is the expected behavior on a console.
- **User keymap rebind** (Settings → Keys). The capture loop runs at step 1, above the screen.
- **The sidebar's existing Esc-handler** at main.rs:1645 (`Esc if app.region == Region::Sidebar` → moves to content). Still runs at step 6.

## Testing

Three new test cases in `mod tests` of `main.rs`:

1. **`esc_in_files_goes_up_a_folder`** — `app.current = ScreenId::Files`, `app.region = Region::ContentLeft`, `app.files_cwd = /tmp/a/b`. Press Esc. Assert `app.files_cwd == /tmp/a`.

2. **`esc_at_filesystem_root_falls_through_to_launcher`** — same as above, but `app.files_cwd == /`. Press Esc. Assert `app.region == Region::Sidebar` (launcher took it).

3. **`esc_in_settings_keymap_submode_clears_binding`** — already covered by the keymap test suite (Task 7 of the user-keymap plan); add a focused regression test to `mod tests` so the integration with the new dispatch order is pinned.

The existing test `esc_from_sidebar_goes_to_content` (line 1645) still passes because the sidebar branch is unchanged.

## Adding the Esc arm to a screen

To keep the surface area small and consistent, follow this pattern when adding Esc to a new screen:

```rust
// In screens/<name>.rs, inside Screen::on_key:
KeyCode::Esc => {
    // <describe what "back" means for this screen>
    if /* can go back */ {
        // ... do the back action ...
        true            // claim the key — launcher doesn't get it
    } else {
        false           // fall through — launcher takes Esc
    }
}
```

Returning `true` from `on_key` claims the key (it won't reach the launcher). Returning `false` lets the launcher handle it.

## Open question — hardware B-button

The UConsole hardware shim maps the B-button to `Esc`. After this change, the B-button inside Files will "go up a folder" (because the screen claims it). This is probably the right behavior on a console layout, but the user may want the hardware shim to distinguish "B-button-Esc" from "physical Esc" if a console-focused build is in scope. For this design we treat them identically; if a need arises, the `wm::keymap` module is the place to add the distinction.
