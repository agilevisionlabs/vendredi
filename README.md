# Vendredi

An EDI simulator for developers: a fake trading partner that runs on your machine, receives the EDIFACT documents your integration sends, and replies with acknowledgements and response documents.

> Status: early design. Nothing is implemented yet; this document defines what the project is and is not.

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
- Scenarios are declarative: rules match inbound messages and list the actions to take. Logic that goes beyond matching, such as building a reply from the inbound message, is JavaScript run by an embedded **Boa** engine.
- Transports sit behind a common interface so AS2, FTPS and SFTP can be added without touching the core.
