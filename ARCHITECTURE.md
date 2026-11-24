# Architecture

High-level architecture of Vendredi. Goals and non-goals are in [README.md](README.md). Dependency choices are deliberately not made here, with two exceptions: the script engine is Boa and the embedded store is SQLite.

## Shape

One cargo workspace, one shipped binary.

| Part | Role |
|---|---|
| `core` (library) | Domain model, EDI layer, scenario engine, script host, and the transport and storage interfaces. No UI dependency. |
| `transports` | HTTP and watched folder. AS2, FTPS and SFTP are added later behind the same interface. |
| `app` (binary) | Two thin shells over `core`: an egui GUI and a headless mode. With no arguments it opens the GUI; `run --headless <workspace>` runs without it. |

## Data flow

```
transport ──bytes──▶ EDI layer ──interchange──▶ scenario engine
(HTTP/folder)        parse, validate            match rules → actions
     ▲                                               │
     └──bytes── EDI layer ◀── reply documents ───────┘
                build, envelope

every step ──▶ event log ──▶ GUI traffic view / JSON lines on stdout
```

## Main pieces

### Partner

A simulated trading partner: its `UNB` identifiers, its channels and its scenario. One workspace hosts several partners.

### EDI layer

Our own types for interchange, message and segment, plus parsing, writing, validation and `CONTRL` generation. Any third-party EDIFACT crate stays hidden behind this layer, so it can be replaced.

### Scenario engine

Scenarios are declarative. A rule matches inbound messages and lists the actions to take:

- send a static EDIFACT file
- run a script
- delay
- inject a fault (reject, duplicate, malformed reply, and so on)

Rules hold only simple matches. Any condition or computation beyond that belongs in a script. There is no template or expression language.

### Script host

Scripts are JavaScript, run by an embedded Boa engine.

- A script receives the parsed inbound interchange and a small host API: build and send messages, generate `CONTRL`, get the next control reference, read the current time, log.
- A script has no file or network access.
- The JS context is owned by the engine thread.
- Delays are handled by the engine, not by the script sleeping.

### Event log

An append-only record of everything received, decided and sent. The GUI traffic view and the headless output are both readers of it.

## Runtime model

- **The engine is the single owner of state.** It runs off the UI thread, takes commands (from the GUI or CLI) and transport events in, and emits events out. The GUI never touches engine state directly; this is what makes headless mode free.
- **Transports talk to the engine by messages only.** Each transport can be threaded or async internally without that choice leaking into `core`.
- **Time is injected.** Scenarios with delays are testable without real sleeping.

## Workspace on disk

- **Configuration is plain text files** (partners, rules, scripts, static documents) in a directory that can be committed next to the integration code. The GUI is an editor for those files, and CI runs the same directory headless.
- **Everything recorded lives in one SQLite database**, `.vendredi/vendredi.db` inside the workspace: traffic history and per-partner state. It is not meant to be committed. SQLite is compiled into the binary, so there is nothing to install.

## Data model

There are two kinds of data: what the user authors (the workspace) and what the simulator records (traffic).

```
Authored                              Recorded
────────                              ────────
Workspace                             Transfer ──< Interchange ──< Message
 └─ Partner ─┬─ Channel (receive/send)                  │              │
             ├─ Rule ──< Action                      Finding        Execution ──< Transfer (out)
             │            └─ Script | Document
             └─ State (counters, key/value)
```

### Authored (workspace files)

| Entity | Holds |
|---|---|
| Workspace | Our own side: the `UNB` identity of the system under test, the listening port, history retention |
| Partner | Name, `UNB` identity and qualifier, syntax settings (character set, syntax version, separators), acknowledgement policy |
| Channel | Belongs to exactly one partner, in one direction (receive or send), of one kind: HTTP (path or URL, headers) or folder (directory, file pattern) |
| Rule | A simple match (message type, optionally version and release) and an ordered list of actions |
| Action | One of: send document, run script, delay, fault |
| Script | A JavaScript file and the function to call |
| Document | A static EDIFACT file |

### Recorded (SQLite)

| Entity | Holds |
|---|---|
| Transfer | One thing received or sent on a channel: direction, partner, time, the raw bytes, transport details (headers, file name), delivery status |
| Interchange | Summary of one `UNB`…`UNZ`: sender, recipient, control reference, test flag, acknowledgement requested, acknowledgement status |
| Message | Summary of one `UNH`…`UNT`: type, version, release, reference, segment count |
| Finding | A validation result: code, severity, position in the raw bytes, text |
| Execution | One run of a rule: the triggering message, each action's result, script log output, error |
| State | Per partner: the control reference counter and a key/value store that scripts can read and write |

### Principles

- **Raw bytes are the truth.** We store what arrived and what we sent, plus summary fields for listing and searching. The full segment tree is re-parsed on demand, not stored.
- **Everything recorded is immutable except status.** A transfer's delivery status and an interchange's acknowledgement status change; nothing else does.
- **Cause is always linked.** Every outbound transfer points to the execution that produced it, and every execution to the message or command that triggered it. This chain is what the GUI shows.

## Detailed flow

### Inbound (the system under test sends to the partner)

1. **Receive:** the transport hands over bytes and metadata. The transfer is recorded before anything else, so even garbage is visible. Over HTTP, the POST is answered with a plain status response at this point; reply documents never travel in the HTTP response body.
2. **Parse:** the bytes are split into interchanges (one transfer can hold several) and messages. A failure becomes a finding and the flow stops there for that interchange.
3. **Route:** the channel decides the partner. The `UNB` sender and recipient are checked against it, and a mismatch is a finding.
4. **Validate:** envelope and syntax checks produce findings.
5. **Acknowledge:** per the partner's policy, a `CONTRL` is built from the findings (accept or reject).
6. **Match:** each message is tried against the partner's rules in order; the first match wins. No match is recorded as such.
7. **Execute:** actions run in order. Delays are scheduled on the engine's clock.
8. **Build:** scripts and documents supply messages only. The engine wraps them in the envelope (identities, control references, counts), unless a fault action says to break it.
9. **Deliver:** the reply goes out through the partner's send channel, and the attempt is recorded. A failed delivery is not retried automatically; it can be resent by hand.

### Outbound (the partner sends to the system under test)

1. **Trigger:** a command from the GUI or CLI names a partner and a document or script.
2. **Build and deliver:** steps 8 and 9 above.
3. **Correlate:** when the system's `CONTRL` comes back in, it is matched by control reference and the original interchange's acknowledgement status is updated.

### Where faults are injected

| Level | Examples |
|---|---|
| Transport | HTTP 500, slow response, no response |
| Acknowledgement | Reject with `CONTRL`, send none |
| Envelope | Wrong count in `UNZ`, duplicate control reference |
| Content | Whatever the script returns |

## Working assumptions

These were proposed but not yet confirmed.

- **Validation depth:** the first version validates envelope and syntax only. Message-structure validation against UN/EDIFACT directories is a later, pluggable layer.
- **Partner-initiated sends:** the simulated partner can send a document on its own, triggered from the GUI or CLI.
- **Headless scope:** the first version runs the partners and prints events as JSON lines. Built-in assertions and a control API for tests come later.

## To verify when choosing dependencies

- Whether Boa, with its Unicode data, fits the 30 MB binary target alongside egui.
- Whether Boa's runtime limits are enough to stop a script that hangs.
