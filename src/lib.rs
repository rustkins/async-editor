//! AsyncEditor
//! Async Console Editor that supports asyncronous output to a resizeable print window. Perfect for an AI Chat Interface.
//!
//! Keyboard Commands:
//!
//! - Arrows, PgUp, PgDn => Move
//! todo - Ctrl-W: Erase the input from the cursor to the previous whitespace
//! todo - Ctrl-U: Erase the input before the cursor
//! - Ctrl-L: Clear the screen
//! - Ctrl-Left / Ctrl-Right: Move to previous/next word
//! - Home: Jump to the start of the line
//! - End: Jump to the end of the line
//! - Ctrl-Up, Ctrl-Down:  Contract / Expand Print vs Edit windows
//! - Ctrl-C: Ignored
//!   Ctrl Left/Right => Move Left/Right by Word
//! - Ctrl PgUp / PgDn - Print History Scrollback, ESC to exit.
//! in dev - Ctrl-k => Delete current line
//!   Ctrl-C, Ctrl-D, Ctrl-Q, Ctrl-X => Exit/Quit
//!
//! Note: this works, but doctest will fail, so doc test have been disabled in Cargo.toml
//! ```rust
//! use async_editor::{EditorEvent, AsyncEditor, Result};
//! use std::io::Write;
//! use std::time::Duration;
//! use tokio::time::sleep;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!   let mut st = r###"LInitial String"###;
//!   let st = st.to_string();
//!   let (mut async_editor, mut async_editor_stdout) = AsyncEditor::new(&st, "Ctrl-C/D/Q/X to Quit".to_string(), 0.5f32, 4)?;
//!   let mut count: u32 = 0;
//!
//!   loop {
//! 	tokio::select! {
//! 	_ = sleep(Duration::from_millis(200)) => {
//! 		count += 1;
//! 		//write!(async_editor_stdout, "{}", format!("_Message {}\nreceived\n!", count))?;
//! 		write!(async_editor_stdout, "{}", format!("_Message {} received!", count))?;
//! 	}
//! 	cmd = async_editor.async_editor() => match cmd {
//! 		Ok(EditorEvent::CtrlC | EditorEvent::CtrlD | EditorEvent::CtrlQ | EditorEvent::CtrlX) => {
//! 			break;
//! 		}
//! 		Ok(EditorEvent::CtrlS) => {
//! 			writeln!(async_editor_stdout, "\n\nCtrlS\n")?;
//! 		}
//! 		Ok(_) => {continue;}
//! 			Err(e) => {
//! 				writeln!(async_editor_stdout, "\n\nError: {e:?}\n")?;
//! 				break;
//! 			}
//! 		}
//! 	}
//! 	async_editor.flush()?;
//!   }
//!   async_editor.flush()?;
//!
//!   let text = async_editor.text();
//!   drop(async_editor);
//!
//!   println!("\n\nEdited Text:\n{}",text);
//!   Ok(())
//! }
//!```

// Note: Extended Grapheme Clusters are NOT rust char, and cannot be converted
//       to / from rust char. This software handles everything as a
//       Extended Grapheme Cluster.
//

use crossterm::{
    QueueableCommand,
    cursor::{self, position},
    event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    style::Print,
    terminal::{self, disable_raw_mode},
};
use futures_util::{FutureExt, StreamExt, select};
use grapheme_utils::*;
use historybuffer::HistoryBuffer;
use std::{
    io::{self, Stdout, Write, stdout},
    ops::DerefMut,
    rc::Rc,
    string::String,
};
use thingbuf::mpsc::{Receiver, Sender, errors::TrySendError};
use unicode_segmentation::UnicodeSegmentation;

mod error;
pub use self::error::{Error, Result};

const HISTORY_BUFFER_SIZE: usize = 300 * 160 * 4;

#[derive(Debug)]
pub enum EditorEvent {
    CtrlC,
    CtrlD,
    CtrlQ,
    CtrlN,
    CtrlS,
    CtrlX,
}

pub enum WriteHistoryType {
    PageUp,
    PageDown,
    Quit,
}

/// AsyncEditor - Multiline Terminal Editor with simultaneous stdout
///
/// AsyncEditor is a functional multline editor supporting standard
/// control keys, while simultaneously supporting stdout printing
/// on the upper portion of the screen.
///
/// Extensible keyboard events are bubbled out for special functions.
///
//
// The main AsyncEditor struct functions as a ReadWriteRouter
pub struct AsyncEditor {
    event_stream: EventStream,    // Crossterm Event Stream
    stdout_rx: Receiver<Vec<u8>>, // Stdout pipe
    editor: Editor,               // Multiline Editor
}

impl AsyncEditor {
    // Create a new `AsyncEditor` instance with an associated
    // [`SharedStdout`]
    pub fn new(
        initial_content: &str,
        split_prompt: String,
        print_height: f32,
        tabstop: u8,
    ) -> Result<(Self, SharedStdout)> {
        let (stdout_tx, stdout_rx) = thingbuf::mpsc::channel(500);

        let editor = Editor::new(initial_content, split_prompt, print_height, tabstop)?;

        let mut async_editor = AsyncEditor {
            event_stream: EventStream::new(),
            stdout_rx,
            editor,
        };
        async_editor.editor.term.queue(terminal::EnableLineWrap)?;
        async_editor.editor.term.flush()?;
        Ok((
            async_editor,
            SharedStdout {
                buf: Vec::new(),
                stdout_tx: stdout_tx,
            },
        ))
    }

    pub fn flush(&mut self) -> Result<()> {
        while let Ok(buf) = self.stdout_rx.try_recv_ref() {
            self.editor.writeout(&buf)?;
        }
        self.editor.term.flush()?;
        Ok(())
    }

    /// Polling function for async_editor, manages all input and output.
    /// Returns either an EditorEvent or an Error
    pub async fn async_editor(&mut self) -> Result<EditorEvent> {
        loop {
            select! {
                event = self.event_stream.next().fuse() => match event {
                    Some(Ok(event)) => {
                        match self.editor.handle_event(event) {
                            Ok(Some(event)) => {
                                self.editor.term.flush()?;
                                return Result::<_>::Ok(event) // Try return Ok(event);
                            },
                            Err(e) => return Err(e),
                            Ok(None) => self.editor.term.flush()?,
                        }
                    }
                    Some(Err(e)) => return Err(e.into()),
                    None => {},
                },
                result = self.stdout_rx.recv_ref().fuse() => match result {
                    Some(buf) => {
                        self.editor.writeout(&buf)?;
                        self.editor.term.flush()?;
                    },
                    None => return Err(Error::SharedStdoutClosed),
                },
            }
        }
    }

    pub fn text(&self) -> String {
        self.editor.text()
    }
}

fn string_to_hex(s: &str, maxlen: usize) -> String {
    let mut new_hex_string = String::with_capacity(s.as_bytes().len() * 2);

    for byte in s.as_bytes().iter() {
        let s = format!("{:02x} ", byte);
        new_hex_string.push_str(&s);
        if new_hex_string.len() >= maxlen {
            return new_hex_string[..maxlen].to_string();
        }
    }
    new_hex_string
}

pub struct Editor {
    curx: u16, // Grapheme Cursor Position
    cury: u16,
    hb_active: bool,
    hb_start_index: usize,
    hb_end_index: usize,
    histbuf: HistoryBuffer, // Make the buffer large enough to hold a huge terminal window screen with LOTS of escape characters
    lidx: usize,
    lines: Vec<String>, // Editor text without \n
    lineidx: usize,     // Which line active
    lofs: usize,
    loose_cursor: bool, // Detects when we've moved off a long line.
    printlines: u16,    // Number of Lines used printing
    printx: u16,        // print cursor pos
    printy: u16,
    scrollstart: usize,
    sizex: u16, // screen size
    sizey: u16,
    split_prompt: String,
    tabstop: u8,
    term: Stdout,
    tmpbuf: Rc<String>,
}

impl Editor {
    pub fn new(
        initial_content: &str,
        split_prompt: String,
        print_height: f32,
        tabstop: u8,
    ) -> Result<Self> {
        let term = stdout();
        let (sizex, sizey) = terminal::size()?;
        let (_curx, cury) = position()?;
        let newprintlines = (sizey as f32 * print_height.max(0.1).min(0.9)) as u16;
        terminal::enable_raw_mode()?;

        Ok(Self {
            curx: 0,
            cury: newprintlines + 2,
            hb_active: false,
            hb_start_index: 0,
            hb_end_index: 0,
            histbuf: HistoryBuffer::new(HISTORY_BUFFER_SIZE), // Make the buffer large enough to hold a huge terminal window screen with LOTS of escape characters
            lidx: 0, // line index of grapheme at the cursor
            lines: initial_content.split("\n").map(|s| s.to_string()).collect(), // convert_tabs(s,'→',8).to_string()).collect(),  // Exlusive \n makes a few painful things easier
            lineidx: 0,
            lofs: 0, // line index offset to the start of the displayed text
            loose_cursor: false,
            printlines: newprintlines,
            printx: 0,
            printy: cury + 1,
            scrollstart: 0,
            sizex: sizex,
            sizey: sizey,
            split_prompt: split_prompt,
            tabstop: tabstop,
            term: term,
            tmpbuf: Rc::new(String::new()), // with_capacity(BUFFER_SIZE)), not needed...  Grows to largest need, then reused
        })
    }

    fn ch(&self, idx: usize) -> char {
        grapheme_at_idx(&self.lines[self.lineidx], idx)
            .chars()
            .next()
            .unwrap_or('\0')
    }

    fn grapheme_idx_at_idx(&self, idx: usize) -> usize {
        grapheme_idx_at_idx(&self.lines[self.lineidx], idx)
    }

    fn grapheme_width_lofs_to_lidx(&self) -> u16 {
        let st = &self.lines[self.lineidx][self.lofs..self.lidx];
        if !st.contains('\t') {
            return string_width(&st) as u16;
        }
        let ofs = string_width(&self.lines[self.lineidx][..self.lofs]) % self.tabstop as usize;
        let mut char_width;
        let mut width = 0;
        for (_, g) in st.grapheme_indices(true) {
            if g == "\t" {
                let ts = self.tabstop as usize;
                char_width = ts - ((width + ofs) % ts);
            } else {
                char_width = string_width(g);
            }
            width += char_width;
        }
        return width as u16;
    }

    pub fn handle_event(&mut self, event: Event) -> Result<Option<EditorEvent>> {
        match event {
            // Doesn't work to detect ctrl-shift  <= a *terminal* thing I thinks
            // Control Keys
            Event::Key(KeyEvent {
                code,
                modifiers: KeyModifiers::CONTROL,
                kind: KeyEventKind::Press,
                ..
            }) => match code {
                // Move to beginning
                KeyCode::Char('a') => {
                    self.lidx = 0;
                    self.lofs = 0;
                    self.setpos()?;
                }
                // End of text (CTRL-C)
                KeyCode::Char('c') => {
                    //if self.should_print_line_on_control_c {
                    //	self.print(&format!("{}{}", self.prompt, self.editor))?;
                    //}
                    return Ok(Some(EditorEvent::CtrlC));
                }
                // End of transmission (CTRL-D)
                KeyCode::Char('d') => {
                    //writeln!(self.term)?;
                    return Ok(Some(EditorEvent::CtrlD));
                }
                // Move to end
                KeyCode::Char('e') => {
                    self.move_end()?;
                }
                KeyCode::Char('l') => {
                    self.printx = 0;
                    self.printy = 0;
                    self.redraw()?;
                }
                KeyCode::Char('n') => {
                    return Ok(Some(EditorEvent::CtrlS));
                }
                KeyCode::Char('q') => {
                    return Ok(Some(EditorEvent::CtrlQ));
                }
                KeyCode::Char('s') => {
                    return Ok(Some(EditorEvent::CtrlS));
                }
                KeyCode::Char('x') => {
                    return Ok(Some(EditorEvent::CtrlX));
                }
                KeyCode::Char('u') => {
                    self.lines[self.lineidx].drain(0..self.lidx);
                    self.redraw()?;
                }
                KeyCode::Down => {
                    self.resize_split(3)?;
                }
                KeyCode::End => {
                    self.lineidx = self.lines.len().saturating_sub(1);
                    self.scrollstart = self.lineidx; //.saturating_sub(1);
                    self.cury = self.printlines + 2; // + (self.lineidx - self.scrollstart) as u16;
                    self.lidx = self.len();
                    self.setpos()?;
                    self.redraw()?;
                }
                KeyCode::Home => {
                    self.lineidx = 0;
                    self.scrollstart = 0;
                    self.cury = self.printlines + 2;
                    self.lidx = 0;
                    self.setpos()?;
                    self.redraw()?;
                }
                // Ctrl-Left  Move cursor left to previous word
                KeyCode::Left => {
                    if self.lidx == 0 {
                        self.move_up(1, true)?;
                        self.lidx = self.len();
                    }
                    while self.lidx > 0 && self.prev_char(self.lidx).is_whitespace() {
                        self.lidx = self.prev_grapheme_idx_from_idx(self.lidx);
                    }
                    while self.lidx > 0 && !self.prev_char(self.lidx).is_whitespace() {
                        self.lidx = self.prev_grapheme_idx_from_idx(self.lidx);
                    }
                    self.setpos()?;
                }
                KeyCode::PageDown => {
                    // Go forward in history, Quit if you're caught up
                    // Only pageUp can activate history...  No need to save the index here
                    self.writehistory(WriteHistoryType::PageDown)?;
                }
                KeyCode::PageUp => {
                    // Go back in history. Activate if necessary
                    if !self.hb_active {
                        self.hb_active = true;
                        self.hb_start_index = self.histbuf.get_last_index();
                        self.hb_end_index = self.hb_start_index;
                    } else {
                        if self.hb_start_index == 0 {
                            return Ok(None);
                        }
                    }
                    self.writehistory(WriteHistoryType::PageUp)?;
                }
                // Ctrl-Right  Move cursor right to next word
                KeyCode::Right => {
                    while self.lidx < self.len() && !self.ch(self.lidx).is_whitespace() {
                        self.lidx = self.next_grapheme_idx_from_idx(self.lidx);
                    }
                    if self.lidx == self.len() {
                        self.move_down(1, true)?;
                        self.lidx = 0;
                    }
                    while self.lidx < self.len() && self.ch(self.lidx).is_whitespace() {
                        self.lidx = self.next_grapheme_idx_from_idx(self.lidx);
                    }
                    self.setpos()?;
                }
                KeyCode::Up => {
                    self.resize_split(-3)?;
                }
                _ => {}
            },
            /////////////////////////////////////////////////////////////////////////////
            // Everything Else
            Event::Key(KeyEvent {
                code,
                modifiers: _,
                kind: KeyEventKind::Press,
                ..
            }) => match code {
                KeyCode::Backspace => {
                    if self.lidx == 0 {
                        if self.lineidx == 0 {
                            return Ok(None);
                        }
                        self.lidx = self.lines[self.lineidx - 1].len();
                        //self.loose_cursor = true;
                        let s = self.lines[self.lineidx].clone();
                        self.lines[self.lineidx - 1].push_str(&s);
                        self.lines.remove(self.lineidx);
                        self.move_up(1, false)?;
                        self.setpos()?;
                        self.redraw()?;
                    } else {
                        if self.lidx > self.len() {
                            self.lidx = self.len();
                            self.lofs = 0;
                        }
                        let start = self.prev_grapheme_idx_from_idx(self.lidx);
                        let mut gwid = self.grapheme_width_lofs_to_lidx(); // width with tabs computed the correct width
                        self.lines[self.lineidx].replace_range(start..self.lidx, "");
                        self.lidx = start;
                        gwid = gwid.saturating_sub(self.grapheme_width_lofs_to_lidx());
                        self.curx = self.curx.saturating_sub(gwid);
                        self.redrawline()?;
                    }
                }
                KeyCode::Char(c) => {
                    self.insert_charstr(&c.to_string())?;
                }
                KeyCode::Delete => {
                    if self.lidx == self.len() {
                        if self.lineidx + 1 < self.lines.len() {
                            let s = self.lines[self.lineidx + 1].clone();
                            self.lines[self.lineidx].push_str(&s);
                            self.lines.remove(self.lineidx + 1);
                            self.redraw()?;
                        }
                    } else {
                        let end = self.next_grapheme_idx_from_idx(self.lidx);
                        self.lines[self.lineidx].replace_range(self.lidx..end, "");
                        self.redrawline()?;
                    }
                }
                KeyCode::Down => {
                    self.move_down(1, false)?;
                    self.redraw()?;
                }
                KeyCode::End => {
                    self.move_end()?;
                }
                KeyCode::Esc => {
                    self.writehistory(WriteHistoryType::Quit)?;
                    self.hb_active = false;
                }
                KeyCode::Enter => {
                    if self.lidx > self.len() {
                        self.lidx = self.len();
                    }
                    self.lines.insert(
                        self.lineidx + 1,
                        self.lines[self.lineidx][self.lidx..].to_string(),
                    );
                    self.lines[self.lineidx].drain(self.lidx..);
                    self.move_down(1, true)?;
                    self.redraw()?;
                }
                KeyCode::Home => {
                    self.lidx = 0;
                    self.lofs = 0;
                    self.setpos()?;
                }
                KeyCode::Left => {
                    if self.lidx == 0 {
                        if self.lineidx == 0 {
                            return Ok(None);
                        }
                        self.move_up(1, true)?;
                        self.lidx = self.len();
                    } else {
                        self.lidx = self.prev_grapheme_idx_from_idx(self.lidx);
                    }
                    self.setpos()?;
                }
                KeyCode::PageDown => {
                    let numlines = self.sizey - self.printlines - 2;
                    self.move_down(numlines, false)?;
                }
                KeyCode::PageUp => {
                    let numlines = self.sizey - self.printlines - 2;
                    self.move_up(numlines, false)?;
                }
                KeyCode::Right => {
                    if self.lidx >= self.len() {
                        if self.lineidx + 1 == self.lines.len() {
                            return Ok(None);
                        }
                        self.move_down(1, true)?;
                        self.lidx = 0;
                    } else {
                        self.lidx = self.next_grapheme_idx_from_idx(self.lidx);
                    }
                    self.setpos()?;
                }
                KeyCode::Tab => {
                    self.insert_charstr("\t")?;
                }
                KeyCode::Up => {
                    self.move_up(1, false)?;
                }
                _ => {}
            },
            Event::Resize(x, y) => {
                let curp: f32 = (self.printlines as f32 / self.sizey as f32)
                    .max(0.9)
                    .min(0.1);
                let delta: i16 = (curp * y as f32) as i16 - self.printlines as i16;

                self.sizex = x;
                self.sizey = y;
                self.resize_split(delta)?;

                self.sizey = y;
            }
            _ => {}
        }
        if false {
            // Debug code
            self.split_prompt = string_to_hex(&self.lines[self.lineidx], 40);
            self.redraw()?;
        }
        self.term.queue(cursor::MoveTo(self.curx, self.cury))?;
        self.term.flush()?;
        Ok(None)
    }

    fn insert_charstr(&mut self, ch: &str) -> Result<()> {
        if self.lidx > self.len() {
            self.lidx = self.len();
            self.lofs = 0;
        }
        //let pre_cnt = self.num_graphemes();
        self.lines[self.lineidx].insert_str(self.lidx, ch);
        //if pre_cnt != self.num_graphemes() {
        self.lidx = self.next_grapheme_idx_from_idx(self.lidx);
        self.curx = self.grapheme_width_lofs_to_lidx();

        self.redrawline()?;
        Ok(())
    }

    fn len(&mut self) -> usize {
        self.lines[self.lineidx].len()
    }

    /// Match the curx position as much as possible moving from line to line
    fn matchpos(&mut self) -> Result<()> {
        // Future work -
        // It's nice for the cursor to stay at nearly the same curx as you move
        // up and down... but it's a feature that can wait.
        self.setpos()?;
        Ok(())
    }

    fn move_down(&mut self, num: u16, move_to_beginning: bool) -> Result<()> {
        self.loose_cursor = true;
        if self.lineidx + 1 == self.lines.len() && self.scrollstart + 1 == self.lines.len() as usize
        {
            self.lidx = self.len();
            self.setpos()?;
            self.redrawline()?;
            return Ok(());
        }

        // To support scrolling beyond the bottom of a full screen, the
        // scrollstart and cury calculations sometimes use a virtual
        // lineidx - the line number that would be printed at the bottom of the
        // screen.
        // The switch occurs when the real bottom line rises above the bottom
        // of the screen, the virtual bottom line numbers.
        let virtidx;
        if self.scrollstart
            <= (self.sizey as usize + 2).saturating_sub((self.cury + self.printlines) as usize)
        {
            virtidx = (self.cury - self.printlines) as usize - 2;
        } else {
            virtidx = self.scrollstart + (self.sizey - self.printlines) as usize - 3;
        }

        self.cury += num;
        self.lineidx = (self.lineidx + num as usize).min(self.lines.len().saturating_sub(1));
        if self.cury > self.sizey - 1 || self.lineidx + 1 == self.lines.len() {
            if num > 10 {
                // Pagedown - Max scrollstart move
                self.scrollstart = (self.scrollstart + num as usize).min(self.lineidx);
            } else {
                // Linedown - Minimum scrollstart
                self.scrollstart = (virtidx + num as usize + self.printlines as usize + 3)
                    .saturating_sub(self.sizey as usize)
                    .min(self.lineidx);
            }
        }
        self.cury = self
            .cury
            .min((self.lineidx - self.scrollstart) as u16 + self.printlines + 2)
            .min(self.sizey - 1);

        if move_to_beginning {
            self.lidx = 0;
            self.lofs = 0;
            self.curx = 0;
        } else {
            self.matchpos()?;
        }
        self.redraw()?;
        Ok(())
    }

    fn move_end(&mut self) -> Result<()> {
        self.lidx = self.len();
        self.setpos()?;
        self.redrawline()?;
        Ok(())
    }

    fn move_up(&mut self, num: u16, move_to_end: bool) -> Result<()> {
        self.loose_cursor = true;

        if self.lineidx == 0 && self.scrollstart == 0 {
            self.lofs = 0;
            self.lidx = 0;
            self.curx = 0;
            self.redrawline()?;
            return Ok(());
        }

        self.lineidx = self.lineidx.saturating_sub(num as usize);
        if self.cury == self.printlines + 2 || self.lineidx < self.scrollstart {
            self.scrollstart = self.scrollstart.saturating_sub(num as usize);
        }
        self.cury = (self.lineidx - self.scrollstart) as u16 + self.printlines + 2; // Possible underflow, but...
        if move_to_end {
            self.lidx = self.len();
        }
        self.matchpos()?;
        self.redraw()?;
        Ok(())
    }

    fn next_grapheme_from_idx(&self, idx: usize) -> &str {
        next_grapheme_from_idx(&self.lines[self.lineidx], idx)
    }

    fn next_grapheme_idx_from_idx(&self, idx: usize) -> usize {
        next_grapheme_idx_from_idx(&self.lines[self.lineidx], idx)
    }

    fn prev_char(&self, idx: usize) -> char {
        self.prev_grapheme_from_idx(idx)
            .chars()
            .next()
            .unwrap_or('\0')
    }

    fn prev_grapheme_from_idx(&self, idx: usize) -> &str {
        prev_grapheme_from_idx(&self.lines[self.lineidx], idx)
    }

    fn prev_grapheme_idx_from_idx(&self, idx: usize) -> usize {
        prev_grapheme_idx_from_idx(&self.lines[self.lineidx], idx)
    }

    pub fn redraw(&mut self) -> Result<()> {
        //   Can't run  "let (cx, cy) = position()?;"  for the case when redraw called from writeout:

        let tbuf = Rc::get_mut(&mut self.tmpbuf).ok_or(Error::RedrawRcError)?;
        //let tbuf = Rc::get_mut(&mut self.tmpbuf).ok_or(Error::Msg("Redraw Rc Error"))?;
        tbuf.clear();
        // Just ='s self.buf.extend(std::iter::repeat("=").take(extend).chain(std::iter::once("\n")).collect::<String>().as_bytes());
        //let s = format!("== {} == {}, c: {} {} cursor: {} {} print: {} {} pline: {} scroll: {}  screen: {} {}  ==", self.split_prompt, s, cx, cy, self.curx, self.cury, self.printx, self.printy, self.printlines, self.scrollstart, self.sizex, self.sizey);

        /* Cursor Debug * /
        let s = format!(
                "== {} ==  l: {} o: {} == curx: {} {} print: {} {} pline: {} lineidx: {} sofs: {}  screen: {} {} == Ctrl-D to Quit ",
                self.split_prompt,
                self.lidx,
                self.lofs,
                self.curx,
                self.cury,
                self.printx,
                self.printy,
                self.printlines,
                self.lineidx,
                self.scrollstart,
                self.sizex,
                self.sizey
            );
        let s = format!(
                "=============== {} ==  s: {} e: {} =======  Ctrl-PgUp/Ctrl-PgDn  ===  Ctrl-C/D/Q/X to Quit  ",
                self.split_prompt,
                self.hb_start_index,
                self.hb_end_index);*/
        let s = format!(
            "=====  AsyncEditor  ========== {} ==  Ctrl ⬅️  / ⮕ / ⬆️/ / ⬇️  ==  Ctrl-PgUp/Ctrl-PgDn  ",
            self.split_prompt
        );
        let extend_count = (self.sizex as usize).saturating_sub(string_width(&s));

        self.term
            .queue(cursor::MoveTo(0, self.printlines + 1 as u16))?;
        self.term
            .queue(terminal::Clear(terminal::ClearType::FromCursorDown))?;
        //tbuf.push_str(&format!("{}{}\n", s, std::iter::repeat("=").take(extend_count).collect::<String>()));
        self.term.queue(Print(&format!(
            "{}{}\n",
            s,
            std::iter::repeat("=")
                .take(extend_count)
                .collect::<String>()
        )))?;
        self.term.queue(cursor::MoveToColumn(0))?;

        let end_index = (self.scrollstart
            + (self.sizey.saturating_sub(self.printlines + 2) as usize))
            .min(self.lines.len());
        for lidx in self.scrollstart..end_index {
            // For each line in range
            if lidx == self.lineidx {
                self.redrawline()?;
                if lidx != end_index - 1 {
                    self.term.queue(cursor::MoveToNextLine(1))?;
                }
                continue;
            }

            let line = &self.lines[lidx];
            let maxwidth = self.sizex as usize - 1;
            let stwidth = string_width(line);
            let mut width = 0usize;
            let mut char_width;
            let mut s = String::with_capacity(200);
            let mut news = String::with_capacity(200);

            match (stwidth > maxwidth, line.contains('\t')) {
                (false, false) => {
                    //Printable"
                    self.term.queue(Print(&line))?;
                }
                (_, _) => {
                    // Too Wide or Tabs or both
                    s.clear();
                    for (_, g) in line.grapheme_indices(true) {
                        news.clear();
                        if g == "\t" {
                            let ts = self.tabstop as usize;
                            char_width = ts - (width % ts);
                            let tab_arrow_string: String =
                                std::iter::repeat("→").take(char_width as usize).collect();
                            news.push_str(&tab_arrow_string);
                        } else {
                            char_width = string_width(g);
                            news.push_str(g);
                        }
                        if width + char_width as usize > maxwidth {
                            break;
                        }
                        s.push_str(&news);
                        width += char_width;
                    }
                    self.term.queue(Print(&s))?;
                    if stwidth > maxwidth {
                        self.term.queue(cursor::MoveToColumn(self.sizex - 1))?;
                        self.term.queue(Print(&'>'))?;
                    }
                }
            }
            if lidx != end_index - 1 {
                self.term.queue(cursor::MoveToNextLine(1))?;
            }
        }
        self.term.queue(cursor::MoveTo(self.curx, self.cury))?;
        self.term.flush()?;
        Ok(())
    }

    fn redrawline(&mut self) -> Result<()> {
        self.term
            .queue(terminal::Clear(terminal::ClearType::CurrentLine))?;
        self.term.queue(cursor::MoveToColumn(0))?;
        //if self.lofs > self.len() { self.lofs = 0; }
        let start = if self.lofs > self.len() {
            0
        } else {
            self.grapheme_idx_at_idx(self.lofs)
        };

        // Current line may start at lofs
        let line = &self.lines[self.lineidx][start..];
        let maxwidth = self.sizex as usize - 2;
        let stwidth = string_width(line);
        let mut width = 0usize;
        let mut char_width;
        let mut s = String::with_capacity(200);
        let mut news = String::with_capacity(200);

        match (stwidth > maxwidth, line.contains('\t')) {
            (false, false) => {
                //Printable"
                //tbuf.push_str(line);
                self.term.queue(Print(&line))?;
            }
            (_, _) => {
                // Too Wide or Tabs or both
                s.clear();
                for (_, g) in line.grapheme_indices(true) {
                    news.clear();
                    if g == "\t" {
                        let ts = self.tabstop as usize;
                        char_width = ts - (width % ts);
                        let tab_arrow_string: String =
                            std::iter::repeat("→").take(char_width as usize).collect();
                        news.push_str(&tab_arrow_string);
                    } else {
                        char_width = string_width(g);
                        news.push_str(g);
                    }
                    if width + char_width as usize > maxwidth + 1 {
                        break;
                    }
                    s.push_str(&news);
                    width += char_width;
                }
                //tbuf.push_str(&line[0..end]);
                self.term.queue(Print(&s))?;
            }
        }
        self.term.queue(cursor::MoveTo(self.curx, self.cury))?;
        Ok(())
    }

    fn resize_split(&mut self, delta: i16) -> Result<()> {
        self.term
            .queue(cursor::MoveTo(self.printx, self.printy as u16))?;
        self.term
            .queue(terminal::Clear(terminal::ClearType::FromCursorDown))?;
        let pre = self.printlines as i16;
        self.printlines = (self.printlines as i16 + delta)
            .max((self.sizey >> 3) as i16)
            .min(self.sizey as i16 - 8) as u16;
        let new_delta = self.printlines as i16 - pre;
        self.cury = (self.cury as i16 + new_delta).max(self.printlines as i16 + 2) as u16;
        while self.cury >= self.sizey {
            self.cury -= 1;
            self.lineidx -= 1;
        }
        self.setpos()?;
        self.writebuf(&[])?;
        Ok(())
    }

    // Set self.curx / self.lofs so self.lidx is visable (string_width[lofs to idx] is < maxsize
    fn setpos(&mut self) -> Result<()> {
        let maxwidth = self.sizex - 1;
        self.lidx = self.grapheme_idx_at_idx(self.lidx);

        // loose_cursor - Signals that the cursor just moved off a possibly long line
        // We want to stay loose until the user starts a changing action, AND
        // We want to keep the current alignment if we're on another long line.
        if self.loose_cursor {
            // Lean toward resetting lofs, but don't for other long lines
            if self.lidx < (self.lofs + (self.sizey << 2) as usize + 15) {
                // really short line detection  - Know changed mode.
                self.lofs = 0;
            } else {
                //self.lofs = grapheme_idx_at_idx(&self.lines[self.lineidx], self.lofs); // Fix when lofs hits in the middle of a char
                self.lofs = self.grapheme_idx_at_idx(self.lofs); // Fix when lofs hits in the middle of a char
            }
        }
        if self.lidx < self.lofs {
            // really short line detection  - Know changed mode.
            self.lofs = 0;
        }
        self.loose_cursor = false;

        let mut stwidth = self.grapheme_width_lofs_to_lidx();

        loop {
            if stwidth <= maxwidth {
                self.curx = stwidth;
                return Ok(());
            }
            // It would be nice to just subtract the first char width, but for tabs
            //stwidth -= self.string_width_at_idx(self.lofs);
            self.lofs = self.next_grapheme_idx_from_idx(self.lofs);
            stwidth = self.grapheme_width_lofs_to_lidx();
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    fn writebuf(&mut self, buf: &[u8]) -> Result<()> {
        // The common writing worker

        // The caller is responsible for restoring the cursor to self.printx/y
        // and clearing the screen below

        for line in buf.split_inclusive(|b| *b == b'\n') {
            self.term.write_all(line)?;
            self.term.flush()?;
            if line.ends_with(b"\n") {
                self.term.queue(cursor::MoveToColumn(0))?;
                self.printx = 0;
                self.printy += 1;
            } else {
                let mut x = self.printx as usize + line.len(); // Todo - count graphemes!
                while x > self.sizex as usize {
                    x -= self.sizex as usize;
                    self.printy += 1;
                }
                self.printx = x as u16;
            }
        }

        self.printy = self.printy.min(self.sizey);
        if self.printy > self.printlines {
            self.term
                .queue(terminal::ScrollUp(self.printy - self.printlines))?;
            self.printy = self.printlines;
        }

        self.redraw()?;
        Ok(())
    }

    fn writehistory(&mut self, write_history_type: WriteHistoryType) -> Result<()> {
        self.term.queue(cursor::MoveTo(0, 0))?;
        self.term.queue(terminal::Clear(terminal::ClearType::All))?;

        self.printx = 0;
        self.printy = 0;

        //let mut limit = self.printlines * self.sizey - 1;
        let mut linecnt = self.sizex;
        let mut num_lines = 0;
        let mut buf = Vec::<u8>::with_capacity(self.printlines as usize * self.sizex as usize);
        let mut revbuf: Vec<u8> = vec![];

        match write_history_type {
            WriteHistoryType::PageDown => {
                self.hb_start_index = self.hb_end_index;
                while let Some(ch) = self.histbuf.get(self.hb_start_index + buf.len()) {
                    buf.push(ch);
                    linecnt -= 1;
                    if ch == b'\n' || linecnt == 0 {
                        linecnt = self.sizex - 1;
                        num_lines += 1;
                        if num_lines >= self.printlines {
                            break;
                        }
                    }
                }
                self.hb_end_index = self.hb_start_index + buf.len();
                if num_lines < self.printlines {
                    // Back at the bottom
                    buf = self
                        .histbuf
                        .get_recent(self.printlines as usize * self.sizex as usize);
                    self.writebuf(&buf)?;
                    self.hb_active = false;
                } else {
                    self.writebuf(&buf)?;
                }
            }
            WriteHistoryType::PageUp => {
                self.hb_end_index = self.hb_start_index;
                while let Some(ch) = self.histbuf.get(self.hb_start_index) {
                    buf.push(ch);
                    linecnt -= 1;
                    if ch == b'\n' || linecnt == 0 {
                        linecnt = self.sizex - 1;
                        num_lines += 1;
                        if num_lines >= self.printlines {
                            break;
                        }
                    }
                    if self.hb_start_index == 0 {
                        break;
                    }
                    self.hb_start_index = self.hb_start_index.saturating_sub(1);
                }
                if self.hb_start_index == 0 {
                    // Load a full page
                    let mut linecnt = self.sizex;
                    let mut num_lines = 0;
                    revbuf =
                        Vec::<u8>::with_capacity(self.printlines as usize * self.sizex as usize);
                    while let Some(ch) = self.histbuf.get(self.hb_start_index + revbuf.len()) {
                        revbuf.push(ch);
                        linecnt -= 1;
                        if ch == b'\n' || linecnt == 0 {
                            linecnt = self.sizex - 1;
                            num_lines += 1;
                            if num_lines >= self.printlines {
                                break;
                            }
                        }
                    }
                    self.hb_end_index = revbuf.len();
                } else {
                    revbuf = buf.into_iter().rev().collect();
                }
                self.writebuf(&revbuf)?;
            }
            WriteHistoryType::Quit => {
                buf = self
                    .histbuf
                    .get_recent(self.printlines as usize * self.sizex as usize);
                self.writebuf(&buf)?;
                self.hb_active = false;
            }
        }
        Ok(())
    }

    fn writeout(&mut self, buf: &[u8]) -> Result<()> {
        // Can't request position: let (cx, cy) = position()?; // Causes Timeout:
        self.histbuf.add(buf);
        if !self.hb_active {
            self.term
                .queue(cursor::MoveTo(self.printx, self.printy as u16))?;
            self.term
                .queue(terminal::Clear(terminal::ClearType::FromCursorDown))?;
            self.writebuf(buf)?;
        }
        Ok(())
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        self.term.queue(cursor::MoveTo(0, self.sizey - 1)).unwrap();
        self.term.queue(cursor::MoveToNextLine(1)).unwrap();
        self.term.flush().unwrap();
    }
}

#[pin_project::pin_project]
pub struct SharedStdout {
    #[pin]
    buf: Vec<u8>,
    stdout_tx: Sender<Vec<u8>>,
}

impl io::Write for SharedStdout {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        if true {
            match self.stdout_tx.try_send_ref() {
                Ok(mut send_buf) => {
                    std::mem::swap(send_buf.deref_mut(), &mut self.buf);
                    self.buf.clear();
                }
                Err(TrySendError::Full(_)) => return Err(io::ErrorKind::WouldBlock.into()),
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        "io Error: ThingBuf Receiver Closed",
                    ));
                }
            }
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
