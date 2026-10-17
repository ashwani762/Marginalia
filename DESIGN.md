# Marginalia design system

Marginalia is used in two very different states of mind. Before and after a meeting the user is
*calm and exploratory* (browse, search, read). During a meeting they are *divided-attention and
anxious about missing something* (listen, jot, glance). One design system has to serve both, so the
rules below are organised around what attention is available, not around screens.

## 1. Principles (and the psychology behind them)

| # | Principle | Why it works | In Marginalia |
|---|---|---|---|
| 1 | **Status is always visible, within 400 ms** | Doherty threshold; Nielsen #1. People trust a recorder only if they can see it is recording and hearing. | Red dot + timer + level meter in the header at every width; a live caption line is always on screen while recording; every long job has a named state ("Finalizing transcript", "Writing summary", "Indexing"). |
| 2 | **One primary action per screen** | Hick's law (choice time grows with options); Von Restorff (the single distinct item is remembered). | One solid accent control per view. Everything else is tonal, outline or ghost. |
| 3 | **Calm during the meeting** | Attention is the scarce resource; peripheral information must not compete with the task. Apple "deference". | Notes are the hero and never lose their space. Transcript and Live Assistance live at the edge (side pane, or a caption dock when narrow), animate slowly, and never flash. |
| 4 | **Progressive disclosure** | Miller's 7±2 and cognitive load: show what is needed now, reveal depth on demand. | Model, GPU and context controls sit under *Advanced*. Source lists collapse to the best citation. |
| 5 | **Recognition over recall** | Nielsen #6. Names and previews beat dates and IDs. | Titles are generated from content. Suggested questions come from the user's own documents and meetings. Meetings and documents are searchable. |
| 6 | **Forgiveness** | Nielsen #3 and #9. Undo beats confirm dialogs because it does not interrupt the common case. | Delete is reversible for 8 s. Nothing the user typed is ever dropped. |
| 7 | **Familiar before clever** | Jakob's law: users spend most of their time in other apps. | Master-detail list, system theme by default, Ctrl+K command palette, standard shortcuts, Esc closes. |
| 8 | **Design the end of the meeting** | Peak-end rule: people judge an experience by its peak and its end. | Stopping hands off to an explicit wrap-up state, then reveals a clean, copy-ready summary. |
| 9 | **Trust is visual** | The product's promise is privacy; the interface must look like it keeps it. | Recording state is unmistakable; "on this device" is a quiet status, not a banner. No decorative glow competing with that signal. |
| 10 | **Accessible by construction** | WCAG 2.2 AA; Fitts's law. | All text ≥ 4.5:1, UI glyphs ≥ 3:1, targets ≥ 28 px (32 px for primary), full keyboard path, motion respects reduce-motion. |

## 2. Audit (what was wrong, how it was measured, what changed)

Evidence is from screenshots at 1440×984 and 2560×1036 plus computed contrast ratios.

| ID | Sev | Finding | Principle | Fix |
|---|---|---|---|---|
| A1 | High | "Models loaded" toast sat on top of *Add folder / Add files*; non-actionable. | Fitts; avoid interrupting for non-actionable news | Toasts anchor bottom-right; start-up success toast removed (status lives in the sidebar). |
| A2 | High | After Stop the header offered *Start transcribing* and the panel still said *Live* while the summary was being written. | Status visibility; error prevention | Explicit wrap-up state with named steps; Start disabled. |
| A3 | High | Below ~1500 px the transcript and level meters were hidden entirely, so nothing proved the app was listening. | Status visibility | Always-on live caption line and a compact meter. |
| A4 | High | When narrow, opening the transcript replaced the notes. | Calm; deference | Transcript becomes a drawer/dock; notes keep their space. |
| A5 | Med | A provisional line could remain under its own finalized line. | Correctness | Stale partials are discarded by timestamp. |
| A6 | Med | Two solid violet CTAs for one task on the welcome screen. | Hick; Von Restorff | Sidebar *Start recording* is tonal; one solid CTA per screen. |
| A7 | Med | Hero had a visible hard-edged disc, loose headline leading, and feature columns on different baselines. | Gestalt alignment; deference | Disc removed, 1.15 leading, columns top-aligned and equal. |
| A8 | High | Contrast: tertiary text 3.2–3.6:1 (graphite) and 2.8–3.4:1 (paper); white on primary 3.94:1; white on Stop 2.77:1; paper ochre 2.2:1, sage 3.0:1. | WCAG 1.4.3 / 1.4.11 | New token values (section 3). |
| A9 | Med | Eleven 20 px "xsmall" targets. | Fitts | Minimum 28 px; primary 32 px. |
| A10 | High | No keyboard shortcuts, no command palette. | Nielsen #7 | Shortcut map (section 5) and Ctrl+K palette. |
| A11 | Med | Settings required *Save*; dark mode was a boolean. | Jakob; immediacy | Light controls apply instantly; only model reloads confirm. Appearance is System / Light / Dark. |
| A12 | High | Delete was immediate and permanent. | Nielsen #3 | 8-second Undo. |
| A13 | Med | Summary repeated its own heading, listed empty "None identified" sections, spoke about "the transcript", and could not be copied. | Signal over filler | Cleaned output, neutral voice, Copy and Export. |
| A14 | Med | Titles were "Meeting · Oct 16, 10:38" with the time shown again on the right. | Recognition over recall | Generated titles; row shows title, one-line preview, relative time. |
| A15 | Low | Suggested questions were hard-coded and often irrelevant. | Recognition | Derived from the user's data. |
| A16 | Med | No search in the meetings list. | Recognition; scale | Search field with live filtering. |
| A17 | Med | Sidebar *New meeting* only cleared the screen and still needed a second click; it is now labelled *Start recording*. | Hick; fewer steps to the core task | Sidebar button and Ctrl+N create the meeting and start recording in one action. Ignored while a recording is live or wrapping up. |

## 3. Foundations

### 3.1 Colour roles

Roles, not hues: content colours are chosen for contrast first, then flavour. "Ink" variants are
text-safe; pigment variants are for fills, dots and illustration only.

| Role | Graphite (dark) | Paper (light) | Notes |
|---|---|---|---|
| canvas | `#080B12` | `#F6F3EE` | window background |
| raised | `#0D1119` | `#FBFAF7` | panels, sidebar |
| sunken | `#131826` | `#EDE7DD` | inputs, pills, hover |
| pressed | `#1A2130` | `#E4DCCF` | active rows |
| hairline | white 7 % | `#DAD2C5` | 1 px borders |
| content/primary | `#EDF0F7` (17:1) | `#151413` (17:1) | |
| content/secondary | `#9AA4B8` (7.4:1) | `#4A4642` (9:1) | |
| content/tertiary | `#8794AB` (≥ 5.2:1) | `#645E55` (≥ 4.7:1) | was 3.2:1 / 2.9:1 |
| accent fill | `#6A58F0` (white 4.9:1) | `#151413` (ivory 17:1) | buttons, selected |
| accent ink | `#A78BFA` (6.8:1) | `#9C442D` (≥ 5.2:1) | links, active labels, glyphs |
| success ink | `#34D399` | `#55624A` (≥ 5.3:1) | |
| warning ink | `#FBBF24` | `#80590E` (≥ 5.1:1) | |
| danger fill / ink | `#D33A3A` (white 4.7:1) / `#F87171` | `#A23B2A` | |
| speaker: you / others | `#67E8F9` / `#C4B5FD` | `#1F3FBF` / `#9C442D` | colour is never the only cue (label + initial) |

Rule: a chip or label may use a role colour only if the pairing in this table meets 4.5:1.

### 3.2 Type

One family per theme (Inter; Paper adds EB Garamond for display). Sizes in px / line-height.

| Style | Size / LH | Weight | Use |
|---|---|---|---|
| caption | 12 / 16 | 500 | timestamps, eyebrows (never below 12) |
| footnote | 13 / 18 | 400 | helper text |
| body | 14.5 / 22 | 400 | UI |
| writing | 16 / 26 | 400 | notes, enhanced notes, summary: surfaces people read for minutes |
| title-3 | 18 / 24 | 600 | panel titles |
| title-2 | 22 / 28 | 600 (Paper 500) | page titles |
| title-1 | 28 / 32 | 600 | meeting title |
| display | 36 / 41 | 600 | hero only, leading 1.15 |

Timecodes use a monospace face with tabular figures so digits do not jitter.

### 3.3 Space, shape, depth

* 4-pt grid: 4 · 8 · 12 · 16 · 24 · 32 · 48. Panel padding 16, page padding 24, list row 56 px.
* Radius: control 8, card 12, sheet 16 (Paper: 6 / 8 / 10).
* Elevation is three steps, not a continuum: *flat* (canvas), *raised* (hairline + soft shadow),
  *overlay* (toast, palette: larger shadow). No glow blobs behind content.
* Targets: ≥ 28 px interactive height, primary 32 px, hit area may exceed the glyph.

### 3.4 Motion

| Token | ms | Easing | Use |
|---|---|---|---|
| micro | 120 | ease-out | hover, press, toggles |
| standard | 200 | ease-in-out | state changes, tab switches |
| emphasis | 320 | ease-out-quint | entrances, new transcript text |
| page | 480 | ease-out-quint | route change, summary reveal |

Motion explains change (where something came from), never decorates. Live indicators pulse at
1.1–1.6 s periods; nothing loops faster. With reduce-motion on, everything snaps to its end state.

## 4. Components

* **Button.** Solid accent (one per screen) · tonal (accent 14 % fill, accent ink) · outline · ghost ·
  danger (only for the active Stop). Icon-only buttons always carry a tooltip.
* **Segmented control.** Min 28 px, 8 px radius, selected segment is sunken + primary text (not a
  second accent fill), used for 2–4 mutually exclusive views.
* **List row.** 56 px, title (body 600), one-line preview (footnote, secondary), trailing relative
  time (caption, tertiary). Selected = pressed surface + 2 px accent bar.
* **Chip.** Text in the role's ink colour on 14 % tint; status chips carry text, never colour alone.
* **Card.** Raised surface, hairline border, 12 px radius, 16 px padding.
* **Toast.** Bottom-right, 4 s, one line, optional single action (Undo). Used only for results the
  user did not directly watch happen, or that can be undone.
* **Empty state.** Icon · one-sentence title · one-sentence body · at most one action.
* **Live caption.** One line, secondary text, new words fade in over 320 ms, pinned above the
  footer while recording.

## 5. Interaction patterns

### Live meeting (calm layout)
1. **Header**: title · red dot + timecode · compact level meter · Transcript toggle · Stop.
2. **Notes** (left) and **Live Enhance** (right) share the canvas, equal height, 16 px type.
3. **Transcript side pane** (≥ 1500 px wide) or **caption dock** (narrower). The dock shows the
   latest two lines and expands to a drawer on demand; notes are never replaced.
4. **Live Assistance** sits at the top of the transcript pane/dock: one quiet status line when idle,
   cards only when there is something found.

### Wrap-up
Stop → header shows *Wrapping up* with the step names in order (Finalizing transcript → Writing
summary → Indexing) and a determinate-looking progress bar; Start is disabled; on completion the
summary fades in (page motion) and the title is replaced by a generated one if still default.

### Destructive actions
Delete hides the item immediately and shows *Meeting deleted · Undo* for 8 s; the row is removed
for real afterwards. No modal.

### Keyboard
| Keys | Action |
|---|---|
| Ctrl+N | Start recording at once |
| Ctrl+Enter | Start / stop recording |
| Ctrl+K | Command palette (meetings, documents, actions) |
| Ctrl+F | Search meetings |
| Ctrl+1 … 5 | Meetings · Knowledge · Ask · Stats · Settings |
| Ctrl+Shift+T | Toggle transcript |
| Ctrl+, | Settings |
| Esc | Close palette / drawer |

## 6. Accessibility checklist

* Contrast computed from the actual theme file values, every text role against all three base
  surfaces in both themes: lowest is 5.1:1 (Paper warning ink) and every role clears 4.5:1.
  Primary fill label 4.9:1 (Graphite) / 17.6:1 (Paper); danger fill label 4.7:1 / 6.3:1.
  Rerun after any token change.
* No information by colour alone (speaker label + initial, status text next to dots).
* Visible focus ring on every control; logical tab order; palette is fully keyboard driven.
* Reduce-motion respected by the animation layer.
* Compact layout (< 1100 px) keeps every action reachable and labelled via tooltip.

## 7. How to verify

1. Screenshot each route at 1440×984, 1100×720 and 2560×1036 in both themes.
2. Recompute contrast for any changed token (target 4.5:1 text, 3:1 glyphs).
3. Walk the keyboard map without the mouse.
4. Record 60 s with speech: confirm caption line, meter, wrap-up states, summary reveal, Undo.
