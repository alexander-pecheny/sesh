# Claude Transcript corpus

Real Claude Code Transcripts, cut to the lines that matter and anonymised, which
`tests/claude.rs` reads through the parser. Each `.jsonl` has a `.expected` file beside it
with the entries a device would be shown, the background work left and whether the turn is
over. `SAMPLES` in the test says what each one holds, and the test fails when the corpus has a
Transcript it does not list.

A main Transcript is `session.jsonl`; a subagent's keeps its name under `subagents/`, since
the parser reads a file in a `subagents` folder as a subagent's.

## What the anonymiser keeps

`scripts/claude-corpus.py` turns every word into a made-up word of the same length, the
same word into the same made-up word within one run, and every digit run into other digits.
It keeps field names, types, enum values, ids, `version`, timestamps, file extensions, the
tags Claude wraps its notices in, and the wording of the notices the parser reads, such as
`[Request interrupted by user]` or "This agent stopped with background work of its own
still running". Images become a one-pixel PNG, and text past 4000 characters keeps only its
head and tail. Lines the parser never reads, such as most attachments, are left out.

## Refreshing it for a new Claude version

Do this when Claude Code changes how it writes a Transcript, or every few releases.

1. Use the new version for real work until its Transcripts hold the situations in
   `SAMPLES`: background commands and Agents, a shell moved to the background with Ctrl+B or
   a timeout, a Monitor, a fork, a subagent waiting on its own shells, TaskStop, a message
   queued mid-turn, an image, slash commands, `/clear`, `/compact` and an automatic
   compaction, AskUserQuestion and an interrupted turn.
2. Find the lines with `uv run python scripts/claude-corpus.py list FILE`, which prints
   only each line's structure.
3. Write a recipe outside the repo, since the source paths name projects:
   `{"background-bash": ["~/.claude/projects/.../SESSION.jsonl:120-200"], ...}`. List a
   session's subagent after it to record both into one sample.
4. Record it twice, as it is into a scratch folder and anonymised into this one:

   ```
   uv run python scripts/claude-corpus.py recipe /tmp/raw RECIPE.json --raw
   uv run python scripts/claude-corpus.py recipe core/sesh-transcript/tests/claude RECIPE.json
   ```

5. Bless both, from `core`, and compare them. The two sets of `.expected` files must list
   the same kinds of entries in the same order, the same background work and the same turn;
   only the text may differ. A difference means the anonymiser broke a notice the parser
   reads, so add it to `NOTICES` in the script.

   ```
   SESH_CLAUDE_CORPUS=/tmp/raw SESH_BLESS=1 cargo test -p sesh-transcript --test claude
   SESH_BLESS=1 cargo test -p sesh-transcript --test claude
   ```

6. Read the new files for leaks before committing: names of companies, people, hosts,
   projects and branches, paths, emails, URLs and tokens. `grep -ri` for the ones you know
   of. Delete the raw copy.
7. Add each new sample to `SAMPLES` with what it holds and the background work and turn
   it must end with. Keep the older samples, since a Host may still run an older Claude.

A Claude update that changes a format the parser reads makes the test fail against these
files. Fix the parser, check that the new `.expected` reads right, and bless.
