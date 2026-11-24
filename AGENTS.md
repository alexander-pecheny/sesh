# Agents

Merge every change into `main` as soon as it is committed and builds, features as well as fixes, and push `main`. Never leave finished work on a branch: work left there has been lost for weeks more than once. Rebase on `main` first when it has moved on.

Never install the Mac app into /Applications from anything but `main`. Merge the work into `main` first, then run `just mac` on it. `scripts/install-mac.sh` refuses a commit that `main` does not contain, or uncommitted changes.
