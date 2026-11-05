---
status: accepted
---

# A Vault moves to another Host through the Mac, and its old Host says where it went

A Vault lives on one Host (ADR 0008), and the user sometimes needs it on another, as when a
machine is retired. Moving it by hand meant copying the database, cloning every repository,
carrying each Worktree's uncommitted work, putting each Transcript where its Agent looks, and then
telling every device. A device left pointing at the old Host kept writing there.

The Mac moves a Vault, from the Vault's menu or as `Sesh vault move NAME --to ALIAS`, which run
the same code. Everything travels through the Mac, a few megabytes per command, since the two
Hosts may not reach each other.

- The Agents of the open Tasks on the old Host stop after their Transcripts are copied into the
  Vault a last time, their Terminals close, and the Mac sends its queued changes. Then a file
  `moved.json` beside `vault.db` names the new Host: its alias on the Mac, and the address, user
  and port that alias reaches. From then on the old Host refuses every push.
- The helper copies the database with `VACUUM INTO`, so the copy is whole while other connections
  write, and the Mac carries it with the Transcript copies as one archive. The Vault keeps its id
  and its numbering, so every device carries on from where it was.
- Each repository the open Tasks use is cloned from its origin on the new Host when it lacks one.
  Each Worktree carries its commits that no remote has as a git bundle, and its uncommitted and
  untracked work as one binary diff, made against a copy of git's index so the old Worktree is
  left as it was. Ignored files stay behind. A diff that does not apply is kept on the new Host.
- Each session's last Transcript is placed from the Vault's copy, as Resume on another Host does,
  so its sessions come back ended and ready to resume. sesh-mux's hooks are installed again
  afterwards, since they go only into account folders that hold Transcripts.
- The new Host is checked for each Agent the sessions use and for a login in each Claude account
  folder they used. Sesh reports what is missing and never copies a login.
- The records are rewritten on the new Host in one push: the new alias, and every path under the
  old home moved under the new one. The marker then says the move is done.

A device learns of a move from the head line of `vault init`, of a refused push, or of a follow,
which says so at once although no record changed. While the move runs it waits. Once it is done,
the Mac follows if its own ssh alias reaches the same address, and the phone follows to its Host
at that address as that user, else to the Host of that name. Otherwise the device asks the user to
add that Host. A move that fails takes the marker away, and the Vault carries on where it was.

## Considered options

- Host-to-Host copying with scp or rsync. Faster, but the Hosts often cannot reach each other,
  and the phone could never run it. Rejected.
- Rebuilding the new Vault from a device's copy, as a refill after an erased Host does (ADR 0023).
  A device holds no Transcripts and may lag the Host. Rejected.
- Deleting the old Vault after the move. A device that has not followed yet would find nothing to
  tell it where to go. Rejected.

## Consequences

Archived Tasks keep their records, so a session in a reopened one still names the old Host until
it resumes elsewhere from the Vault's copy. A change another device queued for a moved session
before it followed overwrites the rewritten record. A Worktree whose commits only the old Host
had is carried only if origin holds the commits it builds on.
