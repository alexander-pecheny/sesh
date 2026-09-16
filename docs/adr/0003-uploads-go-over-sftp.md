# Uploads go over SFTP, on the live SSH handle where there is one

An Upload opens an SFTP subsystem on a channel of the russh handle the Session already
holds. `russh-sftp` speaks the protocol over `channel.into_stream()`, the same move agent
forwarding makes in `ssh.rs`. A mosh Session dropped its handle after bootstrapping
`mosh-server`, so it opens a fresh SSH connection per Upload and closes it after.

SFTP returns typed status codes, so "permission denied" and "no space left on device"
reach the user as themselves rather than as "upload failed". Its `realpath` also resolves
the home directory, which the inserted path needs: a tilde only expands when a shell reads
the word, and the path may be read by a program instead.

## Considered options

- The scp protocol. OpenSSH 9.0 deprecated it in April 2022, and iOS cannot shell out to
  an `scp` binary the way cmux does, so it would mean hand-rolling the `C0644` handshake
  against a remote `scp -t`. Rejected.
- `exec` a shell command, `mkdir -p dir && cat > path`. The shortest of the three and
  guaranteed to work wherever a Host gives a shell, which Sesh's premise says is
  everywhere. Rejected for its errors: a shell exit status and a line of stderr, with no
  way to tell a missing directory from a full disk.
- Holding the mosh bootstrap connection open for the Session. mosh exists to survive a
  network change; a TCP connection opened ten minutes ago does not, and TCP is slow to
  admit it. The connection would be dead exactly when an Upload wanted it. Rejected.

One connection per mosh Upload costs an authentication each time. That is silent with a
key or a saved password, and the prompt it otherwise raises is the existing `AuthSheet`,
whose "Save password" toggle stops the asking.
