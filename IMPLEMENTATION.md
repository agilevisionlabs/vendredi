# Vendredi implementation plan

The first runnable milestone is a local partner exchange. A developer starts Vendredi headless, posts an `ORDERS` interchange, receives the partner’s `CONTRL` and a static `ORDRSP` over its send channel, and can trace both replies to the inbound bytes. The GUI will run this same exchange through the same engine. The starting point is a design-only repository. The scope comes from the [product goals](README.md) and the [current architecture](ARCHITECTURE.md).

## 1. Establish the workspace and test the dependency choices

- Create a Cargo workspace with a UI-independent `core` library, a `transports` crate, and one `vendredi` binary in `app`. Let `app` construct the engine and wire up adapters. Define the first command, transport input, event, clock and storage interfaces around the sample exchange.
- Build a release binary that opens an egui window, evaluates a Boa function and commits a transaction to embedded Turso. Exercise the actual features that will ship. Record artifact size, startup time and idle memory as a baseline for the release checks below.
- Run a nonterminating script and a deeply recursive one under the proposed Boa limits. Require the next engine command to complete after each script fails. For Turso, interrupt a write, reopen the file, check transaction integrity and inspect the stored bytes with a SQLite reader. If either spike fails, resolve the engine limit or storage choice before building features that depend on it. The architecture identifies embedded SQLite as the storage fallback.

## 2. Ship the first headless HTTP exchange

Build these pieces in order. Keep the sample workspace and expected input and output documents as fixtures for subsequent milestones.

1. **Load one partner.** Define a minimal plain-text configuration with the local and partner `UNB` identities, syntax settings, acknowledgement policy, receive path, send URL and one ordered rule that sends a document. Resolve script and document paths relative to the workspace. Bundle an example workspace with an `ORDERS` input and an `ORDRSP` message. Report a bad reference with its file and rule before starting the listener.
2. **Read and write EDIFACT.** Introduce interchange, message and segment types. Parse envelopes, service characters (including `UNA` and escaped separators), and multiple messages and interchanges per transfer. Report envelope, reference and count errors with byte positions. Build envelopes and `CONTRL` from parsed identities, references and findings. Cover custom separators, malformed input, and a bad interchange followed by a valid one in the fixture set.
3. **Record the exchange.** Create the schema in `.vendredi/vendredi.db`. Persist exact transfer bytes and channel metadata first. Store interchange and message summaries, findings, executions, action results and append-only events. Allocate each partner’s next reference and record the outbound intent together before delivery. Preserve the link from each outbound transfer to its execution and trigger, including an engine-generated acknowledgement or a CLI command. Reparse raw bytes when detail is requested.
4. **Connect the HTTP path.** Run the single state-owning engine off the UI thread. A receive adapter passes bytes and metadata to it and returns a plain HTTP status once recording succeeds. Route by channel, check both `UNB` identities, apply the acknowledgement policy, then run the first matching rule for each eligible message. Record a miss. Envelope the static message and deliver replies by separate requests to the configured send URL. Record each delivery result and expose engine events as JSON lines from `vendredi run --headless <workspace>`.

**Acceptance.** Use a local HTTP receiver to capture both replies. Check the plain POST status response, acknowledgement reference, reply identities and counts, and the causal chain in the database and event stream. Send unparsable bytes and verify that the original payload and finding survive restart. Take the receiver offline and verify one failed attempt per outgoing transfer with no automatic retry.

## 3. Complete partner behavior in the core

- Load multiple partners and channels. Exercise first-match ordering by message type, optional version and release. Run actions in declaration order. Schedule delays with the injected clock so another message can progress while one execution waits.
- Add the Boa host on the engine thread. Give scripts parsed input, message building and sending, `CONTRL`, reference allocation, clock, logging and per-partner key/value state. Expose no filesystem or network bindings. Route generated messages through the same envelope and delivery path as static documents. Record script failures and action outcomes against the execution.
- Add transport response faults, acknowledgement rejection or suppression, duplicate references, wrong envelope counts and script-controlled malformed content. Select HTTP response faults before answering the POST. Keep receipt recording independent of the simulated response.
- Wire partner-initiated document and script sends into the command interface and CLI. Correlate incoming `CONTRL` with the original outbound interchange using partner and control reference. Support explicit resend as a linked delivery attempt. Apply workspace history retention while preserving partner counters and key/value state.

**Acceptance.** In a fake-clock test, trigger two partners with identical reference values and a delayed action. Verify ordering, isolated state and acknowledgement correlation. Exercise each fault level and a timed-out script, then process a valid message. Reopen the workspace, check counter continuity, and verify that manual resend preserves the original attempt and its outcome.

## 4. Add watched-folder receive and send

Implement both directions behind the existing transport interface. Define when an inbound file is complete and how it is claimed across duplicate notifications and restarts. For outgoing files, write to a temporary name and publish the finished file by rename. Retain the filename as transfer metadata. Validate channel paths and patterns so a partner cannot accidentally consume its own output.

**Acceptance.** Replay the HTTP fixtures through folders and compare document bytes with deterministic time and references, findings, rule decisions and causal links. Exercise a partial write, duplicate notification, restart and unwritable output directory. Verify a single ingestion per claimed file and a visible delivery failure that can be resent.

## 5. Deliver the desktop workflow

Open egui when the binary receives no arguments. Provide a ready-to-run sample partner and a way to send its example interchange within a minute of first launch. Build the traffic list and detail view from engine events and stored summaries. Show raw bytes, parsed segments, findings and the path through the rule to each reply. Expose partner-initiated send and manual resend through the command interface. Add workspace editing for identities, channels, policies, rules, scripts and static documents, using the same loader and validation as headless mode. Request repaint on engine or user events.

**Acceptance.** On a fresh machine, launch the binary offline, complete the sample exchange and inspect its replies. Edit a rule in the GUI, reopen the saved workspace with the headless command, and verify the new behavior and the same persisted history.

## 6. Verify the release binary

Run the HTTP and folder exchange, fault, restart and GUI smoke checks on Windows and macOS (Intel and Apple Silicon). Build and launch on Linux. Distribute one self-contained executable per platform and architecture. Verify first launch and the bundled sample on a clean machine offline. Capture release binary size, time from launch to ready, resident memory and idle CPU over a fixed observation window for GUI and headless runs. Compare against the initial goals of under 30 MB on disk, under one second to start, under 100 MB RAM and near-zero idle CPU. Record the platform, workload and any missed target so the measurements can be repeated.

## Decisions to settle during implementation

| Decision | Planning assumption and when it matters |
|---|---|
| Validation and acknowledgements | Plan for envelope and syntax validation in v1. Before milestone 2 fixtures are finalized, select the supported syntax profile and define which findings reject an interchange, suppress business replies, or permit processing. Define handling for incoming `CONTRL` to avoid acknowledgement loops. |
| Configuration and channels | Choose a plain-text format in milestone 2. Before adding multiple channels, define send-channel selection when a partner has several and reject ambiguous receive routes during loading. |
| Headless commands | Assume the first version runs partners and emits JSON lines, with partner-initiated sends available from GUI and CLI. Settle CLI invocation and workspace ownership in milestone 3 so a command cannot create a competing writer. Built-in assertions and a control API are deferred in the architecture. |
| Fault and recovery semantics | Before milestone 3, define how HTTP faults are chosen from an inbound request, how to report ambiguous acknowledgements after a duplicate-reference fault, and how interrupted outbound attempts appear for manual resend. |

The proposed first release defers directory-based message validation, further transport protocols and a plugin system.

## Local verification on 2026-11-24

The release builds use the locked Cargo workspace. Ten core tests passed, including termination during an open database transaction, script budget recovery, deterministic reply bytes, acknowledgement correlation, retention and reference continuity. The separate desktop widget test exercised the sample-order button, transfer inspection, text editing, save/reload and headless reopening.

Six local-instance tests passed for each of the four executables below. They exercised HTTP delivery to a real receiver, offline delivery failure and resend, malformed input and process restart, folder publication and recovered claims, unavailable output directories, acknowledgement, envelope and duplicate-delivery faults, and slow or silent HTTP receipts with scripted malformed content.

| Executable | Bytes | Local environment |
|---|---:|---|
| macOS arm64 | 22,158,704 | Native macOS 27.2.1, including window and sample exchange |
| macOS x64 | 22,227,232 | Rosetta on the same Mac, including window and sample exchange |
| Windows x64 | 20,525,568 | MinGW build under Wine in an offline Linux container. A no-argument launch created the sample workspace, opened the desktop and produced both replies. |
| Linux arm64 | 23,166,768 | Native arm64 Linux VM. A separate offline container received only the binary and sent a bundled document. |

The table below uses decimal MB. Each release run initialized a fresh workspace, timed launch to the ready event or visible native window, completed an order, settled for one second, and measured resident memory and process CPU over five seconds. These are single-run observations. Run `tests/release_macos.py` to repeat them.

| Executable and mode | Ready | Resident memory | Idle CPU |
|---|---:|---:|---:|
| macOS arm64 headless | 19 ms | 17.4 MB | 0.0% |
| macOS arm64 desktop | 328 ms | 124.1 MB | 0.0% |
| macOS x64 headless under Rosetta | 29 ms | 14.8 MB | 0.0% |
| macOS x64 desktop under Rosetta | 705 ms | 73.3 MB | 0.2% |

The arm64 desktop exceeds the initial 100 MB resident-memory target. The original three-dependency GUI spike at commit `d6d433d` measured 20,134,288 bytes on disk, 755 ms to a visible window and 125.6 MB resident using the same window probe. All four final executables are under 30 MB. Native Windows and physical Intel Mac results are pending. The repository includes a native runner matrix, a Windows window and resource probe, and packaging that verifies the executable extracted from each archive.
