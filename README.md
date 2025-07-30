# Async_Editor

## What is this?

Async-Editor is a console editor that supports asyncronous output to a resizable print window. 

It was design to be the perfect interface for an AI Chat Interface.

It supports Extended Grapheme Clusters


## Keyboard 

Ctrl-L			Clear the Print Window

Ctrl-Left/Rt		Move to the Previous/Next Word

Ctrl-Up/Down 		Resize the window split up and down

Ctrl-PgUp/PgDown	Freeze and look through the Print Window History. (Ctrl PgDn or ESC to resume live updated)

Home/End		Beginning/End of the current line



Ctrl-C/D/Q/X		Currently enabled Special Commands (Quit in the example below)


## Example Usage

```rust
//use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use async_editor::{EditorEvent, AsyncEditor, Result};
use std::io::Write;
use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<()> {
    let mut st = "Initial Text".to_string();

    let (mut async_editor, mut async_editor_stdout) =
        AsyncEditor::new(&st, "Ctrl-C/D/Q/X to Quit".to_string(), 0.5f32, 4)?;

    let mut count: u32 = 0;
    loop {
        tokio::select! {
            _ = sleep(Duration::from_millis(200)) => {
                count += 1;
                write!(async_editor_stdout, "{}", format!("_Message {}\nreceived\n!", count))?;
                //write!(async_editor_stdout, "{}", format!("_Message {} received!", count))?;
            }
            cmd = async_editor.async_editor() => match cmd {
                Ok(EditorEvent::CtrlC | EditorEvent::CtrlD | EditorEvent::CtrlQ | EditorEvent::CtrlX) => {
                    writeln!(async_editor_stdout, "\n\nQuitting...\n")?;
                    break;
                }
                Ok(EditorEvent::CtrlS) => {
                writeln!(async_editor_stdout, "\n\nCtrlS\n")?;
                }
                Ok(_) => {continue;}
                Err(e) => {
                    writeln!(async_editor_stdout, "\n\nError: {e:?}\n")?;
                    break;
                }
            }
        }
        async_editor.flush()?;
    }
    async_editor.flush()?;
    let _text = async_editor.text();
    drop(async_editor);
    println!("\n\nGot:\n{}",text);
    Ok(())
}
```

# Installation

Add `async_editor` to your `Cargo.toml`:

```toml
[dependencies]
async_editor = "0.1"
```

