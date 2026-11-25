# Vendredi

An EDI simulator for developers: a fake trading partner that runs on your machine, receives the EDIFACT documents your integration sends, and replies with acknowledgements and response documents.

## Run an exchange

On macOS, extract the download, move **Vendredi.app** to Applications, and double-click it in Finder. For CLI commands, use `/Applications/Vendredi.app/Contents/MacOS/vendredi`. On Windows or Linux, open the extracted executable. On first launch it creates a sample workspace in the user's home directory under `Vendredi`. Choose **Send sample order** in the empty traffic list or from **Send document…**. Select the inbound transfer or either reply to inspect its raw bytes, validation findings and execution history. The sample supplier writes a `CONTRL` and an `ORDRSP` into the workspace’s `replies` folder.

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

- Choose **+ Add partner**, or select a partner and choose **Edit partner**. Set its EDIFACT identity, acknowledgement policy, intake channels, and reply destination. **Save partner** updates `vendredi.toml` and reloads the workspace. The sample Supplier watches `inbox` inside the workspace for `.edi` files.
- Open **Scripts → Add script** to choose a partner and an existing or new rule. Enter a JavaScript filename and function, edit the source, and choose **Save script**. Click a script in the list to edit it or detach it from its rule. Detaching keeps the source file.
- Open **Rules** to edit a rule’s message match, priority, and ordered actions. Clicking a matched rule in the traffic timeline opens the same editor.
- **Settings → Edit settings** controls the listener address, workspace identity, and traffic retention. **Edit configuration file** opens the full TOML.
- Each partner has ordered rules, one or more receive channels and one send channel. The first matching rule runs. The sample supports UN/EDIFACT syntax version 3, independent of the message directory release.
- To submit a file through a folder channel, write it under a temporary name in the same directory, close it, and rename it to the configured suffix (e.g. `.edi`). The suffix signals that the file is complete. Vendredi claims and removes it after recording. Each outgoing file is published under its transfer ID.
- Choose **Traffic → Clear history…** to permanently remove exchanges from the last minute, hour, day, or week. Select all partners or a single partner, including one already removed. The last transfer determines the age of each exchange. Cleanup skips pending deliveries and scheduled actions and preserves partner state and reference counters.
- A running headless instance accepts `send Supplier confirmation.edi`, `send Supplier script.js functionName`, `inspect 1`, `resend 2`, and `quit` on standard input. Use a JSON array of strings for arguments containing spaces. With the workspace closed, run `vendredi send ./demo Supplier confirmation.edi`, `vendredi inspect ./demo 1` or `vendredi resend ./demo 2`. One engine owns the workspace at a time.
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
