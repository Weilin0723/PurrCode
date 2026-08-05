//! Markdown, for the words the agent writes.
//!
//! The model answers in markdown whether or not anyone asked it to, so a plain
//! label turns "### 1. Backend Improvements" into literal hashes on screen and
//! leaves the reader decoding source. This module is the parser and the egui
//! renderer that stop that happening.
//!
//! Two properties matter here more than coverage of the specification.
//!
//! The first is that ordinary prose has to come out the other side untouched.
//! A sentence with no markdown in it is by far the most common input, and
//! mangling `snake_case_name` or `3 * 4` to be clever about the rare one is a
//! bad trade.
//!
//! The second is that the buffer is almost always *incomplete*. Assistant text
//! streams a token at a time, so at some point during every single reply there
//! is an open code fence or a half-typed `**`. A parser that waits for the
//! closing marker before it shows anything makes the answer look stalled; one
//! that lets an unclosed marker eat the rest of the document makes the answer
//! look like it vanished. Both are treated here as ordinary states of a live
//! document rather than as errors.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId, Label, RichText, Sense, Stroke, Ui};

use crate::theme::{self, Tokens};

// ── Geometry ───────────────────────────────────────────────────────────
//
// Nothing here picks a colour; these are the few distances the layout needs
// that the theme has no name for, kept together so a list and a quote indent
// by the same amount instead of by two different guesses.

/// One level of list nesting.
const LIST_NEST: f32 = 16.0;
/// The column the bullet or number lives in. The item's text starts after it,
/// and every wrapped line of that item lines up with the first one.
const LIST_GUTTER: f32 = 18.0;
/// Space between the marker and the text it labels.
const LIST_MARK_GAP: f32 = 6.0;
/// Deeper than this and the text column is too narrow to read; the runtime
/// occasionally emits pathological indentation, and a document should degrade
/// rather than collapse.
const MAX_LIST_DEPTH: usize = 5;
/// The indent of a quoted passage, which is also where its rule is drawn.
const QUOTE_INDENT: i8 = 12;
/// The band a horizontal rule occupies, hairline included.
const RULE_ROOM: f32 = 13.0;
/// The narrowest column worth laying text into. Below this the auxiliary panel
/// is so thin that wrapping produces one word per line; clamping keeps the
/// arithmetic away from zero and negative widths.
const MIN_MEASURE: f32 = 80.0;

// ── Document model ─────────────────────────────────────────────────────

/// A run of text inside one block.
///
/// Deliberately flat: `**bold `code`**` is vanishingly rare in an agent reply,
/// and a recursive span tree would buy that one case at the cost of a parser
/// nobody can hold in their head.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Span {
    Text(String),
    Bold(String),
    Italic(String),
    BoldItalic(String),
    InlineCode(String),
    Link { text: String, url: String },
}

impl Span {
    /// The characters this span puts on screen, with its syntax removed.
    ///
    /// Used by the renderer and by the test that proves no input text is lost.
    pub fn text(&self) -> &str {
        match self {
            Self::Text(text)
            | Self::Bold(text)
            | Self::Italic(text)
            | Self::BoldItalic(text)
            | Self::InlineCode(text) => text,
            Self::Link { text, url } => {
                if text.is_empty() {
                    url
                } else {
                    text
                }
            }
        }
    }
}

/// One paragraph-level element.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Block {
    Heading {
        level: u8,
        spans: Vec<Span>,
    },
    Paragraph(Vec<Span>),
    Bullet {
        depth: usize,
        spans: Vec<Span>,
    },
    Ordered {
        number: u64,
        depth: usize,
        spans: Vec<Span>,
    },
    Code {
        language: Option<String>,
        body: String,
    },
    Quote(Vec<Span>),
    Rule,
}

// ── Block parsing ──────────────────────────────────────────────────────

/// Split a markdown document into blocks.
///
/// Total: there is no error case. Anything the grammar does not recognise is
/// prose, because the alternative is refusing to show the user the answer the
/// agent actually gave.
pub fn parse(source: &str) -> Vec<Block> {
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks: Vec<Block> = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut quote: Vec<String> = Vec::new();
    // The indent widths of the list levels currently open. Models nest with
    // two, three or four spaces depending on the day, so depth is read from
    // the *order* of the indents seen rather than from dividing by a constant
    // the document never agreed to.
    let mut nesting: Vec<usize> = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim_start();
        let indent = indent_width(line);

        // A fence is tested before everything else, because inside one a `#`
        // is a shell comment and a `-` is a flag, not a heading and not a
        // bullet.
        if let Some(fence) = Fence::open(trimmed) {
            flush_paragraph(&mut blocks, &mut paragraph);
            flush_quote(&mut blocks, &mut quote);
            nesting.clear();
            let mut body = String::new();
            index += 1;
            while index < lines.len() && !fence.closes(lines[index]) {
                body.push_str(lines[index]);
                body.push('\n');
                index += 1;
            }
            // Running off the end of the buffer is the *normal* state while a
            // reply streams: the closing fence has not been generated yet. The
            // block is emitted with what has arrived so far, so the snippet
            // grows line by line instead of appearing all at once when the
            // model finally closes it.
            if index < lines.len() {
                index += 1;
            }
            blocks.push(Block::Code {
                language: fence.language,
                body: body.trim_end_matches('\n').to_owned(),
            });
            continue;
        }

        if trimmed.is_empty() {
            flush_paragraph(&mut blocks, &mut paragraph);
            flush_quote(&mut blocks, &mut quote);
            // A blank line ends a paragraph but not a list: `- a\n\n- b` is
            // one list with a loose item, and resetting the nesting here would
            // flatten every list a model chose to space out.
            index += 1;
            continue;
        }

        if is_rule(trimmed) {
            flush_paragraph(&mut blocks, &mut paragraph);
            flush_quote(&mut blocks, &mut quote);
            nesting.clear();
            blocks.push(Block::Rule);
            index += 1;
            continue;
        }

        if let Some((level, rest)) = heading_at(trimmed) {
            flush_paragraph(&mut blocks, &mut paragraph);
            flush_quote(&mut blocks, &mut quote);
            nesting.clear();
            blocks.push(Block::Heading {
                level,
                spans: parse_inline(rest),
            });
            index += 1;
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix('>') {
            flush_paragraph(&mut blocks, &mut paragraph);
            nesting.clear();
            quote.push(rest.strip_prefix(' ').unwrap_or(rest).to_owned());
            index += 1;
            continue;
        }

        if let Some((marker, rest)) = list_item_at(trimmed) {
            flush_paragraph(&mut blocks, &mut paragraph);
            flush_quote(&mut blocks, &mut quote);
            let depth = list_depth(&mut nesting, indent);
            blocks.push(match marker {
                ListMarker::Bullet => Block::Bullet {
                    depth,
                    spans: parse_inline(rest),
                },
                ListMarker::Ordered(number) => Block::Ordered {
                    number,
                    depth,
                    spans: parse_inline(rest),
                },
            });
            index += 1;
            continue;
        }

        flush_quote(&mut blocks, &mut quote);
        if indent == 0 {
            nesting.clear();
        }
        paragraph.push(trimmed.trim_end());
        index += 1;
    }

    flush_paragraph(&mut blocks, &mut paragraph);
    flush_quote(&mut blocks, &mut quote);
    blocks
}

fn flush_paragraph(blocks: &mut Vec<Block>, buffer: &mut Vec<&str>) {
    if buffer.is_empty() {
        return;
    }
    // Soft line breaks join with a space, the way every markdown reader shows
    // them: keeping them would hard-wrap the answer to whatever width the model
    // happened to generate rather than to the width of the reader's window.
    let text = buffer.join(" ");
    buffer.clear();
    blocks.push(Block::Paragraph(parse_inline(&text)));
}

fn flush_quote(blocks: &mut Vec<Block>, buffer: &mut Vec<String>) {
    if buffer.is_empty() {
        return;
    }
    let text = buffer.join(" ");
    buffer.clear();
    blocks.push(Block::Quote(parse_inline(&text)));
}

/// How far a line is indented, counting a tab as four columns.
fn indent_width(line: &str) -> usize {
    let mut width = 0;
    for character in line.chars() {
        match character {
            ' ' => width += 1,
            '\t' => width += 4,
            _ => break,
        }
    }
    width
}

/// The depth of a list item at `indent`, given the levels already open.
///
/// A stack rather than `indent / 2`: that division reads a four-space document
/// as twice as deep as it is, and a three-space one as neither. What actually
/// carries the nesting is that a deeper item is indented *more* than its
/// parent, whatever the step size the model chose.
fn list_depth(nesting: &mut Vec<usize>, indent: usize) -> usize {
    while nesting.last().is_some_and(|open| indent < *open) {
        nesting.pop();
    }
    match nesting.last() {
        Some(open) if indent > *open => nesting.push(indent),
        None => nesting.push(indent),
        _ => {}
    }
    nesting.len().saturating_sub(1)
}

/// `---`, `***`, `___` — three or more of one marker and nothing else.
fn is_rule(trimmed: &str) -> bool {
    let mut marker: Option<char> = None;
    let mut count = 0;
    for character in trimmed.chars() {
        match character {
            ' ' | '\t' => {}
            '-' | '*' | '_' => {
                if marker.is_some_and(|open| open != character) {
                    return false;
                }
                marker = Some(character);
                count += 1;
            }
            _ => return false,
        }
    }
    count >= 3
}

/// `#` through `######`, which must be followed by a space to count — `#42`
/// is an issue number, not a heading.
fn heading_at(trimmed: &str) -> Option<(u8, &str)> {
    let hashes = trimmed
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    // `#` is one byte, so the character count is also the byte offset.
    let rest = &trimmed[hashes..];
    if rest.is_empty() {
        return Some((hashes as u8, ""));
    }
    if !rest.starts_with(' ') {
        return None;
    }
    let body = rest.trim();
    // `## Title ##` closes with decoration; `# C#` does not. The difference is
    // the space in front of the closing run, so that is what is tested.
    let stripped = body.trim_end_matches('#');
    let body = if stripped.len() < body.len() && (stripped.is_empty() || stripped.ends_with(' ')) {
        stripped.trim_end()
    } else {
        body
    };
    Some((hashes as u8, body))
}

enum ListMarker {
    Bullet,
    Ordered(u64),
}

/// `- `, `* `, `+ `, `1. ` or `1) `. The trailing space is required, which is
/// what keeps `*italic*` at the start of a line out of the list grammar.
fn list_item_at(trimmed: &str) -> Option<(ListMarker, &str)> {
    if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
    {
        return Some((ListMarker::Bullet, rest.trim_start()));
    }
    let digits = trimmed
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .count();
    if digits == 0 || digits > 9 {
        return None;
    }
    // Digits are one byte each, so the count is the byte offset.
    let rest = trimmed[digits..]
        .strip_prefix(". ")
        .or_else(|| trimmed[digits..].strip_prefix(") "))?;
    let number = trimmed[..digits].parse::<u64>().ok()?;
    Some((ListMarker::Ordered(number), rest.trim_start()))
}

/// An open fenced block, remembered so its closing marker can be matched.
struct Fence {
    marker: char,
    run: usize,
    language: Option<String>,
}

impl Fence {
    fn open(trimmed: &str) -> Option<Self> {
        let marker = trimmed.chars().next()?;
        if marker != '`' && marker != '~' {
            return None;
        }
        let run = trimmed
            .chars()
            .take_while(|character| *character == marker)
            .count();
        if run < 3 {
            return None;
        }
        let info: String = trimmed.chars().skip(run).collect();
        // A backtick fence's info string may not itself contain a backtick,
        // which is what keeps ```` ```a``` ```` from opening a block.
        if marker == '`' && info.contains('`') {
            return None;
        }
        let language = info
            .split_whitespace()
            .next()
            .map(|word| word.to_ascii_lowercase());
        Some(Self {
            marker,
            run,
            language,
        })
    }

    fn closes(&self, line: &str) -> bool {
        let trimmed = line.trim();
        !trimmed.is_empty()
            && trimmed.chars().all(|character| character == self.marker)
            && trimmed.chars().count() >= self.run
    }
}

// ── Inline parsing ─────────────────────────────────────────────────────

/// Split one line of text into spans.
///
/// Works over a `Vec<char>` rather than byte offsets. The product has a Chinese
/// interface and the agent answers in Chinese; indexing a `&str` by a delimiter
/// position found with `find` is exactly how a renderer panics halfway through
/// a sentence nobody on the team can read.
pub fn parse_inline(source: &str) -> Vec<Span> {
    let characters: Vec<char> = source.chars().collect();
    let mut spans: Vec<Span> = Vec::new();
    let mut literal = String::new();
    let mut index = 0;

    while index < characters.len() {
        let character = characters[index];

        if character == '`' {
            let run = marker_run(&characters, index, '`');
            if let Some(close) = closing_backticks(&characters, index + run, run) {
                push_literal(&mut spans, &mut literal);
                spans.push(Span::InlineCode(
                    characters[index + run..close].iter().collect(),
                ));
                index = close + run;
                continue;
            }
            // No closing run yet — mid-stream, or the model simply typed a
            // backtick. Either way it is a character, not a marker.
            literal.extend(&characters[index..index + run]);
            index += run;
            continue;
        }

        if character == '['
            && let Some((text, url, next)) = link_at(&characters, index)
        {
            push_literal(&mut spans, &mut literal);
            spans.push(Span::Link { text, url });
            index = next;
            continue;
        }

        if (character == '*' || character == '_')
            && let Some((run, inner, next)) = emphasis_at(&characters, index)
        {
            push_literal(&mut spans, &mut literal);
            spans.push(match run {
                1 => Span::Italic(inner),
                2 => Span::Bold(inner),
                _ => Span::BoldItalic(inner),
            });
            index = next;
            continue;
        }

        literal.push(character);
        index += 1;
    }

    push_literal(&mut spans, &mut literal);
    spans
}

fn push_literal(spans: &mut Vec<Span>, literal: &mut String) {
    if literal.is_empty() {
        return;
    }
    spans.push(Span::Text(std::mem::take(literal)));
}

fn marker_run(characters: &[char], start: usize, marker: char) -> usize {
    characters[start..]
        .iter()
        .take_while(|character| **character == marker)
        .count()
}

/// The start of the next run of exactly `run` backticks, per CommonMark: a
/// longer run does not close a shorter one, so `` `a``b` `` stays one span.
fn closing_backticks(characters: &[char], from: usize, run: usize) -> Option<usize> {
    let mut index = from;
    while index < characters.len() {
        if characters[index] == '`' {
            let length = marker_run(characters, index, '`');
            if length == run {
                return Some(index);
            }
            index += length;
        } else {
            index += 1;
        }
    }
    None
}

/// `[text](url)`, all on one line and without nesting.
fn link_at(characters: &[char], start: usize) -> Option<(String, String, usize)> {
    let close = characters[start + 1..]
        .iter()
        .position(|character| *character == ']')
        .map(|offset| start + 1 + offset)?;
    if characters.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = characters[close + 2..]
        .iter()
        .position(|character| *character == ')')
        .map(|offset| close + 2 + offset)?;
    let url: String = characters[close + 2..end].iter().collect();
    if url.trim().is_empty() {
        return None;
    }
    Some((characters[start + 1..close].iter().collect(), url, end + 1))
}

/// Emphasis starting at `start`, as `(marker run length, inner text, next)`.
///
/// Returns `None` for everything that merely *looks* like a marker: an
/// underscore inside a word, an asterisk used as multiplication, and — the case
/// that matters most while text is streaming — an opener whose closer has not
/// been generated yet. In every one of those the caller keeps the character as
/// literal text, so a half-typed `**` shows two asterisks for a frame rather
/// than swallowing the rest of the paragraph.
fn emphasis_at(characters: &[char], start: usize) -> Option<(usize, String, usize)> {
    let marker = characters[start];
    let run = marker_run(characters, start, marker).min(3);

    // `snake_case_name` is an identifier. Intraword underscores are never
    // emphasis; intraword asterisks are, because nothing else uses them.
    if marker == '_' && start > 0 && is_word(characters[start - 1]) {
        return None;
    }
    // An opener is glued to its content: `3 * 4` is arithmetic.
    if characters
        .get(start + run)
        .is_none_or(|next| next.is_whitespace())
    {
        return None;
    }

    let mut index = start + run;
    while index < characters.len() {
        if characters[index] != marker {
            index += 1;
            continue;
        }
        let length = marker_run(characters, index, marker);
        let glued = !characters[index - 1].is_whitespace();
        let intraword = marker == '_'
            && characters
                .get(index + length)
                .is_some_and(|next| is_word(*next));
        if length >= run && glued && !intraword {
            return Some((
                run,
                characters[start + run..index].iter().collect(),
                index + run,
            ));
        }
        index += length;
    }
    None
}

fn is_word(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

// ── Rendering ──────────────────────────────────────────────────────────

/// The type treatment a block hands to the span walker.
///
/// `body` and `strong` are both tokens because emphasis in this toolkit *is* a
/// colour: egui loads one weight per family, so `strong()` brightens rather
/// than thickens. Running prose therefore sits on `text_secondary` and a bold
/// run steps up to `text_primary` — the same contrast step the rest of the
/// product uses between a title and its caption, and it survives all three
/// themes because both ends are named roles.
#[derive(Clone)]
struct TypeStyle {
    width: f32,
    size: f32,
    family: FontFamily,
    body: Color32,
    strong: Color32,
}

/// The characters a link covers, as an offset into the block's laid-out text.
struct LinkRange {
    start: usize,
    end: usize,
    url: String,
}

/// Parse `source` and draw it.
///
/// `measure` is the column width to wrap into, passed in rather than read from
/// `ui.available_width()` so a caller inside a scroll area can hand down a
/// width that does not flinch when a scrollbar appears.
pub fn render(ui: &mut Ui, tokens: &Tokens, source: &str, measure: f32) {
    render_blocks(ui, tokens, &parse(source), measure);
}

/// Draw an already-parsed document. Separate from [`render`] so a caller that
/// wants to inspect or filter the blocks does not have to parse twice.
pub fn render_blocks(ui: &mut Ui, tokens: &Tokens, blocks: &[Block], measure: f32) {
    let width = measure.max(MIN_MEASURE);
    // Prose is capped to a reading measure so a line never runs wall-to-wall
    // (~150 characters on the product's primary surface). Code blocks and
    // diffs are exempt: they get the full column, because wrapping a code line
    // is worse than a long one.
    let prose_width = width.min(theme::READING_MEASURE);
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            ui.add_space(gap_above(block));
        }
        match block {
            Block::Heading { level, spans } => heading(ui, tokens, *level, spans, prose_width),
            Block::Paragraph(spans) => {
                let (job, links) = inline_job(tokens, spans, &prose(tokens, prose_width));
                span_label(ui, job, links);
            }
            Block::Bullet { depth, spans } => list_row(ui, tokens, *depth, "•", spans, prose_width),
            Block::Ordered {
                number,
                depth,
                spans,
            } => list_row(
                ui,
                tokens,
                *depth,
                &format!("{number}."),
                spans,
                prose_width,
            ),
            Block::Code { language, body } => {
                code_block(ui, tokens, language.as_deref(), body, width);
            }
            Block::Quote(spans) => quote_block(ui, tokens, spans, prose_width),
            Block::Rule => {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(prose_width, RULE_ROOM), Sense::hover());
                ui.painter()
                    .hline(rect.x_range(), rect.center().y, tokens.hairline());
            }
        }
    }
}

/// Air above a block. A heading needs room to belong to what follows it rather
/// than to what it interrupts; list items are one object and stay tight.
fn gap_above(block: &Block) -> f32 {
    match block {
        Block::Heading { .. } => 10.0,
        Block::Bullet { .. } | Block::Ordered { .. } => 2.0,
        _ => 6.0,
    }
}

fn prose(tokens: &Tokens, width: f32) -> TypeStyle {
    TypeStyle {
        width,
        size: theme::TYPE_BODY,
        family: FontFamily::Proportional,
        body: tokens.text_secondary,
        strong: tokens.text_primary,
    }
}

/// The heading ramp, derived from the two type tokens it runs between instead
/// of frozen as six hand-picked numbers: move the product's type scale and the
/// headings move with it.
fn heading_size(level: u8) -> f32 {
    let step = f32::from(level.clamp(1, 6) - 1) / 5.0;
    theme::TYPE_TITLE + (theme::TYPE_BODY - theme::TYPE_TITLE) * step
}

/// Which headings are set in the product's display face.
///
/// Only the two that are actually titles. The display face at 15pt stops
/// reading as a title and starts reading as a slightly wrong body font.
fn heading_is_display(level: u8) -> bool {
    level <= 2
}

fn heading(ui: &mut Ui, tokens: &Tokens, level: u8, spans: &[Span], width: f32) {
    let level = level.clamp(1, 6);
    let style = TypeStyle {
        width,
        size: heading_size(level),
        family: if heading_is_display(level) {
            FontFamily::Name("purrcode_display".into())
        } else {
            FontFamily::Proportional
        },
        // A heading is already the emphatic thing on the line, so both roles
        // resolve to the primary colour and `**bold**` inside one is a no-op
        // rather than a second, competing weight.
        body: tokens.text_primary,
        strong: tokens.text_primary,
    };
    let (job, links) = inline_job(tokens, spans, &style);
    span_label(ui, job, links);
}

fn list_row(ui: &mut Ui, tokens: &Tokens, depth: usize, mark: &str, spans: &[Span], width: f32) {
    let depth = depth.min(MAX_LIST_DEPTH);
    let indent = depth as f32 * LIST_NEST;
    let style = prose(tokens, (width - indent - LIST_GUTTER).max(MIN_MEASURE));
    let (job, links) = inline_job(tokens, spans, &style);

    let marker = ui.fonts_mut(|fonts| {
        fonts.layout_job(LayoutJob::simple_singleline(
            mark.to_owned(),
            FontId::new(theme::TYPE_BODY, FontFamily::Proportional),
            tokens.text_muted,
        ))
    });

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(indent);
        // The marker owns a fixed column and the item's text is one galley, so
        // a wrapped line starts under the first word instead of under the
        // bullet — which is the whole difference between a list and a stack of
        // sentences that happen to begin with a dot. Right-aligning inside the
        // column also lines "9." up with "10.".
        let (gutter, _) =
            ui.allocate_exact_size(egui::vec2(LIST_GUTTER, marker.size().y), Sense::hover());
        ui.painter().galley(
            egui::pos2(
                gutter.right() - LIST_MARK_GAP - marker.size().x,
                gutter.top(),
            ),
            marker.clone(),
            tokens.text_muted,
        );
        span_label(ui, job, links);
    });
}

fn quote_block(ui: &mut Ui, tokens: &Tokens, spans: &[Span], width: f32) {
    let style = prose(
        tokens,
        (width - f32::from(QUOTE_INDENT) - 4.0).max(MIN_MEASURE),
    );
    let (job, links) = inline_job(tokens, spans, &style);
    let frame = egui::Frame::new()
        .inner_margin(egui::Margin {
            left: QUOTE_INDENT,
            right: 0,
            top: 2,
            bottom: 2,
        })
        .show(ui, |ui| span_label(ui, job, links));
    // A rule down the left edge, the way a pulled quotation is set in print. A
    // tinted box would read as a status card, and this passage has no status.
    let rect = frame.response.rect;
    ui.painter().line_segment(
        [rect.left_top(), rect.left_bottom()],
        Stroke::new(2.0_f32, tokens.border_strong),
    );
}

/// What code set inside prose looks like — the font it is in, the surface it
/// sits on, the colour it is written in.
///
/// A path in the work log and a backticked symbol in an answer are the same
/// kind of thing, and the moment two files describe that thing separately they
/// drift: the log was setting 11pt secondary text where a reply set 12pt
/// primary, so one file name read as two different objects on one screen. The
/// two renderers stay separate because one is a run inside a galley and the
/// other is a widget in a wrapped row — but they both ask here what it is.
pub(crate) struct CodeChip {
    pub font: FontId,
    pub fill: Color32,
    pub text: Color32,
}

pub(crate) fn code_chip(tokens: &Tokens) -> CodeChip {
    CodeChip {
        font: FontId::new(theme::TYPE_CODE, FontFamily::Monospace),
        fill: tokens.background_secondary,
        text: tokens.text_primary,
    }
}

fn code_block(ui: &mut Ui, tokens: &Tokens, language: Option<&str>, body: &str, width: f32) {
    egui::Frame::new()
        .fill(tokens.background_secondary)
        .stroke(tokens.hairline())
        .corner_radius(theme::RADIUS_CARD)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            let inner = (width - 22.0).max(MIN_MEASURE);
            // Without this the frame shrink-wraps to the longest line and a
            // three-word snippet becomes a three-word box floating in the
            // column.
            ui.set_min_width(inner);
            if let Some(language) = language.filter(|language| !language.is_empty()) {
                ui.label(
                    RichText::new(language.to_ascii_uppercase())
                        .size(theme::TYPE_META)
                        .color(tokens.text_muted),
                );
                ui.add_space(2.0);
            }

            let mut job = match syntect_language(language) {
                // The same highlighter the code editor uses (app/editor.rs), on
                // purpose: a snippet quoted in the reply and the same file open
                // in the editor have to be coloured by one thing, and a
                // hand-rolled second highlighter here would guarantee they
                // disagree.
                Some(name) => egui_extras::syntax_highlighting::highlight(
                    ui.ctx(),
                    ui.style(),
                    &egui_extras::syntax_highlighting::CodeTheme::from_style(ui.style()),
                    body,
                    &name,
                ),
                None => {
                    let mut job = LayoutJob::default();
                    job.append(
                        body,
                        0.0,
                        TextFormat {
                            font_id: FontId::new(theme::TYPE_CODE, FontFamily::Monospace),
                            color: tokens.text_secondary,
                            ..Default::default()
                        },
                    );
                    job
                }
            };
            // Code wraps rather than scrolling sideways. A nested horizontal
            // scroll area inside the transcript steals the wheel from the
            // conversation, and a snippet the reader has to drag to finish is
            // worse than one that folds.
            job.wrap.max_width = inner;
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
            ui.add(Label::new(galley));
        });
}

/// Map a fence's info string — or a file's extension — to a syntect language.
///
/// Deliberately a short, closed list rather than "pass whatever the model
/// wrote to syntect": the fence word is model output, and a closed map is the
/// difference between an unknown language rendering as plain monospace and it
/// rendering as whatever syntax happened to share its name.
///
/// The code editor asks here too. It kept its own extension list, which is how
/// a `.c` file opened in the editor and the same code quoted in a reply came
/// out coloured differently — the words a fence uses (`rs`, `py`, `ts`) are the
/// extensions a path uses, so there was never a reason for two lists.
pub(crate) fn syntect_language(language: Option<&str>) -> Option<String> {
    let name = match language?.trim().to_ascii_lowercase().as_str() {
        "rust" | "rs" => "rust",
        "python" | "py" => "python",
        "javascript" | "js" | "jsx" => "javascript",
        "typescript" | "ts" | "tsx" => "typescript",
        "json" => "json",
        "toml" | "ini" => "ini",
        "yaml" | "yml" => "yaml",
        "html" | "xml" => "html",
        "css" => "css",
        "sh" | "bash" | "zsh" | "shell" | "console" => "shell",
        "markdown" | "md" => "markdown",
        "c" => "c",
        "cpp" | "c++" => "c++",
        _ => "",
    };
    (!name.is_empty()).then(|| name.to_owned())
}

/// Turn spans into one layout job, and record where the links landed.
fn inline_job(tokens: &Tokens, spans: &[Span], style: &TypeStyle) -> (LayoutJob, Vec<LinkRange>) {
    let mut job = LayoutJob::default();
    job.wrap.max_width = style.width;
    let mut links = Vec::new();
    // Character offsets, not bytes: this is what a galley reports back when it
    // is asked which glyph the pointer is over.
    let mut cursor = 0usize;

    for span in spans {
        let text = span.text();
        if text.is_empty() {
            continue;
        }
        let base = TextFormat {
            font_id: FontId::new(style.size, style.family.clone()),
            color: style.body,
            ..Default::default()
        };
        let format = match span {
            Span::Text(_) => base,
            Span::Bold(_) => TextFormat {
                color: style.strong,
                ..base
            },
            Span::Italic(_) => TextFormat {
                italics: true,
                ..base
            },
            Span::BoldItalic(_) => TextFormat {
                color: style.strong,
                italics: true,
                ..base
            },
            // The chip treatment the rest of the product uses for a file name
            // or a command, so a symbol never gets read as a word.
            Span::InlineCode(_) => {
                let chip = code_chip(tokens);
                TextFormat {
                    font_id: chip.font,
                    color: chip.text,
                    background: chip.fill,
                    ..Default::default()
                }
            }
            Span::Link { url, .. } => {
                links.push(LinkRange {
                    start: cursor,
                    end: cursor + text.chars().count(),
                    url: url.clone(),
                });
                TextFormat {
                    color: tokens.accent_primary,
                    underline: Stroke::new(1.0_f32, tokens.accent_primary),
                    ..base
                }
            }
        };
        cursor += text.chars().count();
        job.append(text, 0.0, format);
    }

    (job, links)
}

/// Draw one block's text as a single galley.
///
/// One widget per block rather than one per word: egui's wrapping horizontal
/// layout can only break *between* widgets, so a word-per-widget paragraph
/// wraps at word boundaries but loses the run of an inline code background and
/// costs a widget for every word of a long answer. A galley wraps properly on
/// its own, stays selectable, and — because a galley can say which character
/// the pointer is over — still resolves a click to the link under it.
fn span_label(ui: &mut Ui, job: LayoutJob, links: Vec<LinkRange>) {
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let mut label = Label::new(galley.clone());
    if !links.is_empty() {
        label = label.sense(Sense::click());
    }
    let response = ui.add(label);
    if links.is_empty() {
        return;
    }
    let Some(pointer) = response.hover_pos() else {
        return;
    };
    let index = galley.cursor_from_pos(pointer - response.rect.min).index;
    let Some(link) = links
        .iter()
        .find(|link| index >= link.start && index < link.end)
    else {
        return;
    };
    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    if response.clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(&link.url));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(source: &str) -> Vec<Block> {
        parse(source)
    }

    /// Everything the renderer would put on screen, in order. The language of
    /// a fenced block is included because the renderer shows it as a chip.
    fn rendered_text(blocks: &[Block]) -> String {
        let mut out = String::new();
        for block in blocks {
            match block {
                Block::Heading { spans, .. }
                | Block::Paragraph(spans)
                | Block::Bullet { spans, .. }
                | Block::Ordered { spans, .. }
                | Block::Quote(spans) => {
                    for span in spans {
                        out.push_str(span.text());
                    }
                }
                Block::Code { language, body } => {
                    if let Some(language) = language {
                        out.push_str(language);
                        out.push('\n');
                    }
                    out.push_str(body);
                }
                Block::Rule => {}
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn ordinary_prose_comes_out_of_the_parser_exactly_as_it_went_in() {
        // The common case, and the one a clever parser is most likely to
        // damage: an identifier, an arithmetic asterisk, a path and a hash.
        let source = "The file src/app/mod.rs uses snake_case_names, 3 * 4 = 12, and issue #42.";
        assert_eq!(
            doc(source),
            vec![Block::Paragraph(vec![Span::Text(source.to_owned())])]
        );
    }

    #[test]
    fn a_soft_wrapped_paragraph_keeps_every_word() {
        let blocks = doc("PurrCode keeps the evidence\nbeside the work.");
        assert_eq!(
            blocks,
            vec![Block::Paragraph(vec![Span::Text(
                "PurrCode keeps the evidence beside the work.".to_owned()
            )])]
        );
    }

    #[test]
    fn a_heading_carries_its_level_and_loses_only_its_hashes() {
        assert_eq!(
            doc("### 1. Backend Improvements"),
            vec![Block::Heading {
                level: 3,
                spans: vec![Span::Text("1. Backend Improvements".to_owned())],
            }]
        );
        assert_eq!(
            doc("###### deep"),
            vec![Block::Heading {
                level: 6,
                spans: vec![Span::Text("deep".to_owned())],
            }]
        );
        // Seven is not a heading, and neither is a hash with no space.
        assert!(matches!(
            doc("####### seven").as_slice(),
            [Block::Paragraph(_)]
        ));
        assert!(matches!(
            doc("#42 is open").as_slice(),
            [Block::Paragraph(_)]
        ));
    }

    #[test]
    fn an_unterminated_code_fence_shows_what_has_arrived_so_far() {
        // The state every streaming reply passes through. The block must
        // render with the lines received, and the text after it must not be
        // swallowed — because there is no text after it yet.
        let blocks = doc("Here you go:\n\n```rust\nfn main() {\n    println!(\"hi\");");
        assert_eq!(
            blocks,
            vec![
                Block::Paragraph(vec![Span::Text("Here you go:".to_owned())]),
                Block::Code {
                    language: Some("rust".to_owned()),
                    body: "fn main() {\n    println!(\"hi\");".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn a_closed_fence_keeps_its_language_and_its_body_verbatim() {
        let blocks = doc("```python\nx = 1\n\ny = 2\n```\nafter");
        assert_eq!(
            blocks,
            vec![
                Block::Code {
                    language: Some("python".to_owned()),
                    body: "x = 1\n\ny = 2".to_owned(),
                },
                Block::Paragraph(vec![Span::Text("after".to_owned())]),
            ]
        );
    }

    #[test]
    fn a_fence_hides_markdown_that_is_really_source_code() {
        let blocks = doc("```sh\n# not a heading\n- not a bullet\n```");
        assert_eq!(
            blocks,
            vec![Block::Code {
                language: Some("sh".to_owned()),
                body: "# not a heading\n- not a bullet".to_owned(),
            }]
        );
    }

    #[test]
    fn an_unclosed_bold_marker_stays_literal_instead_of_eating_the_paragraph() {
        let blocks = doc("Replace the **fragile parser");
        assert_eq!(
            blocks,
            vec![Block::Paragraph(vec![Span::Text(
                "Replace the **fragile parser".to_owned()
            )])]
        );
    }

    #[test]
    fn emphasis_is_recognised_in_all_four_of_its_spellings() {
        assert_eq!(
            parse_inline("**a** __b__ *c* _d_ ***e***"),
            vec![
                Span::Bold("a".to_owned()),
                Span::Text(" ".to_owned()),
                Span::Bold("b".to_owned()),
                Span::Text(" ".to_owned()),
                Span::Italic("c".to_owned()),
                Span::Text(" ".to_owned()),
                Span::Italic("d".to_owned()),
                Span::Text(" ".to_owned()),
                Span::BoldItalic("e".to_owned()),
            ]
        );
    }

    #[test]
    fn a_marker_inside_a_word_or_between_numbers_is_not_emphasis() {
        assert_eq!(
            parse_inline("snake_case_name"),
            vec![Span::Text("snake_case_name".to_owned())]
        );
        assert_eq!(
            parse_inline("3 * 4 * 5"),
            vec![Span::Text("3 * 4 * 5".to_owned())]
        );
        assert_eq!(
            parse_inline("a_b_c and _d_"),
            vec![
                Span::Text("a_b_c and ".to_owned()),
                Span::Italic("d".to_owned()),
            ]
        );
    }

    #[test]
    fn inline_code_holds_its_body_and_an_unclosed_backtick_does_not() {
        assert_eq!(
            parse_inline("call `agent_surface(ui)` first"),
            vec![
                Span::Text("call ".to_owned()),
                Span::InlineCode("agent_surface(ui)".to_owned()),
                Span::Text(" first".to_owned()),
            ]
        );
        assert_eq!(
            parse_inline("call `agent_surf"),
            vec![Span::Text("call `agent_surf".to_owned())]
        );
    }

    #[test]
    fn a_link_becomes_one_span_and_a_half_typed_one_stays_text() {
        assert_eq!(
            parse_inline("see [the PRD](docs/prd.md) for more"),
            vec![
                Span::Text("see ".to_owned()),
                Span::Link {
                    text: "the PRD".to_owned(),
                    url: "docs/prd.md".to_owned(),
                },
                Span::Text(" for more".to_owned()),
            ]
        );
        assert_eq!(
            parse_inline("see [the PRD](docs/pr"),
            vec![Span::Text("see [the PRD](docs/pr".to_owned())]
        );
        assert_eq!(
            parse_inline("an array[0] index"),
            vec![Span::Text("an array[0] index".to_owned())]
        );
    }

    #[test]
    fn nested_bullets_report_the_depth_their_indentation_implies() {
        let blocks = doc("- one\n  - two\n    - three\n- back");
        let depths: Vec<usize> = blocks
            .iter()
            .filter_map(|block| match block {
                Block::Bullet { depth, .. } => Some(*depth),
                _ => None,
            })
            .collect();
        assert_eq!(depths, vec![0, 1, 2, 0]);
    }

    #[test]
    fn a_four_space_document_nests_exactly_as_deep_as_a_two_space_one() {
        // `indent / 2` would call this three levels deep. Nesting is carried by
        // the order of the indents, not by their size.
        let two = doc("- a\n  - b");
        let four = doc("- a\n    - b");
        assert_eq!(two, four);
    }

    #[test]
    fn ordered_items_keep_the_numbers_the_model_wrote() {
        let blocks = doc("1. first\n2. second\n7. seventh");
        assert_eq!(
            blocks,
            vec![
                Block::Ordered {
                    number: 1,
                    depth: 0,
                    spans: vec![Span::Text("first".to_owned())],
                },
                Block::Ordered {
                    number: 2,
                    depth: 0,
                    spans: vec![Span::Text("second".to_owned())],
                },
                Block::Ordered {
                    number: 7,
                    depth: 0,
                    spans: vec![Span::Text("seventh".to_owned())],
                },
            ]
        );
    }

    #[test]
    fn a_rule_is_a_rule_and_a_bullet_is_a_bullet() {
        assert_eq!(doc("---"), vec![Block::Rule]);
        assert_eq!(doc("***"), vec![Block::Rule]);
        assert_eq!(doc("___"), vec![Block::Rule]);
        assert_eq!(doc("- - -"), vec![Block::Rule]);
        assert_eq!(
            doc("- item"),
            vec![Block::Bullet {
                depth: 0,
                spans: vec![Span::Text("item".to_owned())],
            }]
        );
        // Two dashes are not a rule, and a dash inside a sentence never was.
        assert!(matches!(doc("--").as_slice(), [Block::Paragraph(_)]));
        assert!(matches!(doc("a --- b").as_slice(), [Block::Paragraph(_)]));
    }

    #[test]
    fn consecutive_quote_lines_become_one_passage() {
        assert_eq!(
            doc("> the durable log\n> and the worktree agree"),
            vec![Block::Quote(vec![Span::Text(
                "the durable log and the worktree agree".to_owned()
            )])]
        );
    }

    #[test]
    fn chinese_text_is_parsed_and_measured_by_character_not_by_byte() {
        let blocks = doc("## 后端改进\n\n- **替换**解析器：见 `crates/agent-runtime`。");
        assert_eq!(
            blocks,
            vec![
                Block::Heading {
                    level: 2,
                    spans: vec![Span::Text("后端改进".to_owned())],
                },
                Block::Bullet {
                    depth: 0,
                    spans: vec![
                        Span::Bold("替换".to_owned()),
                        Span::Text("解析器：见 ".to_owned()),
                        Span::InlineCode("crates/agent-runtime".to_owned()),
                        Span::Text("。".to_owned()),
                    ],
                },
            ]
        );
    }

    #[test]
    fn every_prefix_of_a_streaming_reply_keeps_the_words_that_have_arrived() {
        // The buffer is re-parsed on every frame while the answer streams in,
        // so the parser really does see each one of these prefixes, and every
        // one of them is somebody's screen for a frame.
        let source = "### 计划\n\nReplace the **fragile** parser with a *real* one.\n\n\
                      - `parse()` first\n  - then `render()`\n\n\
                      ---\n\n```rust\nfn main() {}\n```\n\n> and validate it";
        for (boundary, _) in source.char_indices() {
            let prefix = &source[..boundary];
            let shown = rendered_text(&parse(prefix));
            let mut words: Vec<&str> = prefix.split_whitespace().collect();
            // The last token is whatever the model was in the middle of
            // typing, so only the words it finished are checked.
            words.pop();
            for word in words {
                let bare: String = word
                    .chars()
                    .filter(|character| !"*_`#>-".contains(*character))
                    .collect();
                if bare.is_empty() {
                    continue;
                }
                assert!(
                    shown.contains(&bare),
                    "a {boundary}-byte prefix lost {bare:?}\ninput: {prefix:?}\nshown: {shown:?}"
                );
            }
        }
    }

    #[test]
    fn a_whole_document_survives_the_parser_with_all_of_its_prose_intact() {
        let source = "# Plan\n\n\
                      ## 1. Backend\n\n\
                      **a. Replace the fragile normaliser**\n\n\
                      - *Why:* the current one drops fields\n\
                      - *How:* parse into `Value` first\n\n\
                      ---\n\n\
                      > Nothing here is replayed.\n\n\
                      1. First\n2. Second\n";
        let shown = rendered_text(&parse(source));
        for word in [
            "Plan",
            "Backend",
            "Replace",
            "normaliser",
            "Why:",
            "drops",
            "fields",
            "parse into",
            "Value",
            "replayed",
            "First",
            "Second",
        ] {
            assert!(shown.contains(word), "the parser lost {word:?}");
        }
    }

    #[test]
    fn the_heading_ramp_descends_and_tops_out_at_the_title_token() {
        let sizes: Vec<f32> = (1..=6).map(heading_size).collect();
        assert_eq!(sizes[0], theme::TYPE_TITLE);
        assert_eq!(sizes[5], theme::TYPE_BODY);
        for pair in sizes.windows(2) {
            assert!(
                pair[0] > pair[1],
                "a heading may never be smaller than the one below it: {sizes:?}"
            );
        }
        // Out-of-range levels clamp rather than producing a negative size.
        assert_eq!(heading_size(0), theme::TYPE_TITLE);
        assert_eq!(heading_size(9), theme::TYPE_BODY);
        assert!(heading_is_display(1) && heading_is_display(2));
        assert!(!heading_is_display(3));
    }

    #[test]
    fn only_languages_we_actually_map_reach_the_highlighter() {
        assert_eq!(syntect_language(Some("rs")), Some("rust".to_owned()));
        assert_eq!(syntect_language(Some("Bash")), Some("shell".to_owned()));
        assert_eq!(syntect_language(Some("toml")), Some("ini".to_owned()));
        assert_eq!(syntect_language(Some("brainfuck")), None);
        assert_eq!(syntect_language(Some("")), None);
        assert_eq!(syntect_language(None), None);
    }

    #[test]
    fn the_list_depth_stack_pops_back_out_to_a_shallower_level() {
        let mut nesting = Vec::new();
        assert_eq!(list_depth(&mut nesting, 0), 0);
        assert_eq!(list_depth(&mut nesting, 3), 1);
        assert_eq!(list_depth(&mut nesting, 6), 2);
        assert_eq!(list_depth(&mut nesting, 3), 1);
        assert_eq!(list_depth(&mut nesting, 0), 0);
        // An indent between two open levels replaces the deeper one rather
        // than inventing a third.
        assert_eq!(list_depth(&mut nesting, 4), 1);
        assert_eq!(list_depth(&mut nesting, 2), 1);
    }

    #[test]
    fn a_streaming_answer_lays_out_at_every_width_without_taking_the_window_down() {
        // The parser tests above cover what the renderer is *given*; this
        // covers what it does with it. Laying a galley out and asking it which
        // character the pointer is over is arithmetic on a font this machine
        // loaded at runtime, and the transcript is redrawn at whatever width
        // the user dragged the panel to — including widths narrower than the
        // list indent. Running real frames is what proves none of that panics.
        let source = "# 计划\n\n## 1. Backend\n\n**Replace** the *fragile* parser — see \
                      [the PRD](docs/prd.md) and `crates/agent-runtime`.\n\n\
                      - one\n  - two\n    - three\n      - four\n        - five\n          - six\n\
                      \n1. first\n2. second\n\n---\n\n> Nothing uncertain is replayed.\n\n\
                      ```rust\nfn main() { println!(\"hi\"); }\n```\n\n```\nplain\n```\n\n\
                      Still typing a **bold";
        let ctx = egui::Context::default();
        theme::install(&ctx, crate::theme::Appearance::Dark);
        let tokens = Tokens::for_appearance(crate::theme::Appearance::Dark);
        for measure in [0.0_f32, 12.0, 120.0, 640.0, 1400.0] {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    render(ui, &tokens, source, measure);
                });
            });
        }
    }

    #[test]
    fn an_empty_document_is_an_empty_document() {
        assert!(parse("").is_empty());
        assert!(parse("\n\n   \n").is_empty());
        assert!(parse_inline("").is_empty());
    }
}
