<p align="center">
  <a href="README.md"><img src="purrcode-logo.png" alt="PurrCode" width="180"></a>
</p>

# CLI Reference

## General

```bash
purrcode init                          # discover local providers, write secure defaults, start the daemon
purrcode                               # terminal Workbench (default)
purrcode ide                           # native desktop IDE (same as purrcode gui)
purrcode studio                        # authenticated browser maintenance client
purrcode plan "Describe the change"    # plan first, write nothing
purrcode run "Implement the change"    # implement and validate
purrcode sessions                      # review active sessions
purrcode review                        # review the current diff
purrcode approve                       # approve a proposed action
purrcode reject                        # reject a proposed action
purrcode cancel                        # cancel a session
purrcode resume                        # resume a paused plan in the same session
purrcode rollback                      # roll back isolated work
purrcode doctor                        # environment diagnostics
purrcode export-patch                  # export the isolated-worktree patch
purrcode apply                         # apply the reviewed patch
```

## Providers

```text
/connect
/connect import
/provider list
/provider edit <name>
/provider test <name>
/provider remove <name>
```

## Models

```text
/model recommend
/model qualify <model>
/model loaded
/model unload <model>
/model unload-all
```

## Skills, MCP, and capability

```text
/skills search <query>
/mcp search <query>
/capability add <description>
```

## Credentials and configuration

```bash
purrcode credential set <provider>     # store a keychain reference
purrcode credential delete <name>
purrcode config migrate                # migrate configuration files
purrcode config migration-preview      # preview a configuration migration
```

## Upgrade

```bash
purrcode upgrade check --channel stable
purrcode upgrade download /tmp/purrcode-release.tar.gz
purrcode upgrade install --channel stable
purrcode upgrade rollback
```

## Evaluation and evidence

```bash
purrcode benchmark list
purrcode benchmark validate-cases
purrcode benchmark run --output benchmark.json
purrcode benchmark report benchmark.json
purrcode trace show latest
purrcode trace export latest
purrcode explain completion latest
purrcode bundle export latest evidence.json
purrcode bundle verify evidence.json
purrcode bundle replay evidence.json
```

## TUI controls

```text
Enter          Insert newline
Ctrl+G         Submit
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

Terminal support for modified Enter combinations varies. The TUI footer displays the portable active shortcut.
