<div align="center">

# tuimail

**Your mail, at the speed of your keyboard.**

A terminal email client (TUI): IMAP and POP3 to read, SMTP to send, several accounts side by side.<br>
Arrow keys or vim keys. Passwords in your system's keychain. Nothing loaded from the internet when you read.

[Releases on Forgejo](https://forgejo.romaine.life/romaine-life/tuimail/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024-orange.svg)](https://www.rust-lang.org)

**[Get started](#get-started)** · **[Keys](#keys)** · **[Accounts](#accounts)** · **[Settings](#settings)** · **[AI agents](#ai-agents-mcp)** · **[Privacy](#privacy-and-security)**

<img src="docs/hero.png" alt="tuimail: a sidebar with All inboxes and two accounts' folders, beside the work inbox's messages with stars, replies and attachments marked, and Alice's message 'Lunch tomorrow?' open in the reader: its sender, the attached menu, an underlined link and quotes colored by depth">

<sub>The demo's made-up mail: <code>tuimail --demo</code></sub>

</div>

---

This is Nelson's fork of [erictran308/tuimail](https://github.com/erictran308/tuimail), with a `terminal` theme that uses the terminal's own background, text and ANSI colors. Its source and releases live on [Forgejo](https://forgejo.romaine.life/romaine-life/tuimail); GitHub mirrors the source.

## Why tuimail

- **IMAP, POP3 and SMTP.** IMAP accounts keep mail on the server, in its folders; POP3 accounts download the inbox and keep Sent, Drafts, Archive and Trash on your computer. Both send through SMTP, with TLS or STARTTLS.
- **New mail at once.** Where the server can (IMAP IDLE), new mail shows the moment it arrives, with a notification through your terminal. Elsewhere tuimail checks every few minutes, and `R` checks now.
- **Several accounts.** Each in the sidebar with its folders, and **All inboxes** together at the top. Write from any of them (`←` `→` on From).
- **Every command a search away.** `/` opens the command palette: type part of what you want ("archive", "reply to all", "check for mail") and `Enter` runs it. Every key has a command there, and one that can't run where you are says why.
- **Arrows or vim, your choice.** `←↓↑→` and `h j k l` both move, `Enter` opens, `gg` / `G`, `Ctrl-d` / `Ctrl-u`. `?` lists every key, and `O` opens the settings.
- **Reading made calm.** HTML mail is shown as text (bold, italics, links numbered), with nothing it points to fetched: no tracking pixels, no "read" receipts by image. Quotes are colored by depth. `H` shows the plain-text part instead, `o` lists the links, `A` the attachments.
- **Writing in place, or in your editor.** To, Cc, Bcc and Subject with addresses completed from people you've written to or heard from; the text in place, or `Ctrl-e` for `$EDITOR`. Drop a file on the window to attach it. Replies quote, forwards carry the attachments, drafts go to Drafts and open again where you left them.
- **Quick to tidy.** `d` deletes (to Trash), `e` archives, `!` marks junk, `m` moves anywhere, `s` stars, `u` marks read or unread, `t` tags. `Space` marks several messages for any of these at once, and `z` undoes the last of them (`Z` lists them, to undo any). `U` unsubscribes from a mailing list.
- **Conversations.** Replies sit under what they answer, the conversation with the newest message first, so a thread reads top to bottom. They're worked out from the messages themselves, the same for every server and POP3; turn them off in the settings (`O`) for a plain newest-first list.
- **Tags.** Put `work`, `receipts` or any word on a message with `t`. IMAP accounts keep them on the server as keywords, spelled the way Thunderbird spells its tags, so the two show each other's; POP3 accounts keep them on your computer. (Gmail keeps them too, but its website shows only its labels.)
- **Search.** `/`, then `search` and `Enter`, finds mail by sender, recipients, subject or text: at once in what's on your computer, then on the server, which searches whole messages. `tag:work` finds what's tagged.
- **AI agents, on your terms.** Claude Code, Claude Desktop, Cursor or any MCP client can read and sort your mail through `tuimail mcp`: mark, star, tag, move, mark spam, move to Trash, and unsubscribe you from mailing lists when you ask. Each change asks you first (unless you say not to), shows on the screen and undoes with `z`. Agents can't delete anything for good, write mail, or open links.
- **Works offline.** Headers and the messages you've opened are kept in a local cache, so the list and what you've read are there without a connection.
- **Set up in a minute.** Type your address and password: the servers fill in for Gmail, iCloud, Outlook, Yahoo, Fastmail, Proton (through its Bridge), Zoho, GMX, WEB.DE, Yandex, Mail.ru, Posteo, mailbox.org and more, or are guessed for others. tuimail logs in to check before saving.
- **Make it yours.** Catppuccin, Tokyo Night, Dracula, Gruvbox, Nord and Rosé Pine themes (picked in the settings, `O`), and Nerd Font icons where your terminal has them.
- **Private by design.** No account with anyone but your mail provider, no telemetry, no servers in between. Your mail, accounts and settings stay in one folder on your computer, readable only by you.

## Get started

**1. Install tuimail.** Forgejo requires authentication for release downloads, including this public repository. These commands use the Forgejo credential already stored for Git and download the compiled release.

On Linux, choose your file from the table below and run this in Bash. It checks the download against the release's `SHA256SUMS` and puts it in `~/.local/bin` as `tuimail`; the authentication header reaches curl through stdin:

```bash
(
  set -euo pipefail
  set +x
  asset=tuimail-linux-amd64
  forgejo_token=$(printf 'protocol=https\nhost=forgejo.romaine.life\n\n' |
    git credential fill | sed -n 's/^password=//p')
  [ -n "$forgejo_token" ] || { printf 'No stored Forgejo credential found.\n' >&2; exit 1; }
  download=$(mktemp -d)
  trap 'rm -rf "$download"' EXIT
  for file in "$asset" SHA256SUMS; do
    printf 'Authorization: token %s\n' "$forgejo_token" |
      curl --fail --silent --show-error --location --header @- --output "$download/$file" \
        "https://forgejo.romaine.life/romaine-life/tuimail/releases/download/latest/$file"
  done
  (cd "$download" && grep -E "^[0-9a-f]{64}  $asset\$" SHA256SUMS | sha256sum --check --quiet -)
  mkdir -p "$HOME/.local/bin"
  chmod 755 "$download/$asset"
  mv "$download/$asset" "$HOME/.local/bin/tuimail"
)
```

On Windows, choose the file for your architecture and run this in PowerShell. It checks the download against the release's `SHA256SUMS` and installs it as `%LOCALAPPDATA%\Programs\tuimail\tuimail.exe`; the token stays in an in-process header:

```powershell
$ErrorActionPreference = 'Stop'
$asset = 'tuimail-windows-amd64.exe'
$passwordLine = @("protocol=https`nhost=forgejo.romaine.life`n`n" |
  git credential fill | Where-Object { $_ -like 'password=*' })[0]
if (-not $passwordLine) { throw 'No stored Forgejo credential found' }
$headers = @{ Authorization = 'token ' + $passwordLine.Substring(9) }
$release = 'https://forgejo.romaine.life/romaine-life/tuimail/releases/download/latest'
$download = Join-Path ([IO.Path]::GetTempPath()) ("tuimail-$([guid]::NewGuid().ToString('N'))")
$installDir = Join-Path $env:LOCALAPPDATA 'Programs\tuimail'
try {
  New-Item -ItemType Directory -Path $download | Out-Null
  Invoke-WebRequest -Headers $headers -OutFile (Join-Path $download $asset) -Uri "$release/$asset"
  Invoke-WebRequest -Headers $headers -OutFile (Join-Path $download 'SHA256SUMS') -Uri "$release/SHA256SUMS"
  $line = @(Get-Content -LiteralPath (Join-Path $download 'SHA256SUMS') |
    Where-Object { $_ -cmatch "^[0-9a-f]{64}  $([regex]::Escape($asset))$" })
  if ($line.Count -ne 1) { throw "SHA256SUMS doesn't list $asset" }
  if ((Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $download $asset)).Hash -ne $line[0].Substring(0, 64)) { throw "$asset doesn't match SHA256SUMS" }
  New-Item -ItemType Directory -Force -Path $installDir | Out-Null
  Move-Item -Force -LiteralPath (Join-Path $download $asset) -Destination (Join-Path $installDir 'tuimail.exe')
} finally {
  if (Test-Path -LiteralPath $download) { Remove-Item -Recurse -LiteralPath $download }
  $headers.Clear()
  $passwordLine = $null
}
& (Join-Path $installDir 'tuimail.exe') --version
```

Set `asset` to the file for your computer. You can also sign into Forgejo in your browser and download it through these links:

| Computer | File |
| --- | --- |
| Linux, x86_64 | [`tuimail-linux-amd64`](https://forgejo.romaine.life/romaine-life/tuimail/releases/download/latest/tuimail-linux-amd64) |
| Linux, ARM64 | [`tuimail-linux-arm64`](https://forgejo.romaine.life/romaine-life/tuimail/releases/download/latest/tuimail-linux-arm64) |
| Windows, x86_64 | [`tuimail-windows-amd64.exe`](https://forgejo.romaine.life/romaine-life/tuimail/releases/download/latest/tuimail-windows-amd64.exe) |
| Windows, ARM64 | [`tuimail-windows-arm64.exe`](https://forgejo.romaine.life/romaine-life/tuimail/releases/download/latest/tuimail-windows-arm64.exe) |

- **Windows:** add `%LOCALAPPDATA%\Programs\tuimail` to your `PATH`, or run `tuimail.exe` there from Windows Terminal. A browser download can instead be saved as `tuimail.exe` in a folder of your choice.
- **Linux:** needs glibc 2.35 or newer: Ubuntu 22.04, Debian 12, Fedora 36 or later. A browser download is renamed `tuimail` and made executable (`chmod +x tuimail`).

If `tuimail` isn't found afterwards, add `~/.local/bin` to your `PATH`. The Forgejo release includes `SHA256SUMS` for all four files. There's no Mac build. Compare a browser download's SHA-256 checksum with its matching line in that file. Windows executables carry the at-terminal Authenticode signature; the release files do not claim GitHub provenance attestations.

**2. Run it.**

```sh
tuimail
```

The first start asks for an account: your address and password (for Gmail, iCloud, Yahoo and others, an **app password**: see [App passwords](#app-passwords)). The servers fill in; `Ctrl-s` logs in to check them and saves the account, with the password in your system's keychain.

To look around first, `tuimail --demo` shows made-up accounts and mail, connecting nowhere and sending nothing.

### Check your setup

```sh
tuimail --check
```

It says where your data folder is, whether a keychain is there for passwords, and each account it found: its servers, security and where its password comes from. An account it can't use, or a line in `accounts.toml` it doesn't understand, shows up as a warning.

## Keys

The status bar shows the keys for where you are, and `?` lists them all, each with the name the command palette finds it by. `Tab` there goes to the settings.

### Command palette

`/` opens it (`Ctrl-p` while writing, where `/` is text). Type to find a command, `Enter` to run it; `→` and `←` go into and out of its folders, and `Esc` clears what's typed, leaves a folder, then closes it. Every key below has a command, which does exactly what the key does where you are. One that can't run there is listed after the rest with the reason, which shows again if you pick it.

- **Search:** `/`, type `search`, `Enter`, then the words.
- **Info:** `/`, type `info`, `Enter`: this build's repo and link, its version and commit, when it was built, where it's installed and its checkout under `D:\repos`.

The palette is [fzt](https://forgejo.romaine.life/romaine-life/fzt-terminal), loaded from its install (`C:\Program Files\at-home\fzt\current\fzt.dll`, put there by home-sync on the Windows home PCs). Without it, `/` says why it couldn't open; every key still works.

| Key | Action |
| --- | --- |
| `←↓↑→` / `h j k l` | Move; `Enter`, `→` or `l` opens |
| `Tab` | Folders → messages → the message |
| `gg` / `G`, `Ctrl-u` / `Ctrl-d` | First / last, half a page |
| `c` | Write a message |
| `r` / `a` / `f` | Reply / reply to all / forward |
| `d` | Delete: to Trash; from Trash, for good (asking first) |
| `e` / `!` / `m` | Archive / junk / move to a folder |
| `s` / `u` | Star / read or unread |
| `t` | Tag: put a tag on or take it off, or type a new one |
| `z` / `Z` | Undo the last change (a star, read, tag or move) / list the changes, to undo any |
| `Space` / `*` | Mark a message / all of them; the keys above act on the marked |
| `/` | The command palette; `/` `search` `Enter` searches, and `Esc` ends the search |
| `J` / `K` | Reading: the next / previous message |
| `Space` / `b` | Reading: a page down / up |
| `o` / `A` | Reading: the links (`Enter` open, `y` copy) / attachments (`Enter` open, `s` save) |
| `H` | Reading: HTML or plain text |
| `U` | Unsubscribe from a mailing list |
| `R` | Check for mail now |
| `+` / `x` / `P` | Add an account / remove it (on its row) / type its password, or sign in |
| `?` / `O` | Every key / the settings and themes |
| `q` | Quit |

Writing:

| Key | Action |
| --- | --- |
| `Tab` / `Shift-Tab`, `Enter` | The next / previous field; `Tab` takes a suggested address |
| `Ctrl-s` | Send (asking first, unless turned off) |
| `Ctrl-d` | Save in Drafts and close |
| `Ctrl-o` | Attach a file (or drop it on the window) |
| `Ctrl-e` | Write the text in your editor (`$VISUAL`, `$EDITOR`, or `editor` in the settings) |
| `←` `→` on From | Send from another account |
| `x` on Files | Take a file off |
| `Esc` | Close: `y` keeps it in Drafts, `n` throws it away |

## Accounts

Accounts are kept in `accounts.toml` in [your data folder](#your-data). The setup screen (`+`) writes it; you can also edit it while tuimail isn't running:

```toml
[[account]]
name = "work"                      # what the sidebar calls it
email = "me@example.org"
display_name = "Sam Taylor"        # your name, as recipients see it
protocol = "imap"                  # or "pop3"
host = "imap.example.org"
port = 993                         # if not the usual one
security = "tls"                   # "tls", "starttls", or "none" (this computer only, unencrypted)
smtp_host = "smtp.example.org"
smtp_port = 465
smtp_security = "tls"
password = "keychain"              # "keychain", "ask" (each start) or "command"
signature = "Sam"
```

More keys: `username` and `smtp_username` (if not the address), `password_command` (below), `save_sent` (copy sent mail to Sent; off by default for Gmail and Outlook, which do it themselves), `certificate` (a server certificate to trust, for Proton Mail Bridge or your own server), `leave_on_server` (POP3, on by default), and `sent_folder`, `drafts_folder`, `trash_folder`, `archive_folder`, `junk_folder` when the server doesn't say which folder is which.

`x` on an account's row in the sidebar removes it, its cached mail, and its password from the keychain.

### App passwords

Gmail, iCloud, Yahoo, AOL, Fastmail and others don't take your normal password from mail programs: make an app password in your account's security settings and use that. For Gmail: [myaccount.google.com/apppasswords](https://myaccount.google.com/apppasswords) (it needs 2-Step Verification on).

### Passwords from a command

To keep the password in a password manager instead of the keychain:

```toml
password = "command"
password_command = ["/opt/homebrew/bin/pass", "show", "mail/work"]
```

The program is given by its full path and run without a shell; tuimail uses the first line it prints. It runs once at start with your terminal, so it can ask you to unlock something.

### Signing in with Microsoft or Google (OAuth2)

Outlook.com takes only OAuth2, Microsoft 365 is moving to it, and Gmail takes it as well as app passwords. tuimail signs in itself: add the account with `+` and leave the password empty, and `Ctrl-s` opens your browser on Microsoft's or Google's sign-in page. Once you've signed in there, the browser comes back to tuimail (on this computer only), and tuimail keeps the sign-in in your keychain: a token that lasts, never your password. If the sign-in is taken back (your password changed, the app was removed from your account), tuimail says so, and `P` on the account's row signs in again.

**An app to sign in as.** Microsoft and Google sign people in only for apps registered with them. Until tuimail has registrations of its own, register one for yourself (free, about ten minutes) and name it in `accounts.toml`:

- **Microsoft** (Outlook.com, Hotmail, Microsoft 365): at [entra.microsoft.com](https://entra.microsoft.com), **App registrations** → **New registration**. Name it, pick **Accounts in any organizational directory and personal Microsoft accounts**, and as the redirect URI choose **Public client/native (mobile & desktop)** with `http://localhost`. Copy its **Application (client) ID**.
- **Google** (Gmail): at [console.cloud.google.com](https://console.cloud.google.com), make a project and turn on the **Gmail API**. In **Google Auth Platform**, set up the consent screen (External), add the scope `https://mail.google.com/`, and either add yourself as a test user (then you sign in again every 7 days) or publish the app. Then **Clients** → **Create client** → **Desktop app**, and copy its client ID and secret (a desktop app's "secret" isn't one: Google says so).

```toml
[[account]]
email = "me@outlook.com"
host = "outlook.office365.com"
smtp_host = "smtp-mail.outlook.com"
smtp_port = 587
smtp_security = "starttls"
auth = "oauth2"
oauth_client_id = "your Application (client) ID"
# Google only:
# oauth_client_secret = "the client's secret"
```

Start tuimail, and `P` on the account's row signs in. `tuimail --check` says which accounts sign in.

**Or with a program of yours.** tuimail also logs in with an access token printed by a command, run again for each connection so a short-lived token stays fresh. [oama](https://github.com/pdobsan/oama) and the `mutt_oauth2.py` script both do this:

```toml
auth = "oauth2"
password_command = ["/opt/homebrew/bin/oama", "access", "me@outlook.com"]
host = "outlook.office365.com"
smtp_host = "smtp-mail.outlook.com"
smtp_port = 587
smtp_security = "starttls"
```

### Proton Mail

Run [Proton Mail Bridge](https://proton.me/mail/bridge), use the password it shows for the account, and export its certificate (Bridge → Settings → Export TLS certificates), then point tuimail at it:

```toml
host = "127.0.0.1"
port = 1143
security = "starttls"
smtp_host = "127.0.0.1"
smtp_port = 1025
smtp_security = "starttls"
certificate = "/Users/you/Documents/bridge/cert.pem"
```

## Settings

`O` (or `?` then `Tab`) opens the settings: the message beside the list, plain text or HTML, marking read, conversations, asking before deleting and before sending, notifications, AI agents, and the theme. `j` / `k` move, `Enter` or `Space` turns one on or off (or picks the theme), and it's saved at once, changing only its own line in `settings.toml` and keeping the rest of the file (but not its comments).

Everything else is in `settings.toml` in [your data folder](#your-data). Every line is optional; changes made by hand take effect the next time tuimail starts:

```toml
theme = "terminal"       # your terminal's own colors, or latte, frappe, macchiato, mocha, tokyonight, dracula, gruvbox, nord, rose-pine
icons = "auto"           # auto (Nerd Font icons in Ghostty, kitty and WezTerm), nerd or plain
notifications = "auto"   # auto (what your terminal understands), off, bell, osc9, osc777 or osc99
preview = true           # the open message beside the list, when the window is wide enough
prefer_plain = false     # a message's plain-text part rather than its HTML, when it has both
mark_read = true         # mark a message read once it's shown
threads = true           # group the list in conversations
confirm_delete = false   # ask before moving mail to Trash
confirm_send = true      # ask before sending (a message without a subject always asks)
check_minutes = 5        # how often to check where the server can't say at once, and folders other than the inbox (1 to 1440)
fetch_count = 300        # how many of a folder's newest messages to fetch at first (20 to 5000); older ones as you scroll
sidebar_width = 28       # in columns (16 to 50)

agents = false           # let AI agents read and sort your mail (tuimail mcp), while tuimail is open
confirm_agents = true    # ask before each change an agent makes
agent_changes_per_hour = 300  # the most messages agents change in an hour (1 to 100000)

# Where `s` saves attachments, if not your Downloads folder:
# downloads = "/Users/you/Mail attachments"
# The editor for Ctrl-e, as a full path with its arguments, if not $VISUAL or $EDITOR:
# editor = ["/usr/bin/vim"]
```

## AI agents (MCP)

`tuimail mcp` is an [MCP](https://modelcontextprotocol.io) server: an AI agent starts it, and it reaches your mail through the tuimail you have open. Nothing is kept in it and nothing goes around tuimail: every request is checked, and every change is made by tuimail as your keys would make it.

**1. Turn it on.** In tuimail, `O`, then **Let AI agents read and sort your mail**. tuimail must be open for agents to reach your mail.

**2. Add it to your agent**, by tuimail's full path (`which tuimail`):

- **Claude Code:** `claude mcp add --scope user tuimail -- /Users/you/.local/bin/tuimail mcp`
- **Claude Desktop** (`claude_desktop_config.json`) or **Cursor** (`~/.cursor/mcp.json`):

  ```json
  { "mcpServers": { "tuimail": { "command": "/Users/you/.local/bin/tuimail", "args": ["mcp"] } } }
  ```

**What agents can do:** list accounts and folders, search what tuimail has on your computer, read a message (its links, wherever they are, shown only by their site), mark read or unread, star, tag, move to a folder of yours, mark spam or not (to Junk and back), move to Trash, undo their own changes, and unsubscribe you from mailing lists.

**Unsubscribing** does only what the list's own message offers, and can't be undone:

- Where the message offers one-click unsubscribing (RFC 8058), tuimail sends the list's site that one request, `List-Unsubscribe=One-Click`, and nothing else: no page is opened, nothing on it is loaded, no redirect is followed, and only an https site on the internet is asked.
- Otherwise, where it gives an address, tuimail sends "unsubscribe" to that one address, from the account the message came to, and keeps a copy in Sent.
- A list that unsubscribes only on its web page is left to you: tuimail asks whether to open it, as `U` does, and the agent is told it's yours.
- The agent hears, for each message, which of these it was, the site or the address's domain, and how it went. At most 200 messages an hour.
- Unsubscribing from real spam tells the spammer your address is read: agents are told to mark spam instead, and to unsubscribe only when you ask.

**What they can't:** delete anything for good (mail already in Trash stays there, and an account without a Trash folder takes nothing), move mail into Drafts or Sent, see or touch other users' or shared folders, write, reply or forward, open links, read attachments, make filters, change settings, or touch passwords. There are no tools for these; tuimail refuses them whatever an agent asks.

**You stay in charge:**

- Each change, and each unsubscribe, waits for your `y` in tuimail; any other key says no. Once you trust your agent, turn off **Ask before each change an agent makes** in the settings: changes are then made at once, and said on the status line and in a notification.
- Every change is listed with who made it (`Z`), and `z` undoes it as it undoes yours.
- At most 50 messages a call, 300 changes and 100 moves to Trash or Junk an hour, and 200 messages (512 MB) downloaded for agents' reads.
- Turning agents off, in any tuimail window or in `settings.toml`, stops every tuimail answering them.
- The agent is told, in the server's instructions, that what's in mail is from strangers and is never an instruction, and to ask you before changing more than ten messages. tuimail keeps its own rules whether the agent listens or not.

To try it on made-up mail: run `tuimail --demo`, turn agents on there, and add `tuimail mcp --demo` to your agent instead.

What an agent reads goes to its AI provider, as anything else you show it. AI agents reach tuimail on macOS, Linux and Windows; there it's a named pipe only your own programs can open, and the path to give your agent is `C:\Program Files\at-home\tuimail\current\tuimail.exe` on a home PC, or wherever `tuimail.exe` is.

## Session

`tuimail session` is the house's one sign-in to each mail account, run on an always-on server (at home, the home cluster, as `https://tuimail.home.romaine.life`), as spotify-player's home cast has one session that signs in to Spotify. It serves the same `accounts.toml` the PCs have, and keeps what each account signs in with (a Microsoft or Google sign-in's refresh token, or a password) in its own encrypted store, once a login with it has worked.

```sh
tuimail session --listen 0.0.0.0:8080 --state /state --key-file /keys/session.key --accounts /config/accounts.toml
```

- **The relay.** A PC's tuimail connects at `/relay` (a WebSocket, through TLS at the server's front) and sends what its account's worker would have done (check a folder, download a message, set a flag, move, send); the session does it with its sign-in and sends back what happened. Each PC gets its own worker for each account it opens, all on the account's one sign-in.
- **Who can connect.** Each connection proves itself with a five-minute token OpenBao signs for the home PC (role `tuimail`), renewed in the connection every two minutes; the session checks it against OpenBao's published keys and ends a connection whose token has run out. The session has no login of its own.
- **Its store.** `session.db` in `--state`, SQLCipher, encrypted with the 256-bit key in `--key-file` (64 hex digits). Only a Linux build has SQLCipher; others refuse to run the session.
- **Health and log.** `/health` answers `ok`. The log goes to stdout, the same lines and the same privacy as `tuimail.log`.

PCs going through the session, and the archive it will keep, are still to come (TUI-17).

## Privacy and security

- **Nothing fetched to read.** HTML is turned into text on your computer; pictures are shown by their description, never downloaded, so senders can't tell when you read their mail.
- **Signing in keeps no password.** With Microsoft or Google, you sign in on their page in your browser; tuimail keeps only the sign-in's token, in your keychain, and gets short-lived ones with it as needed. The browser comes back to tuimail on this computer only, and a sign-in is good only to the tuimail that asked for it (PKCE).
- **Passwords stay out of files.** They're in your system's keychain (macOS Keychain, Windows Credential Manager, the Secret Service on Linux), asked for at each start, or printed by your own command. Never in `accounts.toml`, never on a command line.
- **TLS, checked.** Connections use TLS (or STARTTLS, which must succeed before anything is sent) and your system's trusted certificates. An unencrypted connection is allowed only to a server on your own computer, at its loopback address; on Linux, only if that server runs as you or the system. On a computer shared with other accounts, prefer STARTTLS with `certificate` (as for Proton Mail Bridge): there, any account could answer on a free port.
- **Nothing about your computer in what you send.** Messages go out with a random Message-ID and a UTC date, and tuimail greets SMTP servers as `[127.0.0.1]`, so your computer's name never ends up in mail headers.
- **Text from senders can't take over your terminal.** Control characters, escape sequences and direction-changing characters are taken out of everything shown, copied or notified.
- **Attachments are handled with care.** File names are cleaned (no folders, no hidden extensions), files never overwrite one another, they're marked as downloaded from the internet, and only pictures, sound, video, PDF and plain text open without asking (Office files, archives, CSV and the rest ask first). Web links open only through your browser, and only `http` and `https`; the link list shows where each one really goes, with look-alike letters spelled out (`xn--`). `mailto` links and `U`'s addresses open here, as a message to write; `U`'s web page is shown before it opens.
- **Pastes stay text.** A paste that arrives as keystrokes (as it always does on Windows) is never taken for commands, and a question that waits for `y` ignores one typed too soon.
- **Local data is yours alone.** The data folder is made readable only by you; the cache (`tuimail.db`) overwrites what's deleted.
- **The log holds no mail.** `tuimail.log` says what failed and when, in fixed words and numbers: never an address, subject, folder name, a server's wording, a token or a password.

To report a problem, see [SECURITY.md](SECURITY.md).

## Your data

Everything is in one folder on your machine (`tuimail --help` prints its path):

| OS | Location |
| --- | --- |
| macOS | `~/Library/Application Support/tuimail` |
| Linux | `~/.local/share/tuimail` |
| Windows | `%LOCALAPPDATA%\tuimail` |

It holds `accounts.toml`, `settings.toml`, and `tuimail.db`: the cache of your folders, headers and the messages you've opened, POP3 accounts' mail, and the addresses used for completing To, Cc and Bcc. Attachments you open go to its `opened/` folder. `tuimail.log` records starts, stops, crashes and what failed, kept to about 2 MB, safe to share: an account appears only as its number, which `tuimail --check` shows. Passwords aren't in it: they're in your system's keychain. Deleting the folder starts tuimail afresh; remove the accounts with `x` first to take their passwords out of the keychain too.

| Variable | Use |
| --- | --- |
| `TM_DATA_DIR` | Keep the data folder somewhere else (a full path) |
| `TM_ICONS` | Which icons: `auto`, `nerd` or `plain` |

## Development

```sh
cargo test                       # offline tests
cargo clippy --all-targets
TM_DATA_DIR=$HOME/tm-test cargo run   # a separate data folder (a full path) for trying things
cargo run -- --demo              # made-up accounts and mail, nothing connects or is sent
```

The tests against a real server run against [GreenMail](https://greenmail-mail-test.github.io/greenmail/) in Docker (IMAP, POP3 and SMTP over TLS); the commands are at the top of `src/live.rs`, and CI runs them on every pull request that changes the code.

Only development builds read a `.env`, and only from the source folder, never the folder they're started from, and only its `TM_*` lines: an installed tuimail ignores it, so a `.env` in a folder you cloned can't change where your mail is kept.

## Built with

- [ratatui](https://ratatui.rs), [crossterm](https://github.com/crossterm-rs/crossterm) and [ratatui-textarea](https://github.com/ratatui/ratatui-textarea)
- [async-imap](https://github.com/async-email/async-imap) for IMAP; POP3 and SMTP written here
- [mail-parser](https://github.com/stalwartlabs/mail-parser) and [mail-builder](https://github.com/stalwartlabs/mail-builder) from Stalwart, and [html2text](https://github.com/jugglerchris/rust-html2text)
- [rustls](https://github.com/rustls/rustls) with [rustls-platform-verifier](https://github.com/rustls/rustls-platform-verifier), [rusqlite](https://github.com/rusqlite/rusqlite), and [keyring-core](https://github.com/open-source-cooperative/keyring-core) for the keychain
- The same UI groundwork, themes and safety rules as [tuigram](https://github.com/erictran308/tuigram), [tuitube](https://github.com/erictran308/tuitube) and [tuimeta](https://github.com/erictran308/tuimeta)
- Colors from [Catppuccin](https://catppuccin.com), [Tokyo Night](https://github.com/folke/tokyonight.nvim), [Dracula](https://draculatheme.com), [Gruvbox](https://github.com/morhetz/gruvbox), [Nord](https://www.nordtheme.com) and [Rosé Pine](https://rosepinetheme.com)

## License

[MIT](LICENSE)
