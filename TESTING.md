# Testing and updating the running app

tuimail is Rust; ratatui draws it, and the `/` palette is the shared fzt engine
(`fzt.dll` from the managed fzt install). CLAUDE.md has the commands, the security
rules and the architecture; this file is how to check a change and how to bring
Nelson's open window up to date.

## Automated checks

From the feature worktree:

```powershell
cargo fmt --check
cargo clippy --locked --all-targets -j 4 -- -D warnings
cargo test --locked -j 4
```

Keep local builds to four jobs (nelsonpc is a gaming PC), and never install a local
build: releases come from CI. A PR runs pipeline-templates' rust suite: fmt, clippy and the
tests on Windows; on Linux the tests with GreenMail on the job's own network, which runs
`live.rs` (its header has the local run); cargo-audit when `Cargo.lock` changes. A merge only
builds and releases. UI
behavior is tested by rendering into ratatui's `TestBackend` (`ui/` tests,
`app::tests::app()`); add a focused test there when what the screen shows changes.

## Isolated check

Never run a test build against the real data folder: it holds Nelson's accounts, his
mail cache and, through the keychain, his passwords. `--demo` runs the real UI on
made-up accounts and mail, connecting nowhere; a temporary `TM_DATA_DIR` keeps even
its settings away from his. Run it on a throwaway tmux server, never on `at`:

```powershell
$exe = (Resolve-Path .\target\debug\tuimail.exe).Path
$data = Join-Path ([IO.Path]::GetTempPath()) ('tuimail-qa-' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $data
tmux-throwaway start tuimail-qa -- -x 150 -y 40 -e "TM_DATA_DIR=$data" $exe --demo
tmux -L tuimail-qa capture-pane -p
```

Allow a moment for the first frame before capturing. Check what the change touches,
and always:

- `/` opens the palette; typing searches it, Escape closes it, and a command runs
  as its key would. A key read together with others while it's open is typing.
- `/` then `info` shows Repo, Link, Version, Commit, Built, Installed and Checkout. A local
  build says it isn't from CI; a CI build shows its commit and build time.
- Resize to a narrow window (60 x 15) and a wide one: the reader moves beside, below
  or in place of the list without losing the selection.
- `q` quits, and the pane's process is gone.

Shift+drag across a line and paste it in at-terminal itself to check selection: a
tmux capture doesn't exercise the terminal's. `q` ends the server with its only
pane (`tmux-throwaway end tuimail-qa` if it's still up); then remove `$data` after
checking it is the `tuimail-qa-*` folder under the temporary directory.

## Previewing a change for Nelson

Run the test build as above, then put it in his window with
`tmux-throwaway show tuimail-qa`. Before describing it, read what he will see: the
shown pane is alive (`#{pane_dead}` is `0`) and `tmux -L tuimail-qa capture-pane -p`
shows the case being described. The turn his answer settles it, end the server.

## Update the existing window after a release

Each push to main releases tuimail and starts home-sync, which installs it at
`C:\Program Files\at-home\tuimail\current\tuimail.exe` on every home PC, and Argo CD
restarts the session on the home cluster when the release changes its image. Every tuimail
already running keeps the old version until it restarts: the `mail` window on each home PC
(they all go through the session: nelsonpc, upstairspc, treadmill, and nelsonlaptop when it
runs one), and the `tuimail mcp` each Claude session started. Restarting all of them is mine to do, never his, and none is left on the
old release as a limit to report.

1. Confirm the new release is installed: `tuimail.exe --version` from the managed
   path, and its Authenticode signature is `Valid` from `CN=at-terminal`.
2. Find each PC's tuimail: here, the `mail` window of the `system` session
   (`tmux list-panes -t system:mail -F '#{pane_id} #{pane_current_command}'`); on another
   PC, over `ssh <pc>`, every pane running it in its `at` server
   (`tmux -L at list-panes -a -F '#{session_name}:#{window_name} #{pane_id} #{pane_current_command}'`;
   a PC can have more than one), with the steps below sent through `ssh` too.
3. Capture it first. If a composer is open, a draft is unsaved or a confirmation is
   asked, leave it and tell him; otherwise go on.
4. `tmux respawn-pane -k -t <pane id> tuimail.exe ''` (the empty argument keeps tmux
   from running it through a shell, as `windows.conf` starts it).
5. Read the pane back: it's alive, it runs the new version (`/` then `info`, then
   any key to close), and his accounts are checking. Send keys one per
   `tmux send-keys` call, from PowerShell (Git Bash turns a lone `/` into a path), and
   never `C-c`: through tmux it ends tuimail rather than closing the palette.
6. Stop every `tuimail mcp` process started before the install (its command line ends
   in ` mcp`; compare its start time with the installed file's), this session's and
   every other session's. Claude Code starts a new one at the session's next tuimail
   call, which then answers with the new release's tools.
7. Only then say it's updated.
