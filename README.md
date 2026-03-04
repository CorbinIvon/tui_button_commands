# Button to Command Executor

A powerful TUI (Terminal User Interface) Controller built with Rust and Ratatui. This application allows you to manage multiple background processes as "buttons," providing real-time interaction, lifecycle controls, and persistent logging.

## Features

- **Background Execution**: Run any shell command in a dedicated background thread.
- **Hooked Interaction**: "Focus" a running command to hook your keyboard directly to its `stdin`. Ideal for `sudo` prompts or interactive CLIs.
- **Process Lifecycle Management**:
  - **[ SUSPEND ] / [ RESUME ]**: Pause and continue processes using `SIGSTOP` and `SIGCONT`.
  - **[ KILL ]**: Send `SIGINT` (Ctrl+C) to a focused process or `SIGKILL` to a background one.
- **Output Streaming**: Real-time output capture. Every command run is streamed to a log file in the `output/` directory.
- **Visual Notifications**: The "FOCUS" button flashes Red/Gray when a command detects keywords like "password" or "sudo" in its output.
- **Smart Sudo**: Automatically injects the `-S` flag to `sudo` commands to ensure they work correctly with the piped interaction screen.
- **Persistence**: Your button layout and command strings are automatically saved to `commands.json`.
- **Advanced Scrolling**:
  - Mouse wheel support for the main list and log views.
  - PageUp/PageDown/Home/End support for fast navigation.
  - Clamped viewport logic to prevent "scrolling into the void."

## Getting Started

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (latest stable)
- A terminal with mouse support (recommended)

### Running the Application

```bash
cd button_to_command
cargo run
```

## Key Controls

### Main Screen
- **Click (Add Button Command)**: Create a new command row.
- **Click Input**: Focus a row to edit the command string.
- **Enter (on input)**: Unfocus and save the command.
- **Arrows Up/Down**: Scroll the button list.
- **PgUp/PgDn**: Fast scroll the button list.
- **'q'**: Quit the application (when no input is focused).

### Interaction / View Output Screen
- **Esc**: Return to the main screen.
- **Ctrl + C**: Send `SIGINT` to the running process.
- **Keys**: All other keys are piped directly to the process's `stdin`.
- **Arrows / Mouse Wheel**: Scroll through history (in "View Output" mode).

## Project Structure

- `button_to_command/`: The main application logic.
- `framework/`: A local component-based TUI framework.
- `commands.json`: Persistent storage for your command buttons.
- `output/`: Directory containing `.log` files for every command execution.

## Technical Notes

The application uses `setsid` to isolate background processes from the main TUI terminal. This prevents terminal escape sequences (like mouse movement) from being misinterpreted by the child processes. Ensure your commands are compatible with non-interactive pipes (e.g., use `sudo -S`).
