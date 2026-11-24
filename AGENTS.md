# Agents

Merge every bugfix into `main` as soon as it is committed. A non-trivial new feature may wait on its branch until I say to merge it.

Never install the Mac app into /Applications from anything but `main`. Merge the work into `main` first, then run `just mac` on it. `scripts/install-mac.sh` refuses a commit that `main` does not contain, or uncommitted changes.
