// A small VT/ANSI terminal emulator for PurrCode Studio (PRD §24.7).
//
// The Studio previously stripped escape sequences and printed the transcript as
// text, which cannot show a progress bar, a cleared screen, a coloured test
// summary, or a cursor. This module keeps a real screen buffer and applies the
// sequences a build/test/shell session actually emits.
//
// It is deliberately hand-written rather than a vendored bundle: the shell
// serves three inlined assets under a strict same-origin policy with no build
// step and no CDN, so a dependency would have to be committed as a minified
// blob nobody in this repository can review.

const DEFAULT_FG = 7;
const DEFAULT_BG = 0;

function blankCell() {
  return { ch: " ", fg: DEFAULT_FG, bg: DEFAULT_BG, bold: false, inverse: false, underline: false };
}

export class Terminal {
  constructor(host, { rows = 30, cols = 100, onInput = () => {}, onResize = () => {} } = {}) {
    this.host = host;
    this.rows = rows;
    this.cols = cols;
    this.onInput = onInput;
    this.onResize = onResize;
    this.reset();
    host.classList.add("terminal-screen");
    host.setAttribute("tabindex", "0");
    host.setAttribute("role", "log");
    host.addEventListener("keydown", (event) => this.handleKey(event));
    this.render();
  }

  reset() {
    this.buffer = Array.from({ length: this.rows }, () => Array.from({ length: this.cols }, blankCell));
    this.cursor = { row: 0, col: 0, visible: true };
    this.style = blankCell();
    this.scrollTop = 0;
    this.scrollBottom = this.rows - 1;
    this.pending = "";
  }

  resize(rows, cols) {
    if (rows === this.rows && cols === this.cols) return;
    const previous = this.buffer;
    this.rows = Math.max(1, rows);
    this.cols = Math.max(1, cols);
    this.buffer = Array.from({ length: this.rows }, (_, r) =>
      Array.from({ length: this.cols }, (_, c) => previous[r]?.[c] ?? blankCell())
    );
    this.scrollTop = 0;
    this.scrollBottom = this.rows - 1;
    this.cursor.row = Math.min(this.cursor.row, this.rows - 1);
    this.cursor.col = Math.min(this.cursor.col, this.cols - 1);
    this.onResize(this.rows, this.cols);
    this.render();
  }

  // ── Parsing ──────────────────────────────────────────────────
  //
  // Incomplete sequences are held in `pending` so a chunk boundary that lands
  // mid-escape does not print the escape as literal text.
  write(text) {
    let input = this.pending + text;
    this.pending = "";
    let index = 0;
    while (index < input.length) {
      const character = input[index];
      if (character === "\x1b") {
        const consumed = this.escape(input, index);
        if (consumed === null) {
          this.pending = input.slice(index);
          break;
        }
        index = consumed;
        continue;
      }
      this.printable(character);
      index += 1;
    }
    // A partial sequence that never completes must not grow without bound.
    if (this.pending.length > 256) this.pending = "";
    this.render();
  }

  escape(input, start) {
    const next = input[start + 1];
    if (next === undefined) return null;
    if (next === "[") return this.csi(input, start);
    if (next === "]") return this.osc(input, start);
    // Single-character escapes we care about; anything else is skipped so it
    // never reaches the screen as text.
    if (next === "M") {
      this.scrollDown(1);
      return start + 2;
    }
    if (next === "c") {
      this.reset();
      return start + 2;
    }
    return start + 2;
  }

  csi(input, start) {
    let index = start + 2;
    let parameters = "";
    while (index < input.length && /[0-9;?<>! ]/.test(input[index])) {
      parameters += input[index];
      index += 1;
    }
    if (index >= input.length) return null;
    const final = input[index];
    const numbers = parameters
      .replace(/[^0-9;]/g, "")
      .split(";")
      .map((value) => (value === "" ? null : Number(value)));
    const at = (position, fallback) => (numbers[position] ?? fallback);

    switch (final) {
      case "A": this.moveCursor(-at(0, 1), 0); break;
      case "B": this.moveCursor(at(0, 1), 0); break;
      case "C": this.moveCursor(0, at(0, 1)); break;
      case "D": this.moveCursor(0, -at(0, 1)); break;
      case "E": this.cursor.row = this.clampRow(this.cursor.row + at(0, 1)); this.cursor.col = 0; break;
      case "F": this.cursor.row = this.clampRow(this.cursor.row - at(0, 1)); this.cursor.col = 0; break;
      case "G": this.cursor.col = this.clampCol(at(0, 1) - 1); break;
      case "H":
      case "f":
        this.cursor.row = this.clampRow(at(0, 1) - 1);
        this.cursor.col = this.clampCol(at(1, 1) - 1);
        break;
      case "J": this.eraseDisplay(at(0, 0)); break;
      case "K": this.eraseLine(at(0, 0)); break;
      case "L": this.insertLines(at(0, 1)); break;
      case "M": this.deleteLines(at(0, 1)); break;
      case "P": this.deleteCharacters(at(0, 1)); break;
      case "S": this.scrollUp(at(0, 1)); break;
      case "T": this.scrollDown(at(0, 1)); break;
      case "X": this.eraseCharacters(at(0, 1)); break;
      case "d": this.cursor.row = this.clampRow(at(0, 1) - 1); break;
      case "m": this.applyStyle(numbers); break;
      case "r":
        this.scrollTop = this.clampRow(at(0, 1) - 1);
        this.scrollBottom = this.clampRow(at(1, this.rows) - 1);
        break;
      case "h": if (parameters.includes("?25")) this.cursor.visible = true; break;
      case "l": if (parameters.includes("?25")) this.cursor.visible = false; break;
      default: break;
    }
    return index + 1;
  }

  osc(input, start) {
    // OSC runs until BEL or ST; it sets titles and hyperlinks, none of which
    // belong on the screen.
    for (let index = start + 2; index < input.length; index += 1) {
      if (input[index] === "\x07") return index + 1;
      if (input[index] === "\x1b" && input[index + 1] === "\\") return index + 2;
    }
    return null;
  }

  printable(character) {
    switch (character) {
      case "\n": this.lineFeed(); return;
      case "\r": this.cursor.col = 0; return;
      case "\b": this.cursor.col = Math.max(0, this.cursor.col - 1); return;
      case "\t": this.cursor.col = Math.min(this.cols - 1, (Math.floor(this.cursor.col / 8) + 1) * 8); return;
      case "\x07": return;
      default: break;
    }
    if (character < " ") return;
    if (this.cursor.col >= this.cols) {
      this.cursor.col = 0;
      this.lineFeed();
    }
    this.buffer[this.cursor.row][this.cursor.col] = { ...this.style, ch: character };
    this.cursor.col += 1;
  }

  // ── Screen operations ────────────────────────────────────────
  clampRow(row) { return Math.min(this.rows - 1, Math.max(0, row)); }
  clampCol(col) { return Math.min(this.cols - 1, Math.max(0, col)); }

  moveCursor(rows, cols) {
    this.cursor.row = this.clampRow(this.cursor.row + rows);
    this.cursor.col = this.clampCol(this.cursor.col + cols);
  }

  lineFeed() {
    if (this.cursor.row === this.scrollBottom) this.scrollUp(1);
    else this.cursor.row = this.clampRow(this.cursor.row + 1);
  }

  blankRow() { return Array.from({ length: this.cols }, blankCell); }

  scrollUp(count) {
    for (let step = 0; step < count; step += 1) {
      this.buffer.splice(this.scrollTop, 1);
      this.buffer.splice(this.scrollBottom, 0, this.blankRow());
    }
  }

  scrollDown(count) {
    for (let step = 0; step < count; step += 1) {
      this.buffer.splice(this.scrollBottom, 1);
      this.buffer.splice(this.scrollTop, 0, this.blankRow());
    }
  }

  insertLines(count) {
    for (let step = 0; step < count; step += 1) {
      this.buffer.splice(this.scrollBottom, 1);
      this.buffer.splice(this.cursor.row, 0, this.blankRow());
    }
  }

  deleteLines(count) {
    for (let step = 0; step < count; step += 1) {
      this.buffer.splice(this.cursor.row, 1);
      this.buffer.splice(this.scrollBottom, 0, this.blankRow());
    }
  }

  deleteCharacters(count) {
    const row = this.buffer[this.cursor.row];
    row.splice(this.cursor.col, count);
    while (row.length < this.cols) row.push(blankCell());
  }

  eraseCharacters(count) {
    for (let col = this.cursor.col; col < Math.min(this.cols, this.cursor.col + count); col += 1) {
      this.buffer[this.cursor.row][col] = blankCell();
    }
  }

  eraseLine(mode) {
    const row = this.buffer[this.cursor.row];
    const from = mode === 1 ? 0 : mode === 2 ? 0 : this.cursor.col;
    const to = mode === 1 ? this.cursor.col + 1 : this.cols;
    for (let col = from; col < to; col += 1) row[col] = blankCell();
  }

  eraseDisplay(mode) {
    if (mode === 2 || mode === 3) {
      this.buffer = Array.from({ length: this.rows }, () => this.blankRow());
      return;
    }
    const first = mode === 1 ? 0 : this.cursor.row;
    const last = mode === 1 ? this.cursor.row : this.rows - 1;
    for (let row = first; row <= last; row += 1) {
      if (row === this.cursor.row) {
        this.cursor.row = row;
        this.eraseLine(mode === 1 ? 1 : 0);
      } else {
        this.buffer[row] = this.blankRow();
      }
    }
  }

  applyStyle(numbers) {
    const values = numbers.length ? numbers : [0];
    for (let index = 0; index < values.length; index += 1) {
      const code = values[index] ?? 0;
      if (code === 0) { this.style = blankCell(); continue; }
      if (code === 1) { this.style.bold = true; continue; }
      if (code === 4) { this.style.underline = true; continue; }
      if (code === 7) { this.style.inverse = true; continue; }
      if (code === 22) { this.style.bold = false; continue; }
      if (code === 24) { this.style.underline = false; continue; }
      if (code === 27) { this.style.inverse = false; continue; }
      if (code >= 30 && code <= 37) { this.style.fg = code - 30; continue; }
      if (code >= 90 && code <= 97) { this.style.fg = code - 90 + 8; continue; }
      if (code >= 40 && code <= 47) { this.style.bg = code - 40; continue; }
      if (code >= 100 && code <= 107) { this.style.bg = code - 100 + 8; continue; }
      if (code === 39) { this.style.fg = DEFAULT_FG; continue; }
      if (code === 49) { this.style.bg = DEFAULT_BG; continue; }
      // 256-colour and truecolour: consume the parameters, keep the nearest
      // basic colour rather than dropping the sequence into the text.
      if (code === 38 || code === 48) {
        const target = code === 38 ? "fg" : "bg";
        if (values[index + 1] === 5) { this.style[target] = (values[index + 2] ?? DEFAULT_FG) % 16; index += 2; }
        else if (values[index + 1] === 2) { this.style[target] = DEFAULT_FG; index += 4; }
      }
    }
  }

  // ── Rendering ────────────────────────────────────────────────
  render() {
    const lines = this.buffer.map((row, rowIndex) => {
      let html = "";
      let run = null;
      const flush = () => {
        if (!run) return;
        const classes = ["c"];
        classes.push(`fg${run.style.inverse ? run.style.bg : run.style.fg}`);
        classes.push(`bg${run.style.inverse ? run.style.fg : run.style.bg}`);
        if (run.style.bold) classes.push("b");
        if (run.style.underline) classes.push("u");
        html += `<span class="${classes.join(" ")}">${escapeText(run.text)}</span>`;
        run = null;
      };
      for (let colIndex = 0; colIndex < row.length; colIndex += 1) {
        const cell = row[colIndex];
        const isCursor =
          this.cursor.visible && rowIndex === this.cursor.row && colIndex === this.cursor.col;
        const style = isCursor ? { ...cell, inverse: !cell.inverse } : cell;
        if (run && sameStyle(run.style, style)) run.text += cell.ch;
        else { flush(); run = { style, text: cell.ch }; }
      }
      flush();
      return `<div class="terminal-row">${html}</div>`;
    });
    this.host.innerHTML = lines.join("");
  }

  // ── Input ────────────────────────────────────────────────────
  handleKey(event) {
    // Let the browser handle copy and select-all so terminal text stays
    // selectable (PRD §19.1 "copy and selection").
    if ((event.ctrlKey || event.metaKey) && ["c", "a", "v"].includes(event.key.toLowerCase())) {
      if (event.metaKey || (event.ctrlKey && event.key.toLowerCase() !== "c")) return;
      if (window.getSelection()?.toString()) return;
    }
    const bytes = keyToBytes(event);
    if (bytes === null) return;
    event.preventDefault();
    this.onInput(bytes);
  }
}

function sameStyle(left, right) {
  return (
    left.fg === right.fg &&
    left.bg === right.bg &&
    left.bold === right.bold &&
    left.inverse === right.inverse &&
    left.underline === right.underline
  );
}

function escapeText(value) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/ /g, " ");
}

export function keyToBytes(event) {
  const named = {
    Enter: "\r",
    Backspace: "\x7f",
    Tab: "\t",
    Escape: "\x1b",
    ArrowUp: "\x1b[A",
    ArrowDown: "\x1b[B",
    ArrowRight: "\x1b[C",
    ArrowLeft: "\x1b[D",
    Home: "\x1b[H",
    End: "\x1b[F",
    PageUp: "\x1b[5~",
    PageDown: "\x1b[6~",
    Delete: "\x1b[3~",
    Insert: "\x1b[2~",
  };
  if (named[event.key] !== undefined) return named[event.key];
  if (event.ctrlKey && /^[a-zA-Z]$/.test(event.key)) {
    return String.fromCharCode(event.key.toUpperCase().charCodeAt(0) - 64);
  }
  if (event.altKey && event.key.length === 1) return `\x1b${event.key}`;
  if (event.key.length === 1 && !event.metaKey && !event.ctrlKey) return event.key;
  return null;
}

/// Rows and columns that fit `host`, measured from a real character cell so the
/// PTY window matches what the user sees.
export function measure(host) {
  const probe = document.createElement("span");
  probe.className = "terminal-probe";
  probe.textContent = "M";
  host.appendChild(probe);
  const rect = probe.getBoundingClientRect();
  probe.remove();
  const width = rect.width || 8;
  const height = rect.height || 16;
  return {
    cols: Math.max(20, Math.floor(host.clientWidth / width)),
    rows: Math.max(6, Math.floor(host.clientHeight / height)),
  };
}
