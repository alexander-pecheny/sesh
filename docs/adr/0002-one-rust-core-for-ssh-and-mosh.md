# One Rust core carries both SSH and mosh

mosh comes from rmosh (code.pecheny.me/pecheny/rmosh), a Rust port, so the app already
links a Rust static library. SSH lives in the same library, on `russh`, rather than in a
second C stack such as libssh2 plus OpenSSL. The app sees one `SeshCore.xcframework`
with one C ABI; the mosh bootstrap runs `mosh-server` over the same russh session and
hands the handshake to the rmosh client in-process.

## Considered options

- libssh2 in Swift: needs OpenSSL cross-compiled for device and simulator, and a second
  FFI surface. Rejected.
- SwiftNIO SSH: pure Swift, but thin on auth methods and known-hosts handling. Rejected.

rmosh is a git submodule and owns the client as a library: its session loop takes byte
sources and sinks rather than a tty, and the `mosh-client` binary is one caller of it.
`sesh-core` depends on the submodule's crates by path and never patches them.
