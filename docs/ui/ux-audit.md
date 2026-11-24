# Desktop UX and UI audit

Audited on 24 November 2026 against the [main-window wireframe](main-window.html). The native review used a disposable macOS Apple Silicon workspace with two partners, an accepted order, two replies, a rejected file and a JavaScript rule. Native frames were inspected at 1440 × 860 and 680 × 420 logical pixels. Widget interaction tests also exercised the desktop at 1080 × 680.

## Findings and verification

| Flow | Finding and resulting behavior | Verification |
| --- | --- | --- |
| Launch and listen | A port conflict previously prevented access to the workspace. The desktop now opens stopped with the error visible. Starting on an ephemeral port displays the assigned port. A workspace that fails to load shows the path, the cause and a Retry button. | Occupied a loopback port, recovered the listener, and repaired invalid TOML while the desktop was open. Verified a release window and a real order exchange. |
| Traffic triage | The wide view keeps the wireframe’s sidebar, traffic table and inspector. At minimum size the old layout left almost no room for the message. Selecting a row now opens its inspector with Back to traffic. The timeline can expand independently. | Selected a transfer at minimum size, inspected it and returned to traffic. Exercised direction and status filters, attention, search text, and clearing an empty result. |
| Inspect an exchange | The timeline’s rule link previously opened the whole configuration. Rule and script links now open the corresponding editor for the recorded partner. Findings, script logs, transport details and raw segments stay with the selected transfer. | Used an actual scripted exchange to follow both links, dismiss their dialogs and read findings and logs. Inspected native captures of all four tabs. |
| Send and recover | Sending required recalling configured filenames and script functions. The dialog now offers the partner’s configured choices, allows manual entry, and keeps the sample action visible at minimum height. A folder-only partner accepts the sample through its configured folder and suffix. | Sent a document, ran a configured script, tested invalid input, and sent a folder sample with a custom suffix. Real transport tests exercised failure, restart and resend. |
| Manage partners | Add and Edit expose EDIFACT settings and transport channels directly. A separate red removal action sits beside Edit. History remains accessible after removal. | Created and edited a partner, tested validation, cancelled removal, confirmed removal and reopened its retained traffic. |
| Manage rules and scripts | Rules have a partner-scoped editor, ordered actions and explicit priority. Script editing exposes the filename, function and source together at compact sizes. Detaching a script identifies the association and keeps its file. | Created, edited, reordered and deleted rules across two partners. Exercised script validation, conflicting edits, runtime logging and detach. |
| Documents | The editor previously read EDIFACT as UTF-8, and a failed file switch could display stale content. The editor now round-trips Latin-1 bytes, identifies the file that failed and supports reload after repair. Drafts have explicit save and discard actions. | Round-tripped an accented message, checked the exact bytes, rejected an invalid edit, then moved and restored the file while the app was open. |
| Workspace settings | Listener, identity and retention now have labelled controls. Shortening retention shows the scope before applying. Raw TOML remains available under Edit configuration file. | Rejected an invalid listener address, verified that the first retention click kept the previous value, then confirmed, reloaded and checked the persisted setting. |
| Clear traffic history | Traffic → Clear history… offers Last minute, Last hour, Last day and Last week. The partner selector includes removed partners with traffic. It defaults to the current partner in a partner view and to all partners elsewhere. The dialog fits at minimum size. | Tested all four inclusive cutoffs against controlled timestamps, including older and future records. Verified whole-exchange deletion by latest transfer, skipped pending deliveries and delays, cancellation, and persistence after reopening. |
| Keyboard and layout | Dialogs accept Escape when a dropdown is closed. The sidebar shows keyboard focus and truncates long labels with the full name on hover. | Exercised Escape from linked editors and select-all/paste in the source editor. Inspected compact dialog footers, labels and sidebar layout. |

## Native screenshots

At minimum size, the original inspector shared the window with the traffic list. The revised inspector displays message segments, with one action to return to the list.

[Original compact traffic view](audit/traffic-small-before.png) · [Revised compact inspector](audit/details-small-after.png) · [Time-scoped cleanup dialog](audit/cleanup-small-after.png)

## Test coverage

All 9 desktop tests and 13 core tests passed with local-listener tests enabled. All 6 release integration tests passed against real temporary folders and loopback HTTP. The native release smoke test confirmed a visible macOS window and delivery of both sample replies in desktop and headless modes. Formatting and strict Clippy checks passed.

To repeat the checks from the repository root on macOS, run the following commands.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo test --workspace --locked --offline -- --include-ignored
cargo build --release --locked --offline
python3 tests/local_instance.py --binary target/release/vendredi
python3 tests/release_macos.py target/release/vendredi
```

## Release observations

On the isolated repeat, the 24.34 MB release binary opened a visible macOS window in 248 ms. After one order and its replies, desktop resident memory reached 128.2 MB. The two five-second desktop idle samples used 11.55% and 13.71% CPU. Both desktop resource measures exceeded the [initial targets](../../IMPLEMENTATION.md). A follow-up stack sample found the UI thread mostly waiting, so the source of the intermittent CPU usage remains unresolved. Headless mode started in 20 ms, used 17.9 MB and recorded 0.0% idle CPU on the isolated repeat.

The visual review covers macOS Apple Silicon. Native Windows and Linux rendering, physical Intel Macs, and screen-reader navigation remain unverified in this audit.
