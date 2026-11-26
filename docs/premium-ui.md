# Making the desktop feel premium

What it takes for Vendredi's desktop to feel premium and attentive ("white glove"), based on an audit of `crates/app/src/gui` on `cases-and-x12`. The items are ordered by impact. Each one names what is wrong today, what good looks like, and the rough size of the work (S under a day, M one to three days, L a week).

## Where it stands

The bones are good. Vendredi starts in about 0.3 s, ships a sensible sample workspace, uses its own type scale (Inter and IBM Plex Mono), and the core loop works: watch traffic, inspect a message, switch a case.

What keeps it from feeling premium is not one big gap but many small ones:

- **The app is a configuration editor more than a guide.** The first launch silently creates `~/Vendredi` and drops you into an empty traffic table. File paths are typed by hand, `vendredi.toml` is edited as raw text, and errors are printed as `anyhow` chains.
- **Visual rhythm is improvised view by view.** This session alone fixed a staircase header, mismatched card paddings, short pickers and two button heights, each in a different file. Without shared layout components, the next view will drift again.
- **Feedback is a line of text in the footer.** Saves, switches, imports and failures all report there, where nobody looks.
- **It does not behave like a Mac app.** There is no menu bar, no shortcuts beyond Escape, no file dialogs, no drag and drop and no remembered window size.

## 1. Onboarding: the first five minutes

| Today | Premium |
|---|---|
| The first launch creates `~/Vendredi` without asking and shows "Waiting for the first interchange." | A welcome screen with three choices: **Try the demo** (today's sample), **Create a workspace** (choose a folder) and **Open a workspace**. |
| The partner, its channels and case are configured in a dialog of raw fields. | A three-step setup: who the partner is (name, standard, identity), how it talks (HTTP path or folder, with the resulting URL shown and copyable), and how it behaves (pick a case from the library). |
| "Send sample order" is a small link in the empty table. | A guided first exchange: a checklist card on the traffic page (send a sample, open the reply, switch to "Reject every order", send again) that ticks itself off from engine events and then disappears. |
| No explanation of EDI concepts in the app. | One-line explanations where a term first appears: interchange, acknowledgement, case and rule. Hover text in the inspector names each segment (`BGM`, `UNB`, `ISA`). |

Each of these builds on code that exists: `sample()`, the case library, `Command::Sample` and the event stream. **Size: L** for the welcome flow and partner setup, **M** for the checklist.

## 2. A shared layout system

Most alignment bugs this session came from each view placing its own frames, rows and spacing. The fix is a few components that every view uses:

- **Page:** title row, optional subtitle and one content width, with the same margins on every page.
- **Card and section:** one frame (border, radius, padding) and a section heading row with optional small actions. The Cases page already has `case_frame`, `title_row` and `section_row`; they should move to `theme.rs` and replace the remaining `form::card` and ad-hoc frames.
- **Spacing scale:** 4, 8, 12, 16, 24 and 32 px, used as named constants instead of the `add_space(6.)` and `add_space(10.)` scattered today.
- **Control heights:** `BUTTON` (36) and `SMALL_BUTTON` (28) exist. Text fields, pickers (still some `ComboBox` with their own height), checkboxes and links in rows should follow them.
- **Layout tests:** the measuring helper from `create_duplicate_and_delete_cases` becomes a shared assertion (`same_row`, `same_height`, `same_edge`) and runs over every view at three window sizes. Alignment regressions then fail in CI instead of in screenshots.

**Size: M.** This is the foundation for everything below, so it should come first.

## 3. Feedback and states

- **Toasts instead of the footer.** "Rule saved", "Supplier now uses case reject-orders" and "Imported 3 cases" appear briefly near the top right. Undo goes in the toast for destructive actions (delete case, remove partner, clear history) instead of a confirmation dialog first.
- **Errors in plain language.** Map the common failures to a sentence and a fix: the port is in use (offer another), a file is missing (show the path and a **Locate…** button), the workspace is locked by another instance, a script has a syntax error (open the editor at the line). Keep the full chain behind **Details**.
- **Loading, empty and error states for every view.** Traffic, Cases, Scripts and Documents each need a designed empty state with one clear action. Only the case list has one today.
- **Live status that means something.** The "Working / Paused" pill is a start. Show activity in it, such as a soft pulse when a message arrives and a counter of pending deliveries, so the app looks alive when it is.
- **Motion with restraint.** Fade dialogs in, slide toasts, and briefly highlight the row of a newly arrived transfer. egui has `animate_bool` and `animate_value_with_time`, and none of it is used today.

**Size: M.**

## 4. Behave like a native app

- **Menu bar** with File (New workspace, Open workspace, Open recent, Reveal in Finder), Edit, View (the pages, zoom), Window and Help (guide, release notes, report a problem). eframe does not provide one, so use the `muda` crate.
- **Keyboard:** ⌘N new partner or case, ⌘O open workspace, ⌘F search traffic, ⌘1–⌘5 for the pages, ⌘S in editors, ⌘⏎ to confirm dialogs, arrow keys in the traffic table and case list, and Delete for removal. Show the shortcuts in tooltips and menus.
- **File dialogs and drag and drop:** the rule editor's "Document file" and "JavaScript file" fields, the import source and the send dialog should have **Choose…** buttons (the `rfd` crate). Dropping an `.edi` file on the window should offer to send it, and dropping a case JSON should offer to import it.
- **Remembered state:** window size and position, the last page, the selected partner and the inspector split. Turn on eframe's `persistence` feature.
- **Idle cost:** already fine. With no input the window does not repaint and uses 0 % CPU. The 6 % in the release check is measured just after a sample exchange, while traffic is being drawn.
- **Accessibility:** keyboard focus rings that are visible, labels on every icon-only control, and AccessKit names for the custom-painted traffic rows and navigation items. Contrast is already fine: muted text on panels is about 6:1, above the WCAG AA minimum of 4.5:1. Dark mode will need the same check.

**Size: M** for menus, shortcuts and persistence, **S** each for file dialogs and drag and drop.

## 5. Visual craft

- **Icons.** Replace the text glyphs (→, ⏸, ▷, ⏷, ●) with one icon set, such as Phosphor or Lucide, loaded as a font. Use them in navigation, the traffic direction chip and buttons.
- **Dark mode** that follows the system. Every colour is already a named constant in `theme.rs`, so this means a second palette and replacing the hard-coded `Theme::Light`.
- **Typography roles.** Name the sizes in use (page title, section, body, caption, code) and stop passing raw sizes such as `bold(17.)` and `.size(11.)` in views.
- **Colour roles.** Use colour for meaning only: blue for the primary action and selection, amber for attention, red for destruction and green for healthy. Muted grey is overused today, so hierarchy should come from weight and size instead.
- **A recognisable mark.** The app icon exists. Add it, small, to the sidebar header and the welcome screen.

**Size: M**, with dark mode on its own about **M**.

## 6. Editing without raw files

- **A visual case editor** that covers samples, initiates and expectations, not only rules. Today only rules have a form.
- **The JavaScript editor:** syntax highlighting (the search highlighter already tokenises EDI), line numbers, inline errors from `script::validate` with the line marked, and a **Run against the selected message** button that shows what the script would send.
- **Documents:** show an EDI document as segments with element names next to its raw text, which the inspector already does for traffic.
- **Raw TOML editing** stays, behind Settings → Advanced, and no flow should need it.

**Size: L.**

## 7. White-glove touches

These cost little and are what people remember:

- **The inspector explains itself:** "This ORDRSP confirms order PO42 from the 10:31 ORDERS", with a link to that message.
- **Copy affordances:** a copy button on the receive URL, the raw message, a segment, a transfer ID and a ready-made `curl` command for the partner's HTTP channel.
- **Status that answers "is it working?":** the partner page shows the last message received and sent, and any delivery failing right now.
- **Release notes on first launch after an update**, and a link to the guide from every empty state.
- **"Report a problem"** that bundles the version, platform and the last error, with a preview so nothing private is sent unseen.

**Size: S** each.

## Suggested order

1. **Layout system and layout tests** (section 2). Otherwise every later screen re-creates the alignment problems.
2. **Native behaviour** (section 4): menus, shortcuts, persistence and file dialogs. These are visible on every use and mostly mechanical.
3. **Onboarding** (section 1): welcome, partner setup and the first-exchange checklist.
4. **Feedback and states** (section 3): toasts with undo, plain errors and empty states.
5. **Visual craft** (section 5): icons, roles and dark mode.
6. **Editors** (section 6) and the white-glove touches (section 7), in the order users ask for them.

Steps 1 and 2 take about two weeks for one developer, and all six about six to eight weeks.

## How to tell it is done

- A new user installs the app and finishes a first exchange and a case switch in under three minutes, without reading the README.
- No flow requires typing a file path or editing TOML.
- Layout tests cover every page at small, medium and large window sizes.
- The app remembers where you left it.
- Every action either succeeds with a toast or fails with a sentence that says what to do next.

## Status

Implemented on `cases-and-x12`, each in its own commit with tests:

- **Layout system:** shared card, row and spacing parts; two control heights; an AccessKit-based layout test over every page at three window sizes, plus the welcome screen.
- **Native behaviour:** remembered window and last page; keyboard shortcuts; macOS menu bar; Choose… file dialogs; drag and drop; AccessKit tree for screen readers. Idle CPU measures 0 %.
- **Onboarding:** welcome screen with demo, create and open, and recent workspaces; three-step partner setup; the getting-started steps on the traffic page; segment explanations on hover.
- **Feedback:** toasts, with Undo for deleting a case, rule, sample or partner; plain-language errors and a redesigned "couldn't open" screen; empty states for traffic, scripts and documents; arrival glow, status pulse and a count of replies still sending.
- **Visual craft:** Phosphor icons; light and dark palettes that follow the system and pass WCAG AA; named type roles; the app's mark in the sidebar and on the welcome screen.
- **Editors:** raw TOML moved under Settings → Advanced; JavaScript highlighting, line numbers and a live syntax check that names the line; running a script on the selected message without sending; a segment guide beside documents; adding and removing samples, recording their replies as the expected result, and running a case's checks.
- **White-glove touches:** a sentence in the inspector on how each transfer fits its exchange; partner status with copyable URL and `curl`; What's New after an update; Report a Problem with a full, editable preview.

Also done since:

- Editing a case's expected replies by hand (they open in Documents) and its partner-initiated sends.
- Field names for common segments in the document guide and the inspector.
- The grey review: introductions and other sentences meant to be read use the darker `prose` style; faint grey is kept for ids, times, counts and short hints.
- A visual check. A test renders pages to PNG in software (`VENDREDI_SNAPSHOTS`). It found icons drawn from the wrong font, samples listed under the wrong heading, a clipped filter and touching buttons, all fixed.

Checked since:

- The macOS menu bar in the running app. `VENDREDI_MENU_DUMP=1 vendredi gui <workspace>` prints the menu the app installed, with its shortcuts, and quits. All five menus and their items are there. The check found About, Hide and Quit named after the lowercase executable, which is fixed.
- The platform-specific code, by reading it: Windows and Linux "Show in Finder" was wrong (Explorer's `/select,` must be one argument, and `xdg-open` cannot reveal a file); both are fixed.

- Linux, in an Ubuntu 24.04 container on Apple silicon, as CI's `linux-arm64` job: core tests, the desktop and listener tests, the build and the 13 end-to-end tests all pass.
- Windows, by cross-compiling for `x86_64-pc-windows-gnu` with MinGW in the same container: it builds without warnings. That check found that New partner and Import cases had no shortcut outside the macOS menu; ⇧⌘N and ⌘I now work in the window on every platform.

- Windows, running: the Windows build's tests ran under Wine in an x86-64 Linux container. Core (41), the app's unit tests and the desktop and listener tests (27) all pass. The run found a real bug, missing-file messages dropping the drive letter of Windows paths, and tests that assumed Unix paths and file locking; all fixed.

- Every platform natively, in CI: the Verify workflow built, tested and packaged macOS (Apple silicon and Intel), Windows (Microsoft's toolchain) and Linux (x64 and ARM64) on `main` and again for the `v0.2.0` tag, which published Vendredi 0.2.0 with signed and notarized Mac apps.

Nothing in this plan is left open.
