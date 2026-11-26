# Vendredi

An EDI simulator for developers: a fake trading partner that runs on your machine, receives the EDIFACT or X12 documents your integration sends, and replies with acknowledgements and response documents.

## Run an exchange

On macOS, extract the download, move **Vendredi.app** to Applications, and double-click it in Finder. For CLI commands, use `/Applications/Vendredi.app/Contents/MacOS/vendredi`. On Windows or Linux, open the extracted executable. The first launch shows a welcome screen: **Try the demo** sets up a sample workspace in the user's home directory under `Vendredi`, **Create a workspace…** sets one up in a folder you choose and walks through describing your first partner, and **Open a workspace…** opens an existing one. Later launches reopen the last workspace. In the demo, the **Getting started** steps above the traffic list send a sample order, open the reply, make the supplier reject orders and send the order again. Select any transfer to inspect its raw bytes, validation findings and execution history; hover a segment to see what it is for. The sample supplier writes a `CONTRL` and an `ORDRSP` into the workspace’s `replies` folder.

The [illustrated workspace handbook](https://vendredi.app/guide/) covers the first exchange, traffic inspection, partner setup, rules, scripts, retries and cleanup. The guide includes full-resolution screenshots and a downloadable starter workspace with the documents and scripts used in the walkthrough. For a quick reference on GitHub, see [traffic search and rule order](docs/guide/traffic-and-rules.md).

To run the sample headless, initialize a workspace and start its listener.

```sh
vendredi init ./demo
vendredi run --headless ./demo
```

Submit the example interchange from a second terminal.

```sh
curl -i --data-binary @demo/order.edi http://127.0.0.1:8180/supplier/orders
```

The listener returns HTTP 202 after recording the inbound bytes. Replies use the partner’s send channel. Headless mode writes one JSON event per line. To receive replies by HTTP, point the send URL at a local receiver as shown in the [HTTP example](examples/orders/vendredi.toml).

## Workspaces and commands

- Choose **+ Add partner** to set up a partner in three steps: who it is, how it connects, and which case answers it. **Edit partner** shows every setting at once. Saving updates `vendredi.toml` and reloads the workspace. A partner's page shows the address it sends to, with buttons that copy the URL or a `curl` command, and when it last sent and received. The sample Supplier watches `inbox` inside the workspace for `.edi` files.
- Open **Cases** to choose the case that answers a partner and to edit its rules: their message match, order, and actions. Clicking a matched rule in the traffic timeline opens the same editor. **Add sample** adds a message to try the case with, and **Run checks** compares the case's replies with the expected ones, as `case test` does.
- Open **Scripts → Add script** to choose a case and one of its rules, or a new rule. Choose or enter a JavaScript file inside the case folder and a function, edit the source, and choose **Save script**. The editor highlights JavaScript and checks its syntax as you type, naming the line of an error. Click a script in the list to edit it or detach it from its rule. Detaching keeps the source file.
- **Settings → Edit settings** controls the listener address, workspace identity, and traffic retention. **Settings → Advanced → Edit configuration file** opens the full TOML.
- Removing a case, rule, sample or partner happens at once, and the confirmation that appears offers **Undo**. Drop an EDI document on the window to send it, or a case file or folder to import it.
- ⌘1 to ⌘5 open the main pages, ⌘F searches traffic, ⌘N adds a case or partner, ⇧⌘S sends a document, ⌘S saves the open file and ⌘O opens another workspace; the arrow keys move through lists. On macOS the menu bar holds the same commands, and **Help → Report a Problem…** prepares a report you can read and edit before sending. The desktop follows the system's light or dark appearance.
- Each partner has one or more receive channels, one send channel and an active case whose ordered rules answer its messages. The first matching rule runs. A partner without an active case only acknowledges what it receives. Vendredi supports UN/EDIFACT syntax version 3, independent of the message directory release, and ANSI X12 interchanges (see [X12 partners](#x12-partners)).
- To submit a file through a folder channel, write it under a temporary name in the same directory, close it, and rename it to the configured suffix (e.g. `.edi`). The suffix signals that the file is complete. Vendredi claims and removes it after recording. Each outgoing file is published under its transfer ID.
- Choose **Traffic → Clear history…** to permanently remove exchanges from the last minute, hour, day, or week. Select all partners or a single partner, including one already removed. The last transfer determines the age of each exchange. Cleanup skips pending deliveries and scheduled actions and preserves partner state and reference counters.
- A running headless instance accepts `send Supplier confirmation.edi`, `send Supplier script.js functionName`, `inspect 1`, `resend 2`, `case Supplier reject-orders`, `sample Supplier Order`, `initiate Supplier Invoice` and `quit` on standard input. Use a JSON array of strings for arguments containing spaces. With the workspace closed, run `vendredi send ./demo Supplier confirmation.edi`, `vendredi inspect ./demo 1` or `vendredi resend ./demo 2`. One engine owns the workspace at a time.
- `vendredi gui ./demo` opens the same configuration and traffic history. Embedded Turso stores exact bytes, delivery attempts, findings, per-partner counters and script state in `.vendredi/vendredi.db`. History retention runs when the workspace opens. Failed or interrupted deliveries are available for explicit resend.
- A partner can acknowledge `always`, when `requested`, or `never`. Envelope, syntax and identity findings suppress business replies. Incoming `CONTRL` messages correlate by partner identity and control reference. Ambiguous references produce an event. The engine skips automatic acknowledgements for interchanges containing a `CONTRL`.
- Actions use `document`, `script` and `function`, `delay_ms`, or `fault`. Fault values are `reject`, `suppress_ack`, `duplicate_reference`, `wrong_count`, `duplicate`, `malformed`, `http500`, `slow_response`, and `no_response`. HTTP and acknowledgement faults affect the triggering receipt or acknowledgement. Envelope and content faults affect subsequent sends in the execution. Scheduled delays live in the running engine.

A JavaScript action receives the inbound interchange and the host API. Partner-initiated scripts receive `null` for the interchange. The engine builds the envelope, header, trailer, counts and escaping for sent messages.

```js
function confirm(interchange, api) {
    const order = interchange.groups[0].messages[0];
    const bgm = order.segments.find(s => s[0] === "BGM");
    const count = (api.get("orders") || 0) + 1;
    api.set("orders", count);
    api.log("Confirmed order " + bgm[2]);
    api.send(api.build("ORDRSP", order.version, [
        ["BGM", "231", "R" + count, "9"],
        ["DTM", ["137", "20261125", "102"]],
    ]));
}
```

The interchange has `standard`, `sender`, `recipient`, `reference`, `test` and `groups`. Each group holds `messages`, and each message has a `type` (`ORDERS`), a `version` (`D96A`), a `reference` and its body `segments`. A segment is an array of its tag and elements. A simple element is a string and a composite element is an array of components. Separators and release characters are already resolved, so values are plain text. `api.build(type, version, segments)` returns a message in the same shape, and `api.send(message)` queues it. An optional fourth argument sets `agency` (default `UN`) and `association`, for example `{ association: "EAN008" }`.

Scripts can also call `api.ack()` to request the acknowledgement for the partner’s standard (`api.contrl()` is an alias), `api.nextReference()` and `api.now()` (Unix milliseconds). Every script runs in a fresh Wasmtime sandbox with 32 MiB of guest memory and an instruction budget. Scripts have no direct filesystem or network access. They can read the inbound interchange, update partner state and request delivery through the configured partner channel. Static documents contain a single `UNH` through `UNT` message using the default separators. The engine applies the partner’s separators and identities on delivery.

Scripts written for 0.1 keep working: `api.build("ORDRSP:D:96A:UN", "BGM+231'")` with raw segment text, `api.send` of that text, and the `messages`, `header`, `findings` and `syntax` properties of the interchange. Each use records a `deprecated` event naming the feature.

Rules match `message`, the message type or X12 transaction set, and an optional `version`, such as `D96A` or the X12 `GS08` value `005010`. A bare directory such as `D` matches every release. Workspaces from 0.1 that set `version = "D"` and `release = "96A"` load unchanged.

## Cases

A case is a reusable partner scenario: rules and actions, the documents and scripts they use, sample inbound messages, partner-initiated sends and the replies it should produce. A case never contains identities or channels. The partner supplies them, so one case works with any partner of the same EDI standard. A shared repository of cases can describe typical behavior, such as a partner that confirms, rejects, answers slowly or sends malformed replies.

Rules live only in cases. Each partner names its active case in `vendredi.toml`, as `case = "reject-orders"`, and the case's folder under `cases/` holds its rules, documents and scripts. Any installed case of the partner's standard can be made active. `vendredi init` installs the starter cases from [`examples/cases`](examples/cases) and makes `confirm-orders` active for the sample partner.

```sh
vendredi case list ./demo
vendredi case use ./demo Supplier reject-orders
vendredi sample ./demo Supplier Order
vendredi case use ./demo Supplier confirm-orders
```

In the desktop, open **Cases** to read a case, choose **Make active**, send its samples or run its partner-initiated sends. **New case** starts an empty case, and **Duplicate** copies a case with its files. Cases you create, duplicate or install with `init` are yours to edit. An imported case is read-only, because importing it again replaces it: duplicate it to change it. A case that is active for a partner cannot be deleted. Each switch records a `case_switched` event, and the traffic timeline names the case that answered each message.

### Case format

A case is a folder under `cases/` with a `case.json` and the files it references, or a single JSON file that embeds those files in a `files` map. Imports write the folder form.

```json
{
  "vendredi_case": 1,
  "id": "reject-orders",
  "name": "Reject every order",
  "description": "Answers each ORDERS with a negative CONTRL and sends no business reply.",
  "standard": "edifact",
  "tags": ["orders", "fault"],
  "acknowledge": "always",
  "rules": [{ "name": "Orders", "message": "ORDERS", "version": "D96A", "actions": [{ "fault": "reject" }] }],
  "samples": [{ "name": "Order", "document": "order.edi" }],
  "initiate": [],
  "expect": [{ "sample": "Order", "replies": ["nack.edi"] }]
}
```

Each rule has a `name`, a `message`, an optional `version` and ordered `actions`: a `document`, a `script` with its `function`, a `delay_ms` or a `fault`. Conditions belong in JavaScript, and the format has no expression language. `acknowledge` optionally overrides the partner's policy. Documents, samples and expected replies are single `UNH`…`UNT` messages, or `ST`…`SE` transaction sets for X12, with the default separators. The engine wraps a sample in an interchange from the workspace identity to the partner. `initiate` lists named action lists that the partner starts on its own, such as sending an invoice. File paths must stay inside the case.

A catalog lists cases by relative path, as folders or single JSON files:

```json
{ "vendredi_catalog": 1, "name": "Starter cases", "cases": ["confirm-orders.json", "late-partner/"] }
```

### Import and test

```sh
vendredi case import ./demo https://example.com/edi-cases/catalog.json
vendredi case import ./demo ../edi-cases --replace
vendredi case test ../edi-cases
```

`case import` accepts a case folder, a case or catalog JSON file, a folder with `catalog.json`, or an `https://` URL. It fetches and checks every case before writing anything: file references, documents and script syntax. Imported scripts are only parsed, and run later in the sandbox when a rule fires. Unchanged cases are skipped. A case already installed with different content is listed with its differences, and replaced only with `--replace` or after confirming in the desktop's **Import cases…** dialog. Each installed case records its source, the SHA-256 of its JSON and the import time in `.vendredi-source.json`. HTTPS imports trust the bundled web roots and the system's certificates, refuse redirects to other schemes, and limit each file to 1 MiB.

`case test` runs every `expect` entry in a temporary workspace with the workspace identity `BUYER:ZZ`, the partner `PARTNER:ZZ`, a clock starting at 2026-01-01T00:00Z and fresh references, so the first sample's interchange reference is `S1` (X12 control number `1`). X12 checks use the `GS08` version of the case's rules, or `005010`. Scheduled delays complete immediately. It compares each sent message with the expected documents, prints the first differing byte of a mismatch, and fails if any check fails. A case repository can run it in CI.

## X12 partners

A partner with `standard = "x12"` exchanges ANSI X12 interchanges. Its `identity` is the `ISA` sender ID and qualifier, and the workspace `identity` fills the other side. [`examples/x12`](examples/x12) is a complete workspace whose partner confirms each 850 with an 855.

```toml
[[partners]]
name = "Supplier"
standard = "x12"
identity = ["SUPPLIER", "ZZ"]
acknowledge = "always"
receive = [{ path = "/supplier/850" }]
send = { directory = "replies" }
x12 = { version = "005010", acknowledgement = "997", ta1 = false, usage = "T" }

case = "orders"
```

The `orders` case in [`examples/x12/cases/orders`](examples/x12/cases/orders) confirms each 850 with the 855 next to its `case.json`.

- `x12.version` is the `GS08` value for documents and replies, and `ISA12` follows from it. `acknowledgement` is `997` or `999`, sent once per functional group. `ta1 = true` also sends a `TA1`. `usage` sets `ISA15`. `application` and `workspace_application` set the `GS02`/`GS03` codes, which default to the identities. `delimiters` sets the element, repetition, component and segment characters, such as `*^:~`. Use `U` for no repetition separator before version 00402.
- Delimiters are read from each inbound `ISA`. Vendredi reports wrong `ISA` widths, `SE`, `GE` and `IEA` counts and mismatched control numbers with their byte offsets. A count or control problem inside one transaction set rejects only that set in the group acknowledgement, and the other sets still reach the rules. Envelope problems and the `reject` fault reject the interchange.
- Documents and expected replies are one `ST`…`SE` transaction set with the default delimiters. The engine writes `ISA`, `GS`, `GE`, `IEA`, the `ST02`/`SE` control numbers and counts. Each reply is one interchange with one group, and its `GS06` equals its `ISA13`.
- Faults behave as for EDIFACT. `reject` sends rejecting 997/999 and TA1 acknowledgements. `wrong_count` miscounts `GE01` and `IEA01`. `duplicate_reference` reuses `ISA13`.
- Incoming 997, 999 and TA1 acknowledgements mark the matching outbound interchange as acknowledged, rejected or partially accepted.
- Scripts receive the same interchange shape as EDIFACT. Groups add `functional` (`GS01`), `sender`, `recipient`, `reference` (`GS06`) and `version` (`GS08`). Messages add `implementation` (`ST03`) when present. A repeated element is `{"repeat": [...]}`. Build replies with `api.build("855", "005010", segments)`. An optional fourth argument can set `implementation`. Values cannot contain a delimiter, because X12 has no release character.

## Build and test

```sh
rustup target add wasm32-wasip1
cargo build --release --locked
cargo test --workspace --locked
cargo test --locked -p vendredi exchange_edit_and_reopen -- --ignored
python3 tests/local_instance.py --binary target/release/vendredi
```

The last two commands bind loopback listeners and use temporary workspaces. On Windows pass `target/release/vendredi.exe`. See the [implementation milestones](IMPLEMENTATION.md) and [runtime architecture](ARCHITECTURE.md).

## Release

Set the workspace version in `Cargo.toml`, write the notes in `docs/releases/vX.Y.Z.md`, then push the tag `vX.Y.Z`. The tag build signs both Mac apps with the Developer ID certificate, has Apple notarize them and staples the ticket. It then runs the same archive tests as every other build and publishes the five archives, a Linux AppImage for each architecture and `SHA256SUMS` as a GitHub release. Other builds sign the Mac apps ad hoc.

The tagged Mac builds run in the `release` environment, which is restricted to `v*` tags, and read these secrets from it:

| Secret | Value |
| --- | --- |
| `MACOS_CERTIFICATE_P12` | Developer ID Application certificate and private key, exported as `.p12` and base64-encoded |
| `MACOS_CERTIFICATE_PASSWORD` | Password of that `.p12` |
| `MACOS_NOTARY_KEY` | Contents of the App Store Connect API key (`.p8`) |
| `MACOS_NOTARY_KEY_ID` | ID of that key |
| `MACOS_NOTARY_ISSUER` | Issuer ID of the App Store Connect team |

To build the same notarized ZIP on a Mac, run `scripts/package_release.py` with `MACOS_SIGNING_IDENTITY` set to the certificate name and `MACOS_NOTARY_PROFILE` set to a profile saved with `xcrun notarytool store-credentials`.

## Goals

- **Dev tool.** Used by developers building and testing EDI integrations locally. It never carries real partner traffic.
- **Trading partner simulation first.** The core is an endpoint that accepts inbound interchanges and answers the way a partner would. Viewing, generating and validating documents exist to support that, not as a separate product.
- **EDIFACT.** The only supported standard.
- **Transports: HTTP and watched folder.** AS2, FTPS and SFTP come later.
- **Single binary.** No installer, runtime, Docker or external database. State lives in plain files or an embedded store.
- **Easy to use.** Launch and be doing something useful within a minute, with sensible defaults and no mandatory configuration.
- **Low footprint.** Initial targets:
  - near 0% CPU when idle (the UI repaints on events only)
  - under 100 MB RAM
  - starts in under a second
  - binary under 30 MB
- **Headless mode.** The same binary runs without the UI, for CI and scripted tests.
- **Cross-platform.** Windows and macOS (Intel and Apple Silicon) are the tested targets. Linux should build and run, but is not a tested target.

## Non-goals

- **Not a production gateway, VAN or translator.** No high availability, no throughput targets, no certification.
- **No X12** or other standards besides EDIFACT.
- **No cloud.** No accounts, no telemetry, no network dependency; it works fully offline.
- **No multi-user features.** One developer, one machine.
- **Not a mapping or ETL tool.** It does not transform EDI to or from your internal formats.
- **No custom DSL or template language.** Anything a declarative rule cannot express is written in JavaScript.
- **No plugin system** in the first version.
- **No web or mobile version.**

## Technical direction

- **Rust**, with **egui** for the UI.
- The simulator core is a library with no UI dependency. The GUI and the headless mode are thin shells over it, shipped in one binary.
- Scenarios are declarative: rules match inbound messages and list the actions to take. Logic that goes beyond matching, such as building a reply from the inbound message, is JavaScript run by **Boa** inside an embedded Wasmtime sandbox.
- Transports sit behind a common interface so AS2, FTPS and SFTP can be added without touching the core.
