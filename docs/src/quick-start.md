<p align="center">
  <a href="README.md"><img src="purrcode-logo.png" alt="PurrCode" width="180"></a>
</p>

# Quick Start

## 1. Initialize PurrCode

```bash
purrcode init
```

Initialization:

- discovers supported local providers;
- creates secure local configuration;
- initializes persistence;
- creates a managed workspace;
- starts the authenticated loopback daemon;
- verifies daemon readiness;
- reports sandbox capability.

## 2. Open a repository

```bash
cd path/to/your-project
```

## 3. Start the terminal Workbench — or the native IDE

```bash
purrcode          # terminal Workbench (default on every platform)
purrcode ide      # native desktop IDE (same as purrcode gui)
```

## 4. Connect a model

Inside the TUI or IDE:

```text
/connect
```

Or import a provider configuration example:

```text
/connect import
```

Credentials are stored in the operating-system credential store and never enter the model context or tool processes.

## 5. Submit a task

Examples:

```text
Explain the architecture of this repository.
```

```text
Add pagination to the orders API and update the tests.
```

```text
Review the current diff and identify possible regressions.
```

The composer supports multiline text, source code, logs, JSON, YAML, TOML, and pasted scripts.

### Composer shortcuts

```text
Enter          Insert newline
Ctrl+G         Submit
Ctrl+C         Cancel active work
```

### TUI shortcuts

```text
Ctrl+B         Toggle workspace panel
Ctrl+D         Open diff
Ctrl+P         Open command palette
Ctrl+K         Switch task mode
Space or E     Expand selected timeline card
Mouse click    Expand a timeline detail card
Up/Down        Scroll the timeline
Mouse wheel    Scroll the timeline
?              Open help
Ctrl+C         Cancel active work
```

## Typical workflow

```text
Describe task
  ↓
Review plan
  ↓
Inspect retrieved context
  ↓
Approve or reject proposed actions
  ↓
Review execution output
  ↓
Inspect diff
  ↓
Review validation evidence
  ↓
Apply, commit, export, or rollback
```

For the full command and slash-command list, see [CLI Reference](cli-reference.md).
