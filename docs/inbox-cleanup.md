# Inbox cleanup with an agent

How an AI agent connected through `tuimail mcp` helps the user catch up on mail and cut down
what arrives, one decision at a time. The user decides; the agent finds, explains and does.
Everything here goes through tuimail's agent tools (`search_messages`, `read_message`,
`mark_read`, `move_messages`, `mark_spam`, `unsubscribe`), so every change shows in the
open tuimail and undoes with `z`.

Ranking and counting across thousands of messages can read the cache (`tuimail.db` in the data
folder) directly, opened read-only (`file:...?mode=ro`); every change still goes through the tools.

## Catching up

1. Find where the user was last caught up: going back from today, the point where the inbox turns
   from mostly unread into a long run of read mail. If there's no such point, say so and name the
   window used instead.
2. Read every message from there to today (`search_messages` paged with `next`; at the end of
   what's kept, `older` fetches more from the server). Report, in order:
   - what needs the user: a deadline, money, security, a person waiting for an answer;
   - what's useful to know: trips, orders on their way, appointments;
   - a one-line count of the rest.
3. When the user says they've dealt with it, mark read and archive the whole window, not just
   what was reported, except what the user asked to keep.
4. Put back in the inbox what's still of interest after archiving: upcoming trips' bookings,
   orders not yet delivered, anything the user is waiting on.

## Older mail

Fetch the whole inbox (search past the cache until `older` is null) and look first for mail from
real people: personal addresses, replies, names the user knows. Most other old unread mail is
bulk. Report the people's messages that still matter, keep those in the inbox, and clear the rest
when the user says so.

## Cutting down senders

1. Rank senders by how much they sent over the last three months, across the inbox and the
   archive, one message per Message-ID. Leave out what the user relies on (banks, deliveries,
   accounts, security notices, their own tools) and what they're using now (a house search's
   listings).
2. Take the candidates one at a time, as "item n of N". For each, say:
   - what the company is, in a line;
   - how they got the address: the first message from them, and any order, receipt, account or
     welcome message in any folder (and what was bought);
   - how much they send and since when;
   - which of their addresses are promotions and which carry orders or account mail, when they
     differ, so only the promotions go.

   Then ask whether to unsubscribe.
3. Real spam (a sender the user never dealt with, political or scam mail): mark it spam rather
   than unsubscribe, since unsubscribing tells the sender the address works.
4. Keep the decisions in the work package for the cleanup, then unsubscribe from all of them with
   `unsubscribe`: one-click where the sender supports it (RFC 8058), else an email to the list's
   unsubscribe address; a sender with only a web page is left to the user, with its site named.
5. Archive what the unsubscribed senders already sent, unless the user wants it kept.
