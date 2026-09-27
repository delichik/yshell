# YShell Phase 1 Implementation Design

This document defines the first implementation phase for turning YShell from an interactive mockup into a real usable product slice.

This phase is intentionally narrow:
- One real SSH session
- One real terminal pane
- One real connect and disconnect lifecycle
- One minimal saved-session flow

It does not attempt to close the full product surface in one pass.

## 1. Goal

Phase 1 is successful when a user can:
- Enter a Quick Connect target
- Establish a real SSH connection
- Open one interactive shell with PTY
- See remote output in the terminal area
- Type input into that terminal
- Resize the terminal session
- Disconnect and reconnect
- Save a session and reopen it later

This is the smallest vertical slice that makes the product meaningfully usable and aligns with the documented Milestone 1 and release criteria.

## 2. Why This Phase Comes First

Current evidence shows:
- Config parsing and storage are already real
- App shell startup is real
- Terminal, session, SSH, and SFTP layers exist mostly as models or placeholders

The strongest existing foundation is:
- `crates/yshell-config`
- `crates/yshell-logging`
- parts of `crates/yshell-core`

The weakest but most critical gap is:
- no real end-to-end session pipeline from UI to SSH to terminal to UI

Because of that, Phase 1 must optimize for product closure, not breadth:
- do not add more mock UI
- do not prioritize proxy/tunnel/advanced commands yet
- do not build full SFTP before SSH terminal is real

## 3. Non-Goals For Phase 1

The following stay out of scope unless needed to support the core vertical slice:
- multi-pane split behavior
- advanced tab management
- proxy runtime
- port forwarding runtime
- remote file editing
- batch quick commands
- Compose Pane
- Send Key Input To
- full logging policy inheritance UI
- full theme and appearance controls
- advanced host-key management UI

These can remain placeholder-backed while the SSH terminal slice is being closed.

## 4. Required Architecture Shift

The main architectural problem is not just missing functionality. It is that the app shell is not connected to the core runtime.

Today:
- `yshell-app` does not depend on `yshell-core`, `yshell-ssh`, `yshell-sftp`, or `yshell-terminal`
- `AppState` stores `AppViewModel`, but runtime UI updates bypass it
- UI callbacks mutate `MainWindow` state directly
- `CoreCommandDispatcher` records commands but does not drive live sessions

Phase 1 must introduce a real runtime bridge inside `yshell-app`.

## 5. Proposed Runtime Shape

### 5.1 New Ownership Boundary

`yshell-app` becomes the runtime composition crate.

It should own:
- config loading
- UI startup
- app runtime state
- session controller lifecycle
- projection from backend state into UI properties

It should not own:
- SSH protocol implementation details
- terminal parsing internals
- config schema rules

### 5.2 New App Runtime Structure

Introduce an application runtime object in `crates/yshell-app`, conceptually similar to:

```text
AppRuntime
  config_store
  command_dispatcher
  session_runtime_map
  ui_projection
```

Where:

`config_store`
- loads and saves session config

`command_dispatcher`
- remains the app-to-core command entry point

`session_runtime_map`
- tracks one runtime object per open session
- stores SSH session handle, terminal model, and UI-facing status

`ui_projection`
- translates runtime state into `MainWindow` properties

## 6. Phase 1 Module Plan

### 6.1 `crates/yshell-app`

Add direct dependencies on:
- `yshell-core`
- `yshell-ssh`
- `yshell-terminal`

Phase 1 changes:
- create `runtime.rs` or equivalent
- create `session_runtime.rs` for one live session lifecycle
- move direct callback logic in `bootstrap.rs` into runtime methods
- wire `Quick Connect` to open a real session instead of only validating input
- load saved sessions into the sidebar state

### 6.2 `crates/yshell-core`

Keep `SessionManager` and `SessionCommand`, but extend the layer from passive model to useful coordination contract.

Phase 1 changes:
- preserve `OpenSession`, `CloseSession`, `SetSessionState`
- treat `SendTerminalInput` and `ResizeTerminal` as real control inputs from app runtime
- add or clarify events needed by app runtime for state projection

This crate should still avoid directly depending on Slint.

### 6.3 `crates/yshell-ssh`

This crate must stop defaulting to fake transport for product runtime.

Phase 1 changes:
- keep fake adapter for tests
- introduce a real adapter implementation behind the same conceptual boundary
- define shell-session operations clearly enough for app runtime to use

Minimum runtime responsibilities:
- connect
- authenticate
- open PTY shell
- read remote bytes
- write input bytes
- resize PTY
- disconnect

The fake adapter remains useful for deterministic tests, but the product path must opt into the real adapter.

### 6.4 `crates/yshell-terminal`

This crate already has useful core logic.

Phase 1 changes:
- keep parser and grid model local to the crate
- define one app-facing way to feed remote bytes into the grid
- define one app-facing way to turn UI keystrokes into session input bytes

Important constraint:
- do not couple terminal rendering logic to network code

### 6.5 `yshell-ui` and `ui/main_window.slint`

Phase 1 should not attempt full UI architecture purity.

Instead:
- keep `MainWindow` as the live rendered surface
- replace static placeholder text with runtime-driven session state
- make sidebar, tab label, and terminal content reflect one active session
- defer more complete component composition until the runtime slice is working

This is a deliberate tradeoff:
- real behavior first
- view-model cleanup second

## 7. UI Scope For Phase 1

### 7.1 Quick Connect

Current:
- validates text only

Target:
- parse target
- create in-memory session record
- attempt real connect
- show connecting, connected, or error state

### 7.2 Sidebar

Current:
- static labels and config path text

Target:
- show at least:
  - one active quick-connect session
  - saved sessions loaded from config
- allow selecting one saved session to connect

### 7.3 Tab Area

Current:
- one static disconnected button

Target:
- show one active tab with state text:
  - connecting
  - connected
  - disconnected
  - error

### 7.4 Terminal Area

Current:
- descriptive placeholder text

Target:
- render terminal transcript text from the terminal model
- display remote output
- accept input

For this phase, a simple text-based projection is acceptable if it is real and interactive.

It does not need to become a fully polished terminal renderer yet.

### 7.5 Session Save Flow

Current:
- no end-to-end UI persistence flow

Target:
- allow saving a Quick Connect target into config
- show it in the session list on next load

This can start with a minimal action rather than the full session editor.

## 8. Implementation Sequence

### Step 1: Compose the runtime in `yshell-app`

Deliverables:
- app runtime object
- real dependencies between `yshell-app` and `yshell-core` / `yshell-ssh` / `yshell-terminal`
- callback routing through runtime methods

Why first:
- everything else depends on the runtime bridge existing

### Step 2: Define a real session runtime contract

Deliverables:
- one session runtime struct
- lifecycle states
- terminal byte ingestion
- input and resize handling

Why second:
- it becomes the stable unit that app code can manage

### Step 3: Replace Quick Connect mock behavior

Deliverables:
- parse target
- connect through runtime
- surface state in UI

Why third:
- this is the first user-visible closed loop

### Step 4: Project terminal output into the UI

Deliverables:
- remote bytes feed terminal model
- terminal model projects visible text to UI
- UI sends input back to session runtime

Why fourth:
- this creates actual product usefulness

### Step 5: Load and save minimal sessions

Deliverables:
- config load into sidebar
- save from quick-connect or equivalent minimal action
- reopen saved session

Why fifth:
- this closes the first persistence loop without waiting for the full session editor

## 9. Testing Strategy Without Blocking The Design

This repository currently cannot be validated locally in this environment because Rust tooling is not installed.

That does not block design or implementation preparation.

Phase 1 should still be written to support later verification through:
- unit tests around runtime state projection
- adapter tests for fake and real SSH boundaries
- integration tests for connection lifecycle
- product smoke tests that stop relying only on placeholder servers

Until toolchain validation is available, all implementation work should clearly distinguish:
- code written
- code reviewed statically
- code executed and verified

## 10. Risks

### Risk 1: Building too much UI before backend closure

Mitigation:
- UI changes must serve the one real session vertical slice only

### Risk 2: Coupling Slint directly to transport details

Mitigation:
- keep runtime translation in `yshell-app`
- keep protocol logic in `yshell-ssh`

### Risk 3: Replacing fake infrastructure too early

Mitigation:
- keep fake adapters for deterministic tests
- add real runtime path alongside them

### Risk 4: Trying to close SFTP too early

Mitigation:
- SFTP begins only after SSH terminal is truly usable

## 11. Product Owner Decision

For the next implementation batch, the acceptance question is not:
- “Did we add more UI?”

It is:
- “Can a user really connect to one SSH host and use a shell?”

If the answer is no, the batch is incomplete.
