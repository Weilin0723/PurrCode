# PurrCode native IDE — UI gap analysis

**Scope:** `crates/purrcode-ide` (egui/Rust desktop application).
**Date:** 2026-08-05.
**Method:** static read of the crate against two external reference rule-sets. Every gap below was confirmed by reading the cited source; nothing here is carried over from a summary on trust.

---

## Reference provenance

Both references were reported reachable by the collection pass, and both digests were supplied to this audit as evidence.

| Reference | Status | How it was used here |
| --- | --- | --- |
| `github.com/nextlevelbuilder/ui-ux-pro-max-skill` | Reachable. A local mirror exists at `/private/tmp/claude-501/-Users-jackzhang-Documents-GitHub-PurrCode/531e3262-1823-409a-bdca-e452e9107810/scratchpad/uxpm/` and was read directly for the rules cited below (`quick-reference.md:10`, `:26`, `:104`). | Cited by rule id and line where I read the file myself; otherwise cited from the supplied digest. |
| `github.com/pbakaus/impeccable` | Reachable per the collection pass. **No local mirror; I did not re-fetch it in this run.** | Cited from the supplied digest only. Where I lean on it I say "impeccable (per supplied digest)". No rule is invented and attributed to it. |

Two conventions are used for anything that is neither repo's rule:

- **[craft]** — well-established interface craft, not attributable to either reference.
- **[taste]** — a preference, not a defect. Marked low, and called out as taste.

---

## Executive summary

The design system in this crate is genuinely good and genuinely under-used. `theme.rs` is a 24-role token set with three appearances and eleven contrast unit tests; `app/primitives.rs` is a real second-order component library with a four-tone button family, full state coverage, focus rings, and layout invariants proved by tests. The problem is distribution: `primitives::` is imported by exactly **one** file outside itself (`settings.rs`, 105 references; `code.rs`, 1). Every other surface — welcome, sidebar, workbench, dock, editor, terminal — hand-rolls its own buttons, rows, headings, and cards. The result reads as two products in one window: a settings dialog built to a standard, and a workbench assembled ad hoc.

The five highest-impact gaps are not style drift, though. They are:

1. **A source file can be edited and the edits silently discarded.** The centre editor is a live, writable `TextEdit`; nothing in the crate ever writes a file back to disk; closing the tab drops the buffer without a prompt.
2. **The editor's line numbers desync from its code** — the gutter and the text are two independent scroll areas, contradicting the doc comment directly above them.
3. **The command pill is painted over the branch chip at the minimum window size**, and the guard written to prevent exactly that is mathematically unreachable.
4. **Agent prose runs to ~150 characters per line** on the product's primary surface, with no measure cap at all.
5. **The agent empty state forces a 520pt column into a panel that can be 320pt wide** — the same class of egui "container claims more than it has" bug as the wordmark defect that was just fixed.

Against the impeccable triage order (per supplied digest: *broken/data-loss first, then missing non-happy states, then hierarchy and drift*), gaps 1–3 are category one and should gate any release. Against the ui-ux-pro-max review gate ("a change you have not looked at is not finished"), gaps 2, 3, 4, and 5 can only be *proved* on screen — the verification plan at the end says exactly how.

**What is working, and should not be touched:** the token contrast test suite (`theme.rs:525-700`); the state coverage in `code.rs` change review (loading / never-checked / zero / populated, all four distinguished); the error taxonomy in `errors.rs` (every notice carries headline + impact + next step); the composer's IME preedit guarding (`workbench.rs:648-660`); the transcript's fixed height reservation, which deliberately holds the reading position still across a run (`workbench.rs:200-215`); and the whole of `primitives.rs`.

---

## Gap table

| # | Gap | Severity | Surface | Source of the rule | Effort |
| --- | --- | --- | --- | --- | --- |
| 1 | Editor edits are unsaveable and are discarded without a prompt | **High** | Code editor | impeccable triage rank 1; uxpm forms rule | M |
| 2 | Line-number gutter scrolls independently of the code | **High** | Code editor | [craft] | S |
| 3 | Command pill occludes the branch chip at min window width; its collision guard is dead code | **High** | Application bar | impeccable "nothing occludes" | S |
| 4 | Agent transcript has no measure cap (~150ch lines) | **High** | Workbench transcript | uxpm `line-length` (`quick-reference.md:104`); impeccable measure rule | S |
| 5 | Agent empty state forces a 520pt column into a 320pt panel | **High** | Workbench empty state | impeccable "nothing overflows"; uxpm no-horizontal-scroll | S |
| 6 | No visible keyboard focus outside settings; the theme's focus ring is on the wrong egui slot | **Medium** | Whole app | uxpm `focus-states` (`quick-reference.md:10`); impeccable seven states | L |
| 7 | Explorer and search rows are content-width, so hit target and hover fill are ragged | **Medium** | Sidebar | uxpm touch/hit-target + states; [craft] | S |
| 8 | The welcome card recedes instead of lifting — it uses the chrome token as its fill | **Medium** | Welcome | impeccable "declare elevation once"; the crate's own `theme.rs:670` test | S |
| 9 | Two primary-button languages; the start-screen CTA is a 30%-alpha wash | **Medium** | Welcome, sidebar | uxpm one-primary-CTA + hierarchy; impeccable drift classification | M |
| 10 | Dead and lying controls: a no-op "Retry failed", a terminal cwd `if` with identical branches | **Medium** | Dock | uxpm "controls that look tappable but do nothing" | S |
| 11 | Shipped "no folder opened" panel hides missing recents; the better implementation is dead code | **Medium** | Welcome navigation | impeccable "empty states must distinguish" | S |
| 12 | Three tab strips, three visual languages, in one window | **Medium** | Editor, dock, code panel | impeccable drift at the narrowest correct level | M |
| 13 | `"●"` text bullets used as status markers, in a crate whose icon module forbids it | **Medium** | Dock | uxpm icon discipline; impeccable "icons are drawn, never typed" | S |
| 14 | Type ramp not enforced: 33 literal `.size()` sites against a declared 5-step scale | **Medium** | Whole app | uxpm fixed type ramp; impeccable type-scale rule | M |
| 15 | `section_heading`'s trailing gap becomes horizontal padding inside a row | **Low** | Dock, sidebar | [craft] | S |
| 16 | Eight raw corner radii against three radius tokens | **Low** | Welcome, dock, code, errors | uxpm semantic-tokens-only | S |
| 17 | The terminal grid has no surface of its own | **Low** | Terminal | impeccable "theme the browser's own surfaces" (analogue) | M |
| 18 | Welcome card centred against an assumed height it does not have | **Low** | Welcome | [craft] | S |
| 19 | Four different empty-state shapes | **Low** `[taste]` | Dock, workbench, settings, sidebar | impeccable consistency | S |

---

## Gap detail

### 1. Editor edits are unsaveable and are discarded without a prompt — **High**

**Observed.** `crates/purrcode-ide/src/app/editor.rs:258-274` mounts a fully writable editor and commits every keystroke back into the in-memory buffer:

```rust
let _ = ui.add(
    egui::TextEdit::multiline(&mut content)
        .code_editor()
        …
);
…
if let Some(file) = self.open_files.get_mut(index)
    && file.body.as_ref() != Ok(&content)
{
    file.body = Ok(content);
    file.modified = true;
    self.dirty.insert(file.path.clone());
}
```

The tab then advertises the unsaved state with a dot (`editor.rs:383-386`). But `fs::write` appears exactly once in the entire crate, in `welcome.rs:110`, and it writes the recent-workspaces JSON. There is no ⌘S handler, no save action, no autosave. And `crates/purrcode-ide/src/app/code.rs:855-865` throws the buffer away unconditionally:

```rust
pub(crate) fn close_file(&mut self, index: usize) {
    if index < self.open_files.len() {
        let path = self.open_files[index].path.clone();
        self.open_files.remove(index);
        self.dirty.remove(&path);
```

`code.rs:820-823` (`× Close all`) does the same for every open file at once, and does not even clear `self.dirty`.

**Expected.** impeccable's triage order (per supplied digest) puts "broken or blocked tasks, data loss, misleading state" at rank 1, above every other category. ui-ux-pro-max's forms rules require confirming before dismissing a surface with unsaved changes, and offering undo for destructive actions. The dirty dot is *misleading state* in the precise sense both use: it promises there is something to save when nothing can be.

**Fix.** Pick one of two, and do it before shipping:
- (a) Make the centre editor read-only — `TextEdit::multiline(&mut content.as_str())` or `.interactive(false)` — and delete the `modified` / `dirty` bookkeeping at `editor.rs:271-274`. This is honest and small.
- (b) Add a real save path: ⌘S writing `file.body` to `file.path`, clearing `dirty` on success and raising a notice on failure (the `errors.rs` taxonomy already has a shape for it), plus a confirm dialog in `close_file` when the path is in `self.dirty`.

Do not ship (a) and (b) blended.

---

### 2. Line-number gutter scrolls independently of the code — **High**

**Observed.** The doc comment at `crates/purrcode-ide/src/app/editor.rs:194-196` says:

> "The gutter and the text live in one horizontal row inside the same vertical scroll area so they scroll together."

The code does the opposite. `editor.rs:215` and `editor.rs:233` create two sibling `ScrollArea::vertical()`s with different `id_salt`s (`"editor_gutter"`, `"editor_code"`) inside one `ui.horizontal`. Each owns its own scroll offset. Scrolling the code moves the code only; the numbers stay where they were.

The same block also guesses the gutter width at `editor.rs:212`:

```rust
let gutter_width = 20.0 + (line_count.to_string().len() as f32) * 8.0;
```

8pt per digit is a constant, not a measurement of the live monospace face — contrast `code.rs:684`, which measures the widest line number from the actual font before sizing its diff gutter.

**Expected.** [craft] Line numbers that do not track their lines are worse than no line numbers: they are confidently wrong. This is not in either reference's rule list because no web checklist covers a code gutter.

**Fix.** Collapse to one `ScrollArea::vertical` wrapping a single `ui.horizontal` that contains both columns, so one offset drives both. While there, replace the `* 8.0` with `ui.fonts_mut(|f| f.glyph_width(&mono, '0'))` the way `code.rs:684` already does.

**Only confirmable visually** — the desync is a runtime scroll behaviour, not a static property.

---

### 3. Command pill occludes the branch chip at minimum window width — **High**

**Observed.** `crates/purrcode-ide/src/app/bar.rs:137-145`:

```rust
let width = (bar.width() * 0.32).clamp(200.0, 440.0);
let rect = Rect::from_center_size(…, Vec2::new(width, 24.0));
// Never let the pill collide with the workspace name on a narrow window.
if rect.width() < 200.0 {
    return;
}
```

`clamp(200.0, 440.0)` has a floor of exactly 200, so `rect.width() < 200.0` can never be true. The guard is unreachable and the pill is always painted, over the centre of the bar, *after* the left cluster (`bar.rs:107-109` — "Drawn last so it sits on top of the row it is centred in").

The collision is not hypothetical at the minimum window size. `lib.rs:47` sets `.with_min_inner_size([900.0, 600.0])`. At 900pt the pill is `clamp(288, 200, 440)` = 288pt wide, centred at x=450, so it spans **306..594**. The left cluster is: 70pt traffic-light inset (`bar.rs:38`, `TRAFFIC_LIGHT_INSET` 78 − 8) + a 30pt brand badge + the 20pt wordmark + 8pt + a 13pt folder glyph + the repo name in strong body + 4pt + the branch chip (`Margin::symmetric(8,2)` around an 11pt glyph and label). For this repository ("PurrCode", branch `v1.0/feat-adding-IDE`) the branch chip alone lands well inside 306..594.

**Expected.** impeccable (per supplied digest) treats occlusion as a structural breakage of its own: "no text is painted under an opaque layer or a second text run", checked at every supported width. The pill's fill is opaque (`background_raised` / `surface_hover`), so it does not merely crowd the chip — it hides it.

**Fix.** Measure the left cluster's right edge and the right cluster's left edge (both are already laid out before the pill draws), and skip the pill — or shrink it and re-centre it in the actual free span — when the free span is narrower than the pill. Alternatively drop the pill below 1100pt of window width and leave ⌘P as the only route, which is what the tooltip already says.

**Only confirmable visually** at exact widths.

---

### 4. Agent transcript has no measure cap — **High**

**Observed.** `crates/purrcode-ide/src/app/workbench.rs:216`:

```rust
let measure = ui.available_width();
```

and that value is handed straight to `self.message(ui, message, measure)` → `markdown::render(ui, tokens, source, measure)` (`markdown.rs:619-626`), which only ever floors it (`measure.max(MIN_MEASURE)`). There is no ceiling anywhere in the path.

At the default window (`lib.rs:46`, 1480pt) with the rail (60) and sidebar (268) open and no aux panel, the transcript column is roughly 1100pt. At `TYPE_BODY` = 14pt proportional that is on the order of 150 characters per line. The user's own bubbles are capped, but only at `(row * 0.72)` — about 110 characters at the same width.

**Expected.** ui-ux-pro-max, read directly from the local mirror, `quick-reference.md:104`:

> `line-length` - Limit to 65-75 characters per line

impeccable (per supplied digest) gives 45–75ch, "hard problem past ~80". Both are unambiguous, and this is the surface the product is named for.

**Fix.** Clamp once, where the measure is taken: `let measure = ui.available_width().min(READING_MEASURE);` with `READING_MEASURE` a new `theme.rs` constant around 680–720pt (≈ 72ch at 14pt with the shipped UI face — measure it with `fonts.glyph_width` rather than guessing). Left-align the capped column rather than centring it, so the composer below and the prose above keep one left edge. Code blocks and diffs should keep the full width; only prose is capped.

---

### 5. Agent empty state forces a 520pt column into a 320pt panel — **High**

**Observed.** `crates/purrcode-ide/src/app/workbench.rs:82`:

```rust
ui.set_max_width((ui.available_width() * 0.86).max(520.0));
```

`.max(520.0)` turns a *maximum* into a *minimum*: whenever the available width is under about 605pt, the column is set to 520pt regardless of the space it has. The agent surface is not always in the wide centre pane — `mod.rs:1882` routes `AuxView::Agent` to `agent_surface(ui)` inside the right auxiliary panel, whose `width_range` is `320.0..=720.0` (`mod.rs:1461`). Dragged to 320pt, the empty state's headline, subtitle, and three starter cards are laid out into 520pt inside a ~300pt space and are clipped by the panel.

This is exactly the class of defect that produced the just-fixed wordmark bug: an egui sizing call that claims more than the container has, invisible in source, obvious on screen. Two related instances worth checking in the same pass:
- `welcome.rs:223` `ui.set_max_width(420.0)` inside a frame with `Margin::symmetric(36, 30)` gives a 494pt card; the central pane is only 572pt at the 900pt minimum window and shrinks below the card's width as soon as the sidebar is dragged past ~330pt.
- The starter cards themselves (`workbench.rs:141-170`) rely on an inner `Layout::right_to_left` to claim full width — correct, but it means the card's width is determined by a nested layout rather than by the frame, so it inherits the 520pt above it.

**Expected.** impeccable (per supplied digest): "no content renders wider than its container or forces a horizontal scrollbar". ui-ux-pro-max: no horizontal scroll at any tier; content reflows rather than shrinks.

**Fix.** `ui.set_max_width(ui.available_width().min(520.0))` — cap, don't floor. Then reflow: below ~420pt the brand-badge + eyebrow row and the three starter cards should stack rather than being clipped.

**Only confirmable visually** — drag the aux panel to its 320pt minimum with the agent in it.

---

### 6. No visible keyboard focus outside settings — **Medium**

**Observed.** Two independent failures compound.

*(a) The theme's focus ring is attached to the wrong egui slot.* `theme.rs:325-330`:

```rust
// Focus must be visible, not implied by a colour shift (PRD §27).
visuals.widgets.open.bg_fill = self.surface_hover;
visuals.widgets.open.weak_bg_fill = self.surface_hover;
visuals.widgets.open.bg_stroke = Stroke::new(2.0_f32, self.accent_primary);
```

egui does not use `widgets.open` for focus. `egui-0.33.3/src/style.rs:1180-1191` resolves widget visuals as:

```rust
} else if response.is_pointer_button_down_on() || response.has_focus() || response.clicked() {
    &self.active
```

So a focused standard widget renders `widgets.active` — a **1px** accent stroke over `surface_active` (`theme.rs:319-323`) — which is byte-identical to the pressed state. `widgets.open` is reached only for open menus and combo popups. The intent in the comment is not implemented.

*(b) Every hand-painted control ignores focus entirely.* Each of these reads only `response.hovered()` and paints nothing for focus: `icons.rs:679` (`ghost_button`), `icons.rs:712` (`rail_item`), `workbench.rs:681` (`send_button`), `workbench.rs:714` (`bypass_chip`), `navigation.rs:253` (`session_row`), `code.rs:572` (`changed_file_row`), `editor.rs:309` (`tab`), `bar.rs:146` (`command_pill`), `bar.rs:531` (`dock_status_item`).

The only real focus rings in the product are the six hand-painted ones — `primitives.rs:629`, `settings.rs:515`, `settings.rs:741`, `navigation.rs:411` — all 2px at `rect.expand(1.0)`, `StrokeKind::Outside`. They are correct. They are also confined to settings and the two search fields.

**Expected.** ui-ux-pro-max, read from the local mirror, `quick-reference.md:10`:

> `focus-states` - Visible focus rings on interactive elements (2–4px; Apple HIG, MD)

Accessibility is one of only two CRITICAL categories in that rule-set, and "removing focus outlines" is listed as a top anti-pattern. impeccable (per supplied digest) requires all seven states — default, hover, focus, active, disabled, loading, error — on every interactive component.

**Fix.** Extract the ring that `primitives.rs:629` already draws into a single `theme::focus_ring(painter, rect)` helper, and call it from each of the nine hand-painted controls above. Separately, either move the 2px stroke from `widgets.open` to `widgets.active` (accepting that pressed and focused then look alike) or leave `active` at 1px and accept that only hand-painted controls carry a real ring — but fix the comment either way, because right now it documents behaviour that does not exist. Effort is L only because it touches nine call sites; each one is three lines.

---

### 7. Explorer and search rows are content-width — **Medium**

**Observed.** `crates/purrcode-ide/src/app/code.rs:127-147` (directories) and `code.rs:166-200` (files) build each tree row as:

```rust
let response = ui
    .horizontal(|ui| { … })
    .response
    .interact(Sense::click());
```

`ui.horizontal`'s response rect shrink-wraps its content, so the clickable region of a tree row is only as wide as `indent + icon + filename`. Everything to the right of the name — most of a 268pt column for a short filename — is dead. There is also no hover fill and no cursor change anywhere in either branch: nothing tells the user the row is a target before they hit it.

`navigation.rs:465-489` (search results) uses the same pattern and then paints the hover fill over `response.rect`, so the highlight is as wide as the matched text — a different width on every row, producing a ragged stack of half-width bars.

Both sit in the same 268pt column as `navigation.rs:261-263` and `code.rs:573`, which do it correctly:

```rust
let width = ui.available_width();
let response = ui.allocate_response(egui::vec2(width, crate::theme::ROW_HEIGHT), Sense::click());
```

**Expected.** ui-ux-pro-max requires extending the hit area beyond the visual bounds when the mark is smaller than the target (`quick-reference.md:26`), and requires hover as one of the six mandatory states. Three list treatments in one column is also drift in impeccable's "one-off implementation" class (per supplied digest) — the fix is to promote the shared component, not to patch each row.

**Fix.** Give `render_tree_level` and the search results the same `allocate_response(vec2(available_width, ROW_HEIGHT), Sense::click())` + `surface_hover` + `PointingHand` treatment that `session_row` and `changed_file_row` already use. Better: lift that into one `row(ui, tokens, height, selected) -> Response` helper next to `primitives::list_row` and have all four call it.

---

### 8. The welcome card recedes instead of lifting — **Medium**

**Observed.** `crates/purrcode-ide/src/welcome.rs:217-221`:

```rust
egui::Frame::new()
    .fill(tokens.background_secondary)
    .stroke(egui::Stroke::new(1.0_f32, tokens.border_subtle))
    .corner_radius(12)
    .inner_margin(egui::Margin::symmetric(36, 30))
```

`background_secondary` is documented in `theme.rs:129` as "The chrome: rail, sidebar, title bar, status bar, tab strip" — and the crate's own test at `theme.rs:670-684` asserts:

```rust
relative_luminance(tokens.background_secondary) < relative_luminance(tokens.background_primary),
"the chrome must be darker than the canvas it frames"
```

In Dark, `background_secondary` is `#090e12` and the canvas it sits on is `#0f1418`. So the welcome card is a *darker hole punched into the canvas*, not a raised surface. It is also a fourth card recipe: `tokens.card()` (`theme.rs:271-278`) is `background_raised` + hairline + `RADIUS_CARD` (10) + `Margin::symmetric(10, 8)`, and is used at exactly two sites in the whole application.

**Expected.** impeccable (per supplied digest): "declare elevation once" — pick a border or a shadow, keep card radii coherent, and do not hedge on the depth system. ui-ux-pro-max: consistent elevation scale, semantic tokens only inside components.

**Fix.** `.fill(tokens.background_raised)` and `.corner_radius(theme::RADIUS_CARD)`. Keep the generous `Margin::symmetric(36, 30)` — a start-screen card legitimately wants more air than an inline card — but derive it from the spacing scale rather than as a pair of literals.

---

### 9. Two primary-button languages; the start-screen CTA is a 30%-alpha wash — **Medium**

**Observed.** `welcome.rs:263-273` and `navigation.rs:517-527` are byte-identical duplicates of each other:

```rust
.fill(tokens.accent_primary.linear_multiply(0.30))
.stroke(egui::Stroke::new(1.0_f32, tokens.accent_primary.linear_multiply(0.7)))
.corner_radius(8)
.min_size(Vec2::new(width, 34.0))
```

`primitives.rs:562` (`Tone::Primary`) is a solid `accent_primary` fill with `accent_on` text at `RADIUS_CONTROL` and height 28. Same role, three different appearances.

The mechanism matters: `ecolor-0.33.3/src/color32.rs:330-338` implements `linear_multiply` as `Rgba::from(self).multiply(factor)`, which scales **all four channels including alpha**. So `.linear_multiply(0.30)` is not "a dimmer accent" — it is a 30%-alpha accent. The product's only start-screen call to action is a translucent wash whose composited fill works out to roughly `rgb(31,111,146)` over the card. That still clears AA (≈5.0:1 against `text_primary`, ≈3.5:1 as a UI surface against the card), so this is a **hierarchy and consistency** finding, not a contrast failure — but it is why the CTA reads as secondary chrome instead of as the one thing to do on the screen.

The same mechanism produces translucent notice cards at `errors.rs:324-325` (`accent.linear_multiply(0.10)` / `(0.55)`), which is precisely what `theme.rs:260-263` documents `tint()` as existing to avoid:

> "A surface tinted with `color`… **Opaque**, so it looks the same whatever it is drawn over."

Thirteen ad-hoc `linear_multiply` / `gamma_multiply` factors exist outside `theme.rs` against the one declared `tint()` at 0.16.

**Expected.** ui-ux-pro-max: exactly one primary CTA per screen with secondary actions visually subordinate; hierarchy from size/spacing/contrast; semantic tokens only inside components. impeccable (per supplied digest): fix drift at the narrowest correct level — a duplicated implementation should be replaced by the shared component, not re-tuned twice.

**Fix.** Make `primitives::button` / `Tone` visible outside `app` (it is `pub(crate)` today), then delete both local `primary_button` functions and call it. If the welcome CTA genuinely wants to be larger than the settings default, add a size to the `Tone` family rather than forking the recipe.

---

### 10. Dead and lying controls in the dock — **Medium**

**Observed.** `crates/purrcode-ide/src/app/dock.rs:59-61`:

```rust
if ui.small_button("Retry failed").clicked() {
    // Could send retry action
}
```

It renders identically to the working buttons beside it and does nothing.

`dock.rs:375-381`:

```rust
let wd = if self.selected.is_some() {
    // Could get from session state, for now use repo root
    self.repository_string()
} else {
    self.repository_string()
};
```

Both branches are identical, so a terminal opened while an agent session is selected silently starts in the user's own checkout rather than the session's worktree — a control that appears to be session-scoped and is not.

**Expected.** ui-ux-pro-max names "controls that look tappable but do nothing" as an anti-pattern under the disabled/loading rules; if an action is unavailable it must *look* unavailable. impeccable (per supplied digest) puts misleading state in triage rank 1.

**Fix.** Either wire `Retry failed` to the validation-retry request or drop the button until it exists. For the terminal, either resolve the session worktree or delete the dead `if` and rename nothing — an honest `self.repository_string()` with a comment is better than a branch that pretends.

---

### 11. The shipped "no folder opened" panel hides missing recents — **Medium**

**Observed.** The panel that actually renders is `navigation.rs:54-102`, and at `navigation.rs:78-81` it does:

```rust
let exists = entry.exists();
if !exists {
    continue;
}
```

A folder that was moved or deleted vanishes from the list with no explanation and no way to remove it from the stored history. Its rows are two stacked bare labels (`navigation.rs:82-96`) — name and location on separate lines, no row rect, no hover fill, no cursor change.

Meanwhile `welcome.rs:155-200` (`pub fn navigation`) is a fuller implementation of the same panel — horizontal rows, an `add_enabled(exists, …)` button so a missing entry is visibly disabled, a `"missing"` warning chip with an explanatory tooltip, and a `Remove` action (`welcome.rs:308-320`). It has **no caller anywhere in the crate** (only `welcome::pane` is called, at `mod.rs:1508`).

**Expected.** impeccable (per supplied digest): "an empty state differentiates first use, no results, active filters, missing permission, and failure, and always offers the next useful action." A silently shortened list is the failure case collapsed into the happy one. ui-ux-pro-max: error messages must state cause plus recovery.

**Fix.** Call the existing implementation. Route `navigation_welcome` through `welcome::navigation` (it already returns a `WelcomeChoice`, and `apply_welcome_choice` at `mod.rs:1524-1538` already handles `Forget`), then delete the duplicate. This is a deletion, not a build.

---

### 12. Three tab strips, three visual languages — **Medium**

**Observed.** In one window:

- `editor.rs:289-405` — hand-painted, 36pt, canvas-coloured active tab, 2pt accent top edge, close and dirty sharing one 16pt spot.
- `dock.rs:211-223` — `ui.selectable_label` per terminal with a detached `ui.small_button("×")` shown only on the active one.
- `code.rs:809-823` — the same `selectable_label` + `small_button` pattern again, plus a right-aligned `× Close all`.

**Expected.** impeccable (per supplied digest) classifies this as "one-off implementation — a shared component should replace it", and warns that the blended outcome is what leaves a surface worse than either option. ui-ux-pro-max: style must stay consistent across all screens.

**Fix.** Promote `editor.rs`'s `tab` free function to a shared helper and call it from both other strips. It already takes everything it needs (`icon`, `label`, `active`, `dirty`, `closable`, `id_salt`).

---

### 13. Text bullets used as status markers — **Medium**

**Observed.** `dock.rs:156` and `dock.rs:306`:

```rust
ui.label(RichText::new("●").small().color(color));
```

Two lines away in the same file, `dock.rs:88` and `dock.rs:267` call `crate::icons::step_marker(ui, marker, 9.0, color)` — the token marker for exactly this job. And `icons.rs:854` carries a unit test asserting that icon drawing may never fall back to text.

**Expected.** ui-ux-pro-max: no emoji or typed glyphs as structural icons; use a vector set with one consistent stroke weight. impeccable (per supplied digest): "icons are drawn, never typed… it is a placeholder that shipped."

**Fix.** Replace both with `icons::step_marker`. Two-line change; the helper is already imported in the file.

---

### 14. The type ramp is declared but not enforced — **Medium**

**Observed.** `theme.rs:55-64` declares a five-step ramp: `TYPE_META` 11, `TYPE_LABEL` 12, `TYPE_BODY` 14, `TYPE_TITLE` 20, `TYPE_DISPLAY` 24. The code paints at 33 literal sizes off that ramp, distributed as:

| size | sites |
| --- | --- |
| 9.5 | 1 |
| 10.0 | 2 |
| 10.5 | 14 |
| 11.0 | 5 |
| 11.5 | 3 |
| 12.0 | 6 |
| 12.5 | 1 |
| 24.0 | 1 |

The 14 sites at 10.5 are all the same "section heading" role — `mod.rs:1550`, `mod.rs:1743`, `mod.rs:1750`, `mod.rs:1789`, `navigation.rs:30`, `code.rs:480`, `code.rs:487`, and six in `workbench.rs` — while the *same role* elsewhere is written as `.small()`, which resolves to `TYPE_META` = 11.0 (`theme.rs:281-284`). Two values, one role.

`TYPE_DISPLAY` (24.0, `theme.rs:64`) has no consumer outside its own scale test at `theme.rs:562`, while two surfaces hand-write `24.0` for exactly that role: `workbench.rs:105` and `welcome.rs:229`.

The densest cluster is one screen — `workbench.rs` empty state at 10.0, 24.0, 12.5, 9.5, 11.5 within about 70 lines.

**Expected.** ui-ux-pro-max: font sizes come from a fixed discrete ramp; a discrete ramp plus weight mapping is what creates hierarchy that survives. impeccable (per supplied digest): "adjacent sizes or weights that are too close to carry different jobs are a flat hierarchy defect", and "being on the documented type ramp does not exempt a value" — 9.5pt interactive UI text is below its 11px floor.

**Fix.** Add one constant, `TYPE_EYEBROW = 10.5` (or fold the role into `TYPE_META` and drop it), then mechanically replace the 14 heading sites and route them all through `section_heading`. Replace the two 24.0 literals with `theme::TYPE_DISPLAY`. Raise the single 9.5 site (`workbench.rs:122`, "START WITH") to the eyebrow constant. That collapses 33 literals to about six legitimate ones.

---

### 15. `section_heading`'s trailing gap becomes horizontal padding inside a row — **Low**

**Observed.** `mod.rs:1548-1555` ends with `ui.add_space(6.0)`. It is called inside a `ui.horizontal` at `dock.rs:20`, `navigation.rs:192`, and `navigation.rs:217`, where egui advances the cursor along the *main* (horizontal) axis — so the intended 6pt of air below the heading becomes 6pt of gap to the right of it, and the vertical rhythm the helper exists to guarantee is silently absent on three surfaces.

**Expected.** [craft] A spacing helper must not change meaning with its container.

**Fix.** Split into `section_heading_label` (no trailing space) and `section_heading` (label + space), and use the former inside the three rows.

---

### 16. Eight raw corner radii against three radius tokens — **Low**

**Observed.** `theme.rs:26-30` declares `RADIUS_CONTROL` 7, `RADIUS_CARD` 10, `RADIUS_PILL` 99. Eight sites bypass them: `welcome.rs:220` (12), `welcome.rs:270` (8), `welcome.rs:279` (8), `welcome.rs:296` (6), `navigation.rs:524` (8), `errors.rs:326` (8), `code.rs:674` (6), `code.rs:835` (6).

**Expected.** ui-ux-pro-max: components reference semantic tokens; primitive or raw values inside a component are explicitly the wrong pattern.

**Fix.** Mechanical substitution. 8 → `RADIUS_CONTROL` reads slightly rounder than the current 7; 12 → `RADIUS_CARD`; 6 → `RADIUS_CONTROL`.

---

### 17. The terminal grid has no surface of its own — **Low**

**Observed.** `terminal.rs:519-521`:

```rust
let available = ui.available_size();
let rect = Rect::from_min_size(ui.cursor().min, available);
```

No frame, border, corner radius, padding, or header — the grid butts directly against the dock's 12/8 margin. Focus is real (`terminal.rs:536-551` sets a `set_focus_lock_filter` for tab/arrows/escape) but is only legible from the cursor's fill-vs-hollow state (`terminal.rs:~1002`). Nothing on the surface says "this terminal has the keyboard". Its font size is hardcoded at `terminal.rs:329` (`font_size: 12.0`) rather than `theme::TYPE_CODE`, which is the same number with no token behind it.

**Expected.** impeccable (per supplied digest) requires ≥8px (ideally 12–16px) of padding inside any bordered or colored container, and calls unthemed platform surfaces "the cheapest signal that a page was built rather than assembled". A focused input region with no focus affordance also fails the six-states rule.

**Fix.** Wrap the grid in `tokens.card()` with the fill overridden to the terminal background, add 6–8pt of inner padding, and swap the border to `accent_primary` when `focused` — reusing the ring helper from gap 6. Point `font_size` at `theme::TYPE_CODE`.

---

### 18. The welcome card is centred against a height it does not have — **Low**

**Observed.** `welcome.rs:215`:

```rust
ui.add_space(((available - 340.0) * 0.35).max(16.0));
```

340 is an assumed card height. The card's real height varies: the "or reopen" block (`welcome.rs:240-247`) adds roughly 60pt when any recent folder still exists, and the error line (`welcome.rs:248-251`) another ~30pt. So the card's optical position shifts by up to ~90pt depending on state — it moves down the screen when the error appears rather than staying put.

**Expected.** [craft] Optical placement should be derived from the measured content, not asserted. ui-ux-pro-max's layout-stability rule (reserve space for content that can appear) is the web analogue.

**Fix.** Lay the card out into a `ui.allocate_new_ui` / measure pass, or centre with a `Layout::centered_and_justified` on a *fixed-height* child rather than on the whole panel — note the second option is what caused the wordmark defect and must be applied to a bounded child, not the panel.

**Only confirmable visually** — trigger the error state and watch whether the card moves.

---

### 19. Four empty-state shapes — **Low `[taste]`**

**Observed.** `vertical_centered` + `add_space(24)` + a default-size muted label at `dock.rs:42`, `dock.rs:107`, `dock.rs:194`; the same but at `.size(12.0)` at `workbench.rs:243-249`; a 26pt glyph lockup at 30% height at `mod.rs:1926`; left-aligned `TYPE_BODY` with `BODY_LINE_HEIGHT` at `settings.rs:448`; left-aligned `.small()` pairs at `navigation.rs:117-128`.

**Expected.** impeccable (per supplied digest) treats consistency across comparable areas as conceptual drift.

**This is largely taste** — the dock's centred labels and settings' left-aligned copy are each defensible for their surface, and the impeccable digest itself says mode comes from the surface. What is *not* taste is that none of them distinguish "nothing yet" from "nothing matched" from "could not load"; where that matters (dock Problems, dock Tests) it is worth a real fix. Ranked low, listed for completeness.

---

## Computer-use verification plan

### Launching

`cargo run -p purrcode-ide` **will not work.** `crates/purrcode-ide/Cargo.toml` declares no `[[bin]]` and the crate has no `src/main.rs` — it is a library. The window is opened from the CLI:

```
cargo run -p purrcode-cli --bin purrcode -- ide
```

(`purrcode gui` is the same command; `ide` is an alias, `crates/purrcode-cli/src/main.rs:134`.) It requires a daemon token of ≥32 chars at the configured token path (`main.rs:3661-3665`) — run `purrcode init` first if it bails. Pass `--repository /Users/jackzhang/Documents/GitHub/PurrCode` to skip straight to the workspace, and omit it to land on the start screen. Default window is 1480×940; minimum is 900×600 (`lib.rs:46-47`).

Screenshot everything. Keep a numbered file per step so a finding can be pointed at.

### Ordered steps

**Step 0 — baseline.** Launch with no `--repository`. Screenshot the start screen at the default 1480×940.

**Step 1 — Gap 8 (welcome card elevation) and Gap 9 (CTA weight).** Zoom into the card. *Pass:* the card is visibly lighter than the canvas behind it, and "Open folder…" reads as the loudest element on the screen. *Fail (expected):* the card is a slightly darker rectangle than the pane it sits in, and the CTA is a washed translucent blue that competes with, or loses to, the outlined "reopen" button below it. Sample the pixel colours if the difference is subtle — `#090e12` card on `#0f1418` canvas is the signature.

**Step 2 — Gap 18 (card drift).** Trigger the welcome error path (pick a folder you have no read permission on, or point the daemon at a bad path so `welcome_error` is set). *Pass:* the card's top edge does not move when the red line appears. *Fail:* the whole card slides down. **Visual-only.**

**Step 3 — Gap 11 (missing recents).** Open a folder, quit, `mv` that folder aside, relaunch to the start screen, and open the sidebar's welcome column. *Pass:* the moved folder appears greyed with a "missing" chip and a Remove action. *Fail (expected):* it is simply gone, with no trace and no way to clean the list.

**Step 4 — Gap 3 (command pill occlusion). Visual-only, and the highest-value screenshot in this plan.** Open the repository. Resize the window down to its 900pt minimum (drag until it stops). Screenshot the title bar full-width, then zoom the region x∈[280,620]. *Pass:* the branch chip `v1.0/feat-adding-IDE` and the repo name are fully readable, with clear air before the pill starts. *Fail (expected):* the opaque pill is painted over the chip. Repeat at 1100 and 1480pt to find the exact width where it clears, and record that number — it is the threshold the fix should use.

**Step 5 — Gap 4 (measure).** At 1480pt with the sidebar open and the aux panel closed, send the agent a prompt whose answer is a long prose paragraph (e.g. "explain this repository's architecture in three paragraphs"). Screenshot the transcript. *Pass:* prose lines cap around 70–75 characters. *Fail (expected):* lines run the full column, ~150 characters. Count characters on the longest line directly from the screenshot; do not estimate.

**Step 6 — Gap 5 (forced 520pt column). Visual-only.** With a session selected, move the agent into the right auxiliary panel (the layout toggles in the title bar's right cluster), then drag the aux panel's left edge to its 320pt minimum. Start a new session so the empty state shows. *Pass:* the headline, subtitle, and three starter cards reflow into the narrow column. *Fail (expected):* they are laid out at 520pt and clipped at the panel edge — the "Give PurrCode a job." headline will be cut mid-word and the starter cards' chevrons will be off-screen. Screenshot at 320, 420, and 620pt.

**Step 7 — Gaps 1 and 2 (editor).** Open any `.rs` file from the Explorer into the centre editor.
- *Gap 2, visual-only:* scroll the code area with the trackpad. *Pass:* the numbers move with the lines. *Fail (expected):* the code scrolls, the gutter does not; line 1 stays pinned beside whatever is now at the top. Screenshot before and after the scroll in one pair.
- *Gap 1:* type a character into the file. Screenshot the tab — the dirty dot appears. Press ⌘S. *Pass:* the dot clears and the file on disk changed (verify with `git diff` in a second terminal). *Fail (expected):* nothing happens; `git diff` is clean. Then click the tab's ×. *Pass:* a confirmation appears. *Fail (expected):* the tab closes silently and the edit is gone.

**Step 8 — Gap 7 (row hit targets). Visual-only.** In the Explorer, hover slowly across a tree row from the filename out to the right edge of the sidebar. *Pass:* the cursor becomes a pointer and a hover fill spans the full row width for the whole traverse. *Fail (expected):* no hover fill and no cursor change anywhere, and clicking to the right of the name does nothing. Then run a Search with several matches of differing line lengths and screenshot the hovered list. *Fail (expected):* each hover highlight is a different width, matching its text.

**Step 9 — Gap 6 (focus). Visual-only.** From a fresh window, press Tab repeatedly (about 20 times), screenshotting after each press. *Pass:* a visible 2px accent ring lands on the rail items, tabs, session rows, send button, and command pill in turn. *Fail (expected):* nothing visible moves outside the settings window and the two search fields. Then open Settings (⌘, or the rail's gear) and Tab through it — the ring should be clearly visible there, which is the control case proving the ring exists and is simply not applied elsewhere.

**Step 10 — Gap 13 (typed bullets).** Open the dock's Problems tab with at least one problem present (break the build, or run a failing validation). Zoom into the status markers. *Pass:* the markers match `icons::step_marker` used in the Tests tab two panels over. *Fail (expected):* Problems and Activity render a font-dependent `●` while Tests renders a drawn marker — put the two tabs side by side in one screenshot.

**Step 11 — Gaps 10, 12.** In the dock: click "Retry failed" on a failed validation and confirm nothing at all changes (check the daemon log for an absent request). Then screenshot the editor tab strip, the terminal tab strip, and the source-panel tab strip in one composite — three visual languages in one image is the whole finding.

**Step 12 — Gaps 14, 15, 16 (drift sweep).** Screenshot the workbench empty state, the sidebar, and the dock header at 1× and at 2× zoom. *Look at:* the section headings ("AGENT", "RECENT", "START WITH", "TASKS", the dock's tab label) — they should all be the same size and weight. *Fail (expected):* they are not, and the dock's heading has extra space to its right rather than below it, so its underline of air lands in the wrong axis.

**Step 13 — Gap 17 (terminal).** Open the Terminal dock tab, click into the grid, screenshot; click into the composer, screenshot the terminal again. *Pass:* an obvious focus treatment on the terminal region appears and disappears. *Fail (expected):* only the block cursor changes from filled to hollow — a 6×12pt difference the user will not see.

### Gaps that cannot be confirmed by screenshot

Gaps 1 (the missing write path), 10 (the empty click handler and the identical `if` branches), 14 (the literal count), and 16 (the radius count) are code facts. They are cited with file:line above and need no runtime evidence. Everything else in the plan above should be treated as unconfirmed until the screenshot exists — per the ui-ux-pro-max review gate, a change you have not looked at is not finished, and the same holds for a defect you have not looked at.
